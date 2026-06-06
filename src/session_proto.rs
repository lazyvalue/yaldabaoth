//! Wire protocol for `sketch-session-server` ↔ `sketch-gpui` communication.
//!
//! NDJSON (newline-delimited JSON) over a Unix domain socket. Each line is a
//! self-contained JSON object with a `"type"` discriminator.
//!
//! Two directions:
//! - **Request/Response** — GUI sends a `Request`, server replies with a
//!   `Response` carrying the same `id`.
//! - **Notification** — server pushes a `Notification` (no `id`, no reply).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::acp_channel::{PermissionMode, ReplyEvent};

/// Server-assigned stable session handle (UUID string).
pub type ServerSessionId = String;

/// How a GUI connection attaches to a session.
///
/// At most one `Owner` may be attached at a time (the connection allowed to
/// send prompts / change permission mode / close the session). Any number of
/// `Observer`s may attach concurrently — they receive the full replayed
/// transcript and the live event stream but cannot drive the session. This is
/// the basis for the blue-green build loop: a freshly built "candidate" GUI
/// attaches as an `Observer` to mirror live sessions, then `Promote`s to
/// `Owner` once the previous owner disconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    Owner,
    Observer,
}

// ── Envelope types ─────────────────────────────────────────────────

/// A framed message on the wire. Every line is one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Frame {
    #[serde(rename = "request")]
    Request { id: u64, req: Request },
    #[serde(rename = "response")]
    Response { id: u64, result: Response },
    #[serde(rename = "notification")]
    Notification { note: Notification },
}

// ── GUI → Server requests ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum Request {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "list_sessions")]
    ListSessions,

    #[serde(rename = "create_session")]
    CreateSession {
        cwd: PathBuf,
        label: String,
        /// Resume a prior ACP session id, if available.
        resume_session_id: Option<String>,
    },

    #[serde(rename = "attach")]
    Attach {
        session_id: ServerSessionId,
        /// Owner (drives the session) or Observer (read-only mirror).
        mode: AttachMode,
    },

    #[serde(rename = "detach")]
    Detach { session_id: ServerSessionId },

    /// An attached `Observer` claims ownership of a session. Succeeds only
    /// when the session currently has no owner (i.e. the previous owner
    /// disconnected). Used by a candidate GUI to take over after the old
    /// instance closes.
    #[serde(rename = "promote")]
    Promote { session_id: ServerSessionId },

    #[serde(rename = "prompt")]
    Prompt {
        session_id: ServerSessionId,
        text: String,
    },

    /// Interrupt the in-flight turn (ACP `session/cancel`). Owner-only,
    /// like `Prompt`. The session stays alive; the current turn resolves
    /// with `StopReason::Cancelled`.
    #[serde(rename = "cancel")]
    Cancel { session_id: ServerSessionId },

    /// Hard recovery: kill + respawn the agent subprocess, resuming the same
    /// ACP session. Owner-only. The escalation when a graceful `Cancel`
    /// won't unstick a turn wedged on a hung upstream request.
    #[serde(rename = "restart_session")]
    RestartSession { session_id: ServerSessionId },

    #[serde(rename = "set_permission_mode")]
    SetPermissionMode {
        session_id: ServerSessionId,
        mode: PermissionMode,
    },

    #[serde(rename = "close_session")]
    CloseSession { session_id: ServerSessionId },

    #[serde(rename = "rename_session")]
    RenameSession {
        session_id: ServerSessionId,
        label: String,
    },
}

// ── Server → GUI responses ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    #[serde(rename = "ok")]
    Ok { data: ResponseData },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseData {
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "sessions")]
    Sessions { sessions: Vec<SessionInfo> },
    #[serde(rename = "session")]
    Session { session: SessionInfo },
    #[serde(rename = "ack")]
    Ack,
}

/// Metadata about a managed session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: ServerSessionId,
    pub acp_session_id: Option<String>,
    pub label: String,
    pub cwd: PathBuf,
    pub turns: usize,
    pub connected: bool,
    pub permission_mode: PermissionMode,
    /// Whether some connection currently owns (can drive) this session.
    /// A candidate GUI uses this to decide whether it can `Promote`.
    pub has_owner: bool,
}

