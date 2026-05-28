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
    Attach { session_id: ServerSessionId },

    #[serde(rename = "detach")]
    Detach { session_id: ServerSessionId },

    #[serde(rename = "prompt")]
    Prompt {
        session_id: ServerSessionId,
        text: String,
    },

    #[serde(rename = "set_permission_mode")]
    SetPermissionMode {
        session_id: ServerSessionId,
        mode: PermissionMode,
    },

    #[serde(rename = "close_session")]
    CloseSession { session_id: ServerSessionId },
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
