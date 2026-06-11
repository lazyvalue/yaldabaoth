//! The single owner of agent-session state and identity (spec-agent-session-
//! ownership.md). Every agent conversation lives here, keyed by a stable
//! [`SessionId`]; the server-session-id → `SessionId` index is private and
//! mutated *only* by this module's API. That is what makes the historical
//! family of bugs — two tiles bound to one session, "attached ×4", duplicate
//! forwarders — structurally impossible: the only way to obtain a session for a
//! server sid is [`SessionStore::open_or_focus`], which returns the *existing*
//! one rather than minting a second.
//!
//! The store is generic over the payload `P` so its invariants can be unit-
//! tested without constructing the (large, gpui-bound) [`AgentSession`]. The
//! live app instantiates `SessionStore<AgentSession>` (alias [`AgentSessions`]).

use super::*;
use std::collections::BTreeMap;

/// Stable, monotonic, never-reused local identity for an agent session.
/// Independent of the server session id (absent before attach; may change if
/// `session/load` falls back to `session/new`).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SessionId(pub(crate) u64);

/// Outcome of an idempotent bind: did we mint a new session, or is the sid
/// already shown somewhere (so the caller should focus that tile, not bind a
/// second copy)?
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum Bind {
    Created(SessionId),
    AlreadyOpen(SessionId),
}

impl Bind {
    pub(crate) fn id(self) -> SessionId {
        match self {
            Bind::Created(id) | Bind::AlreadyOpen(id) => id,
        }
    }
}

/// Returned by [`SessionStore::bind_sid`] when the sid is already owned by a
/// different session — the caller must drop the duplicate rather than create a
/// second binding (INV-1/INV-3).
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct AlreadyBound(pub(crate) SessionId);

struct Entry<P> {
    payload: P,
    /// The bound server session id, or `None` for a pre-attach local session.
    /// Stored HERE, not on the payload, so there is exactly one source of truth
    /// for the binding and nothing to keep in sync.
    sid: Option<String>,
}

/// The owner. Private fields — the app reaches sessions only through this API.
pub(crate) struct SessionStore<P> {
    entries: BTreeMap<SessionId, Entry<P>>,
    by_sid: HashMap<String, SessionId>,
    next: u64,
}

impl<P> Default for SessionStore<P> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            by_sid: HashMap::new(),
            next: 0,
        }
    }
}

