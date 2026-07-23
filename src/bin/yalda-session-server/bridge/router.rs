//! `TopicRouter` — the bidirectional `session_id ⇄ ThreadId` map that makes
//! each agent session a Telegram forum topic (spec-external-chat-bridge.md §4,
//! §6, §8).
//!
//! Routing is by topic, not by a stateful "focused session": the thread an
//! inbound message arrives in *is* the session. The router is **pure** — it
//! answers lookups and computes what topic ops a session-lifecycle event
//! implies; the bridge task performs the async transport calls and records the
//! resulting `ThreadId`. That split keeps this unit-testable with no transport.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use yalda::session_proto::ServerSessionId;

use super::transport::ThreadId;

/// Bidirectional session⇄topic map. Persisted (as [`TopicMapSnapshot`]) so a
/// restart re-binds each session to its existing topic instead of orphaning
/// threads or opening duplicates.
#[derive(Default)]
pub struct TopicRouter {
    by_session: HashMap<ServerSessionId, ThreadId>,
    by_thread: HashMap<ThreadId, ServerSessionId>,
}

/// A serializable snapshot of the map for persistence.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopicMapSnapshot {
    /// `(session_id, thread_id)` pairs.
    pub pairs: Vec<(ServerSessionId, i64)>,
}

impl TopicRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild from a persisted snapshot.
    pub fn from_snapshot(snap: &TopicMapSnapshot) -> Self {
        let mut r = Self::new();
        for (sid, tid) in &snap.pairs {
            r.bind(sid.clone(), ThreadId(*tid));
        }
        r
    }

    /// Serialize the current map for persistence.
    pub fn snapshot(&self) -> TopicMapSnapshot {
        let mut pairs: Vec<(ServerSessionId, i64)> = self
            .by_session
            .iter()
            .map(|(sid, tid)| (sid.clone(), tid.0))
            .collect();
        // Stable order so the persisted file is deterministic (nicer diffs,
        // and deterministic tests).
        pairs.sort();
        TopicMapSnapshot { pairs }
    }

    /// Record a session⇄topic binding (both directions).
    pub fn bind(&mut self, session: ServerSessionId, thread: ThreadId) {
        self.by_session.insert(session.clone(), thread);
        self.by_thread.insert(thread, session);
    }

    /// Drop a session's binding (both directions). Returns its former thread.
    pub fn unbind(&mut self, session: &str) -> Option<ThreadId> {
        let thread = self.by_session.remove(session)?;
        self.by_thread.remove(&thread);
        Some(thread)
    }

    /// The topic for a session, if bound.
    pub fn thread_of(&self, session: &str) -> Option<ThreadId> {
        self.by_session.get(session).copied()
    }

    /// The session a topic is bound to, if any. `GENERAL` (and any unmapped
    /// topic) resolves to `None`.
    pub fn session_of(&self, thread: ThreadId) -> Option<&ServerSessionId> {
        self.by_thread.get(&thread)
    }

    pub fn is_bound(&self, session: &str) -> bool {
        self.by_session.contains_key(session)
    }

    /// All bound session ids (order unspecified).
    #[allow(dead_code)] // used by the T-005 `/sessions` command
    pub fn sessions(&self) -> impl Iterator<Item = &ServerSessionId> {
        self.by_session.keys()
    }

    /// All `(session, thread)` bindings, sorted for determinism.
    pub fn bindings(&self) -> Vec<(ServerSessionId, ThreadId)> {
        let mut v: Vec<_> = self
            .by_session
            .iter()
            .map(|(s, t)| (s.clone(), *t))
            .collect();
        v.sort();
        v
    }
}

/// The topic-lifecycle action a session-lifecycle event implies. The bridge
/// task turns these into transport calls; keeping the decision pure makes it
/// table-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicAction {
    /// Open a new topic named `name` for `session`, then `bind` the returned id.
    Open { session: ServerSessionId, name: String },
    /// Close the topic bound to `session`, then `unbind`.
    Close { session: ServerSessionId, thread: ThreadId },
    /// Rename the topic bound to `session`.
    Rename { thread: ThreadId, name: String },
    /// Nothing to do (e.g. a create for an already-bound session).
    Noop,
}

impl TopicRouter {
    /// A session was created (anywhere — GUI or `/new`): open a topic unless one
    /// already exists for it.
    pub fn on_session_created(&self, session: &str, label: &str) -> TopicAction {
        if self.is_bound(session) {
            TopicAction::Noop
        } else {
            TopicAction::Open {
                session: session.to_string(),
                name: topic_name(label),
            }
        }
    }

    /// A session was closed: close its topic if bound.
    pub fn on_session_closed(&self, session: &str) -> TopicAction {
        match self.thread_of(session) {
            Some(thread) => TopicAction::Close {
                session: session.to_string(),
                thread,
            },
            None => TopicAction::Noop,
        }
    }

    /// A session was renamed: rename its topic if bound.
    pub fn on_session_renamed(&self, session: &str, label: &str) -> TopicAction {
        match self.thread_of(session) {
            Some(thread) => TopicAction::Rename {
                thread,
                name: topic_name(label),
            },
            None => TopicAction::Noop,
        }
    }