// ── Server → GUI notifications (no response expected) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Notification {
    /// Wraps a `ReplyEvent` from the agent. The GUI processes it the same
    /// way it currently processes events from `AcpChannelClient::try_recv`.
    #[serde(rename = "reply_event")]
    ReplyEvent {
        session_id: ServerSessionId,
        event: ReplyEvent,
    },

    /// Turn boundary — the agent's `session/prompt` response resolved.
    #[serde(rename = "turn_ended")]
    TurnEnded {
        session_id: ServerSessionId,
        turn_count: usize,
        /// The channel generation this boundary was produced on (bumps on
        /// every force-restart / reconnect, server-side `channel_generation`).
        /// Carried additively (A.8a) so a later pass (8b) can delete the
        /// turn-end *inference* in the pumps and instead trust this explicit
        /// signal, using `generation` to reject a replayed boundary from a
        /// channel that has since been superseded. `#[serde(default)]` keeps
        /// old persisted `event_log`s (written before this field existed)
        /// deserializable — they replay with `generation == 0`.
        #[serde(default)]
        generation: u64,
    },

    /// The user submitted a prompt. Logged so re-attaching GUIs can
    /// replay user turns alongside agent replies.
    #[serde(rename = "user_prompt")]
    UserPrompt {
        session_id: ServerSessionId,
        text: String,
    },

    /// Session handshake complete — agent subprocess is live.
    #[serde(rename = "session_attached")]
    SessionAttached {
        session_id: ServerSessionId,
        acp_session_id: Option<String>,
    },

    /// Agent subprocess died or was killed.
    #[serde(rename = "session_detached")]
    SessionDetached {
        session_id: ServerSessionId,
        reason: String,
    },

    /// Ownership of a session changed. Broadcast to all attached connections
    /// (owner and observers). When `has_owner` flips to `false`, an observer
    /// may `Promote` to claim the session — this is the signal a candidate
    /// GUI waits for after the previous owner closes.
    #[serde(rename = "owner_changed")]
    OwnerChanged {
        session_id: ServerSessionId,
        has_owner: bool,
    },

    /// A session was created. Broadcast to **every** connection (not just
    /// attached ones) over the manager-level channel so any GUI can keep its
    /// session list in sync without polling `list_sessions`.
    #[serde(rename = "session_created")]
    SessionCreated { session: SessionInfo },

    /// A session was closed/removed server-side. Broadcast to every
    /// connection so each GUI drops the matching slot from every panel,
    /// regardless of which connection initiated the close.
    #[serde(rename = "session_closed")]
    SessionClosed { session_id: ServerSessionId },

    /// A session's label changed. Broadcast to every connection so the new
    /// label propagates to every panel and every GUI instance.
    #[serde(rename = "session_renamed")]
    SessionRenamed {
        session_id: ServerSessionId,
        label: String,
    },
}

// ── Socket path helpers ────────────────────────────────────────────

/// Default socket path: `/tmp/sketch-session-$USER.sock`.
pub fn default_socket_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    PathBuf::from(format!("/tmp/sketch-session-{user}.sock"))
}

/// Resolved socket path, respecting `SKETCH_SESSION_SOCKET` override.
pub fn socket_path() -> PathBuf {
    std::env::var("SKETCH_SESSION_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_socket_path())
}

/// PID file path — colocated with the socket.
pub fn pid_file_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    PathBuf::from(format!("/tmp/sketch-session-{user}.pid"))
}

/// Path to the JSON file where the session server persists session metadata
/// across restarts. Lives in the cache dir alongside other sketch state.
pub fn session_server_persist_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("sketch").join("session_server.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A.8a back-compat: an `event_log` written before `generation` existed
    /// has no `generation` key. `#[serde(default)]` MUST let it deserialize
    /// (to `generation == 0`) so old persisted transcripts still replay.
    #[test]
    fn turn_ended_deserializes_without_generation() {
        let old = r#"{"type":"turn_ended","session_id":"s1","turn_count":3}"#;
        let note: Notification = serde_json::from_str(old).expect("old log must deserialize");
        match note {
            Notification::TurnEnded {
                session_id,
                turn_count,
                generation,
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(turn_count, 3);
                assert_eq!(generation, 0, "missing generation defaults to 0");
            }
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    /// New logs carry `generation` and it survives a round-trip.
    #[test]
    fn turn_ended_round_trips_generation() {
        let note = Notification::TurnEnded {
            session_id: "s1".to_string(),
            turn_count: 7,
            generation: 4,
        };
        let json = serde_json::to_string(&note).unwrap();
        assert!(json.contains("\"generation\":4"));
        let back: Notification = serde_json::from_str(&json).unwrap();
        match back {
            Notification::TurnEnded { generation, .. } => assert_eq!(generation, 4),
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }
}
