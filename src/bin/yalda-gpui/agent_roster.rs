//! The **universal agent-session roster** (universal-agent-list;
//! spec-universal-agent-list.md). A live, read-only cache of EVERY session the
//! session-server knows about — including ones this GUI has never opened —
//! keyed by server sid.
//!
//! It is a mirror of server truth: seeded by `list_sessions` at connect and
//! kept live by the `SessionCreated` / `SessionClosed` / `SessionRenamed`
//! broadcasts the server already pushes (`apply_server_batch`, agent_ui.rs).
//! Both the jump panel's "Agent sessions" section and the per-tile session
//! selector render as **read-only projections** of this one roster, so a
//! rename / add / close / selection updates both at once (they re-render from
//! the same notified root state).
//!
//! The roster is distinct from the `AgentSessions` store
//! (`agent_sessions.rs`): the store holds the *live conversations* this GUI has
//! bound to a tile (`Entity<AgentSession>`, the transcript/channel); the roster
//! holds *metadata about every session that exists*. A session can be in the
//! roster but not the store (running elsewhere, never opened here) — that is
//! exactly the gap this closes.

use std::collections::BTreeMap;
use yalda::session_proto::SessionInfo;

/// Cache of all server-known sessions, keyed by server sid. Stable iteration is
/// by sid; display order is by label (`entries_by_label`).
#[derive(Default)]
pub(crate) struct AgentRoster {
    by_sid: BTreeMap<String, SessionInfo>,
}

impl AgentRoster {
    /// Insert or update one session's metadata (from `SessionCreated` or a
    /// `list_sessions` row). Returns `true` if anything actually changed.
    pub(crate) fn upsert(&mut self, info: SessionInfo) -> bool {
        match self.by_sid.get(&info.session_id) {
            Some(existing) if *existing == info => false,
            _ => {
                self.by_sid.insert(info.session_id.clone(), info);
                true
            }
        }
    }

    /// Drop a session (from `SessionClosed`). Returns `true` if it was present.
    pub(crate) fn remove(&mut self, sid: &str) -> bool {
        self.by_sid.remove(sid).is_some()
    }

    /// Update a session's in-flight flag (from the `SessionBusy` broadcast,
    /// bug-0022). Returns `true` when the session is known and the flag actually
    /// flipped — the caller notifies on `true` so the jump panel repaints. A
    /// broadcast for an unknown session is a no-op (the next `list_sessions`
    /// carries the current state).
    pub(crate) fn set_busy(&mut self, sid: &str, busy: bool) -> bool {
        match self.by_sid.get_mut(sid) {
            Some(info) if info.busy != busy => {
                info.busy = busy;
                true
            }
            _ => false,
        }
    }

    /// Update a session's label (from `SessionRenamed`). Returns `true` if the
    /// session is known and the label actually changed. A rename for a session
    /// not yet in the roster is a no-op here (the eventual `list_sessions` /
    /// `SessionCreated` carries the current label).
    pub(crate) fn rename(&mut self, sid: &str, label: &str) -> bool {
        match self.by_sid.get_mut(sid) {
            Some(info) if info.label != label => {
                info.label = label.to_string();
                true
            }
            _ => false,
        }
    }

    /// Replace the whole roster with a fresh `list_sessions` snapshot. Returns
    /// `true` if the contents changed (so the caller can decide to notify).
    pub(crate) fn replace_all(&mut self, sessions: Vec<SessionInfo>) -> bool {
        let mut next = BTreeMap::new();
        for s in sessions {
            next.insert(s.session_id.clone(), s);
        }
        let changed = next != self.by_sid;
        self.by_sid = next;
        changed
    }

    pub(crate) fn get(&self, sid: &str) -> Option<&SessionInfo> {
        self.by_sid.get(sid)
    }

    #[allow(dead_code)] // used by the Phase-2 selector projection
    pub(crate) fn is_empty(&self) -> bool {
        self.by_sid.is_empty()
    }

    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.by_sid.len()
    }

    /// All entries in stable display order (by label, sid as tiebreak).
    pub(crate) fn entries_by_label(&self) -> Vec<&SessionInfo> {
        let mut v: Vec<&SessionInfo> = self.by_sid.values().collect();
        v.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.session_id.cmp(&b.session_id)));
        v
    }
}