impl<P> SessionStore<P> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn alloc(&mut self) -> SessionId {
        let id = SessionId(self.next);
        self.next += 1;
        id
    }

    /// Idempotent server-session bind — the ONLY way to attach a sid (INV-1).
    /// If a session already carries `sid`, returns `AlreadyOpen(id)` and mutates
    /// nothing. Otherwise mints a session (payload built by `make`) bound to
    /// `sid` and returns `Created(id)`.
    pub(crate) fn open_or_focus(&mut self, sid: &str, make: impl FnOnce(SessionId) -> P) -> Bind {
        if let Some(&id) = self.by_sid.get(sid) {
            return Bind::AlreadyOpen(id);
        }
        let id = self.alloc();
        self.entries.insert(
            id,
            Entry {
                payload: make(id),
                sid: Some(sid.to_string()),
            },
        );
        self.by_sid.insert(sid.to_string(), id);
        Bind::Created(id)
    }

    /// A fresh local session with no sid yet (pre-attach placeholder). Bind its
    /// sid later with [`bind_sid`] once `attach`/`create` resolves.
    pub(crate) fn create_local(&mut self, make: impl FnOnce(SessionId) -> P) -> SessionId {
        let id = self.alloc();
        self.entries.insert(
            id,
            Entry {
                payload: make(id),
                sid: None,
            },
        );
        id
    }

    /// Bind `sid` to an existing local session. Errors with the *current* owner
    /// if `sid` is already bound elsewhere — the caller drops the duplicate
    /// session rather than creating a second binding (INV-1/INV-3).
    pub(crate) fn bind_sid(&mut self, id: SessionId, sid: String) -> Result<(), AlreadyBound> {
        if let Some(&owner) = self.by_sid.get(&sid) {
            return if owner == id {
                Ok(()) // idempotent re-bind of the same pairing
            } else {
                Err(AlreadyBound(owner))
            };
        }
        let Some(entry) = self.entries.get_mut(&id) else {
            return Ok(()); // session already gone; nothing to bind
        };
        // Releasing any prior sid this session held keeps `by_sid` total.
        if let Some(old) = entry.sid.replace(sid.clone()) {
            self.by_sid.remove(&old);
        }
        self.by_sid.insert(sid, id);
        Ok(())
    }

    /// Release the sid a session currently holds (if any), keeping the session
    /// itself alive in the store under its `SessionId`. After this, the sid is
    /// no longer routable (`locate` returns `None`), so an in-flight
    /// `SessionClosed(sid)` broadcast can't locate — and destroy — a session
    /// that is being respawned (the close-before-create race). Returns the
    /// released sid for teardown bookkeeping.
    pub(crate) fn clear_sid(&mut self, id: SessionId) -> Option<String> {
        let entry = self.entries.get_mut(&id)?;
        let old = entry.sid.take()?;
        self.by_sid.remove(&old);
        Some(old)
    }

    /// O(1) routing: the session bound to `sid`, if any (INV-4).
    pub(crate) fn locate(&self, sid: &str) -> Option<SessionId> {
        self.by_sid.get(sid).copied()
    }

    pub(crate) fn sid_of(&self, id: SessionId) -> Option<&str> {
        self.entries.get(&id).and_then(|e| e.sid.as_deref())
    }

    pub(crate) fn contains(&self, id: SessionId) -> bool {
        self.entries.contains_key(&id)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<&P> {
        self.entries.get(&id).map(|e| &e.payload)
    }

    pub(crate) fn get_mut(&mut self, id: SessionId) -> Option<&mut P> {
        self.entries.get_mut(&id).map(|e| &mut e.payload)
    }

    /// Convenience: locate by sid and borrow mutably in one step (the routing
    /// hot path). `None` if no session is bound to `sid`.
    pub(crate) fn get_by_sid_mut(&mut self, sid: &str) -> Option<&mut P> {
        let id = self.by_sid.get(sid).copied()?;
        self.entries.get_mut(&id).map(|e| &mut e.payload)
    }

    /// Drop a session and release its sid. Returns the payload so the caller can
    /// run teardown (drop the channel/forwarder) outside the borrow.
    pub(crate) fn close(&mut self, id: SessionId) -> Option<P> {
        let entry = self.entries.remove(&id)?;
        if let Some(sid) = &entry.sid {
            self.by_sid.remove(sid);
        }
        Some(entry.payload)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = SessionId> + '_ {
        self.entries.keys().copied()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (SessionId, &P)> + '_ {
        self.entries.iter().map(|(id, e)| (*id, &e.payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invariants are payload-independent, so a trivial stub stands in for the
    // (gpui-bound) AgentSession.
    type Store = SessionStore<u32>;

    #[test]
    fn open_or_focus_is_idempotent_per_sid() {
        let mut s = Store::new();
        let first = s.open_or_focus("S1", |_| 10);
        let second = s.open_or_focus("S1", |_| 999); // make MUST NOT run
        assert!(matches!(first, Bind::Created(_)));
        assert_eq!(second, Bind::AlreadyOpen(first.id()));
        assert_eq!(s.len(), 1, "one sid → exactly one session (INV-1)");
        assert_eq!(
            s.get(first.id()),
            Some(&10),
            "second open did not overwrite"
        );
    }

    #[test]
    fn distinct_sids_get_distinct_sessions() {
        let mut s = Store::new();
        let a = s.open_or_focus("A", |_| 1).id();
        let b = s.open_or_focus("B", |_| 2).id();
        assert_ne!(a, b);
        assert_eq!(s.len(), 2);
        assert_eq!(s.locate("A"), Some(a));
        assert_eq!(s.locate("B"), Some(b));
    }

    #[test]
    fn bind_sid_rejects_a_second_owner() {
        let mut s = Store::new();
        let a = s.create_local(|_| 1);
        let b = s.create_local(|_| 2);
        assert!(s.bind_sid(a, "S".into()).is_ok());
        // Binding the same sid to a different session is refused with the owner.
        assert_eq!(s.bind_sid(b, "S".into()), Err(AlreadyBound(a)));
        // Re-binding the same pairing is a no-op success (idempotent).
        assert!(s.bind_sid(a, "S".into()).is_ok());
        assert_eq!(s.locate("S"), Some(a));
        assert_eq!(s.sid_of(b), None, "b never got the sid");
    }

    #[test]
    fn close_frees_the_sid_for_reattach() {
        let mut s = Store::new();
        let a = s.open_or_focus("S", |_| 1).id();
        assert_eq!(s.locate("S"), Some(a));
        let payload = s.close(a);
        assert_eq!(payload, Some(1));
        assert_eq!(s.locate("S"), None, "sid released on close");
        assert!(s.is_empty());
        // A fresh open for the same sid mints a NEW id (no reuse).
        let b = s.open_or_focus("S", |_| 2).id();
        assert_ne!(a, b);
    }

    #[test]
    fn local_session_has_no_sid_until_bound() {
        let mut s = Store::new();
        let id = s.create_local(|_| 7);
        assert_eq!(s.sid_of(id), None);
        assert_eq!(s.locate("X"), None);
        s.bind_sid(id, "X".into()).unwrap();
        assert_eq!(s.sid_of(id), Some("X"));
        assert_eq!(s.get_by_sid_mut("X"), Some(&mut 7));
    }

    #[test]
    fn bind_sid_replaces_a_sessions_prior_sid() {
        // Re-binding the SAME session to a new sid releases the old one so
        // `by_sid` stays total (the sid-replacement branch of `bind_sid`).
        let mut s = Store::new();
        let id = s.create_local(|_| 1);
        s.bind_sid(id, "A".into()).unwrap();
        s.bind_sid(id, "B".into()).unwrap();
        assert_eq!(s.locate("A"), None, "old sid released");
        assert_eq!(s.locate("B"), Some(id), "new sid routes");
        assert_eq!(s.sid_of(id), Some("B"));
    }

    #[test]
    fn clear_sid_keeps_the_session_but_frees_routing() {
        // `clear_sid` drops only the sid binding; the session payload and id
        // survive so a respawn can re-bind without losing transcript state.
        let mut s = Store::new();
        let id = s.open_or_focus("S", |_| 9).id();
        assert_eq!(s.clear_sid(id), Some("S".to_string()));
        assert_eq!(s.locate("S"), None, "sid no longer routable");
        assert_eq!(s.sid_of(id), None, "session carries no sid");
        assert!(s.contains(id), "session itself survives");
        assert_eq!(s.get(id), Some(&9), "payload preserved");
        // Re-bind works afterward.
        s.bind_sid(id, "S2".into()).unwrap();
        assert_eq!(s.locate("S2"), Some(id));
    }
}