    /// Startup reconciliation: given the set of live sessions (id, label), emit
    /// the ops that make the topic map match reality — open a topic for any live
    /// session that lacks one, close any topic whose session is gone. Pure; the
    /// caller executes and then re-binds/unbinds.
    pub fn reconcile(&self, live: &[(ServerSessionId, String)]) -> Vec<TopicAction> {
        let mut actions = Vec::new();
        let live_ids: std::collections::HashSet<&ServerSessionId> =
            live.iter().map(|(s, _)| s).collect();

        // Open a topic for any live session with no binding.
        for (sid, label) in live {
            if !self.is_bound(sid) {
                actions.push(TopicAction::Open {
                    session: sid.clone(),
                    name: topic_name(label),
                });
            }
        }
        // Close any bound topic whose session no longer exists.
        for (sid, thread) in self.bindings() {
            if !live_ids.contains(&sid) {
                actions.push(TopicAction::Close {
                    session: sid,
                    thread,
                });
            }
        }
        actions
    }
}

/// A topic name from a session label. Telegram forum topic names are 1–128
/// chars; fall back to a placeholder for an empty label and truncate long ones.
pub fn topic_name(label: &str) -> String {
    let trimmed = label.trim();
    let name = if trimmed.is_empty() { "agent" } else { trimmed };
    name.chars().take(128).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> ServerSessionId {
        s.to_string()
    }

    #[test]
    fn bind_and_resolve_both_directions() {
        let mut r = TopicRouter::new();
        r.bind(sid("s1"), ThreadId(11));
        assert_eq!(r.thread_of("s1"), Some(ThreadId(11)));
        assert_eq!(r.session_of(ThreadId(11)), Some(&sid("s1")));
        assert_eq!(r.session_of(ThreadId::GENERAL), None);
        assert_eq!(r.thread_of("nope"), None);
    }

    #[test]
    fn unbind_clears_both_directions() {
        let mut r = TopicRouter::new();
        r.bind(sid("s1"), ThreadId(11));
        assert_eq!(r.unbind("s1"), Some(ThreadId(11)));
        assert_eq!(r.thread_of("s1"), None);
        assert_eq!(r.session_of(ThreadId(11)), None);
        assert_eq!(r.unbind("s1"), None);
    }

    #[test]
    fn created_opens_once_then_noops() {
        let mut r = TopicRouter::new();
        match r.on_session_created("s1", "Build feature") {
            TopicAction::Open { session, name } => {
                assert_eq!(session, "s1");
                assert_eq!(name, "Build feature");
            }
            other => panic!("expected Open, got {other:?}"),
        }
        // After binding, a second create is a no-op (idempotent).
        r.bind(sid("s1"), ThreadId(1));
        assert_eq!(r.on_session_created("s1", "Build feature"), TopicAction::Noop);
    }

    #[test]
    fn closed_and_renamed_target_the_bound_thread() {
        let mut r = TopicRouter::new();
        r.bind(sid("s1"), ThreadId(7));
        assert_eq!(
            r.on_session_renamed("s1", "New name"),
            TopicAction::Rename {
                thread: ThreadId(7),
                name: "New name".to_string()
            }
        );
        assert_eq!(
            r.on_session_closed("s1"),
            TopicAction::Close {
                session: sid("s1"),
                thread: ThreadId(7)
            }
        );
        // Unbound session → nothing to do.
        assert_eq!(r.on_session_closed("ghost"), TopicAction::Noop);
        assert_eq!(r.on_session_renamed("ghost", "x"), TopicAction::Noop);
    }

    #[test]
    fn reconcile_opens_missing_and_closes_orphaned() {
        let mut r = TopicRouter::new();
        r.bind(sid("live"), ThreadId(1)); // still alive → left alone
        r.bind(sid("gone"), ThreadId(2)); // session vanished → close

        let live = vec![
            (sid("live"), "Live one".to_string()),
            (sid("fresh"), "Fresh one".to_string()), // alive, no topic → open
        ];
        let actions = r.reconcile(&live);

        assert!(actions.contains(&TopicAction::Open {
            session: sid("fresh"),
            name: "Fresh one".to_string()
        }));
        assert!(actions.contains(&TopicAction::Close {
            session: sid("gone"),
            thread: ThreadId(2)
        }));
        // "live" is untouched.
        assert!(!actions.iter().any(|a| matches!(
            a,
            TopicAction::Open { session, .. } | TopicAction::Close { session, .. } if session == "live"
        )));
    }

    #[test]
    fn snapshot_round_trips() {
        let mut r = TopicRouter::new();
        r.bind(sid("s2"), ThreadId(20));
        r.bind(sid("s1"), ThreadId(10));
        let snap = r.snapshot();
        // Deterministic (sorted) order.
        assert_eq!(
            snap.pairs,
            vec![(sid("s1"), 10), (sid("s2"), 20)]
        );
        let r2 = TopicRouter::from_snapshot(&snap);
        assert_eq!(r2.thread_of("s1"), Some(ThreadId(10)));
        assert_eq!(r2.thread_of("s2"), Some(ThreadId(20)));
    }

    #[test]
    fn topic_name_handles_empty_and_long() {
        assert_eq!(topic_name("  "), "agent");
        assert_eq!(topic_name("  hi  "), "hi");
        assert_eq!(topic_name(&"x".repeat(200)).chars().count(), 128);
    }
}
