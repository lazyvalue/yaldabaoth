//! The **universal agent-session roster** (universal-agent-list;
//! spec-universal-agent-list.md). A live, read-only cache of EVERY session the
//! session-server knows about — including ones this GUI has never opened —
//! keyed by server sid.
//!
//! It is a mirror of server truth: seeded by `list_sessions` at connect and
//! kept live by the `SessionCreated` / `SessionClosed` / `SessionRenamed` /
//! `SessionArchived`
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
pub(crate) struct AgentRoster {
    by_sid: BTreeMap<String, SessionInfo>,
    /// Runtime entry time for the roster's current busy/idle state. Local-open
    /// sessions carry richer timing in `AgentState`; this covers sessions the
    /// GUI has never attached to.
    state_since: BTreeMap<String, std::time::Instant>,
}

impl Default for AgentRoster {
    fn default() -> Self {
        Self {
            by_sid: BTreeMap::new(),
            state_since: BTreeMap::new(),
        }
    }
}

impl AgentRoster {
    /// Insert or update one session's metadata (from `SessionCreated` or a
    /// `list_sessions` row). Returns `true` if anything actually changed.
    pub(crate) fn upsert(&mut self, info: SessionInfo) -> bool {
        match self.by_sid.get(&info.session_id) {
            Some(existing) if *existing == info => false,
            _ => {
                let reset_state_time = self
                    .by_sid
                    .get(&info.session_id)
                    .is_none_or(|existing| existing.busy != info.busy);
                if reset_state_time {
                    self.state_since
                        .insert(info.session_id.clone(), std::time::Instant::now());
                }
                self.by_sid.insert(info.session_id.clone(), info);
                true
            }
        }
    }

    /// Drop a session (from `SessionClosed`). Returns `true` if it was present.
    pub(crate) fn remove(&mut self, sid: &str) -> bool {
        self.state_since.remove(sid);
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
                self.state_since
                    .insert(sid.to_string(), std::time::Instant::now());
                true
            }
            _ => false,
        }
    }

    /// Update a session's agent-subprocess liveness (from the `SessionConnected`
    /// broadcast, bug-0027). Returns `true` when the session is known and the
    /// flag actually flipped — the caller notifies on `true` so the jump panel
    /// repaints. Deliberately does NOT touch `state_since`: coming back online
    /// is not a Waiting/Working transition and must not reorder the state tabs.
    pub(crate) fn set_connected(&mut self, sid: &str, connected: bool) -> bool {
        match self.by_sid.get_mut(sid) {
            Some(info) if info.connected != connected => {
                info.connected = connected;
                true
            }
            _ => false,
        }
    }

    /// Update a session's server-authoritative cold-storage state. Archived
    /// sessions remain roster entries, but no longer own a live ACP transport.
    pub(crate) fn set_archived(&mut self, sid: &str, archived: bool) -> bool {
        match self.by_sid.get_mut(sid) {
            Some(info) if info.archived != archived => {
                info.archived = archived;
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
        let now = std::time::Instant::now();
        let mut next_state_since = BTreeMap::new();
        for s in sessions {
            let since = self
                .by_sid
                .get(&s.session_id)
                .filter(|old| old.busy == s.busy)
                .and_then(|_| self.state_since.get(&s.session_id))
                .copied()
                .unwrap_or(now);
            next_state_since.insert(s.session_id.clone(), since);
            next.insert(s.session_id.clone(), s);
        }
        let changed = next != self.by_sid;
        self.by_sid = next;
        self.state_since = next_state_since;
        changed
    }

    pub(crate) fn get(&self, sid: &str) -> Option<&SessionInfo> {
        self.by_sid.get(sid)
    }

    pub(crate) fn state_since(&self, sid: &str) -> Option<std::time::Instant> {
        self.state_since.get(sid).copied()
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
