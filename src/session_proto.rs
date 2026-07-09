//! Wire protocol for `yalda-session-server` ↔ `yalda-gpui` communication.
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
use crate::agent_event::AgentEvent;

/// Server-assigned stable session handle (UUID string).
pub type ServerSessionId = String;

// ── Envelope types ─────────────────────────────────────────────────

/// A framed message on the wire. Every line is one of these.
// wire/event enum — boxing the large variant would ripple through serialization + every match site
#[allow(clippy::large_enum_variant)]
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

    /// Attach the single client to a session (strict 1:1: one tile per session,
    /// no mirroring). The server spawns exactly one forwarder per session to
    /// stream events to this client; a session shown in no tile is simply
    /// unattached (no forwarder), never "owned by someone else".
    #[serde(rename = "attach")]
    Attach {
        session_id: ServerSessionId,
        /// Cursor-based incremental reconnect: the client's last-seen transcript
        /// position as `(generation, index)`, where `index` is the number of
        /// `event_log` entries already received on channel `generation`. When
        /// the cursor's generation matches the session's current
        /// `channel_generation` and the index is in range, the server streams
        /// ONLY the tail `[index..]` rather than the full log. Otherwise (None,
        /// generation mismatch, or out-of-range index) it falls back to a full
        /// replay from index 0.
        ///
        /// `#[serde(default)]` keeps it additive — every pre-cursor persisted
        /// message deserializes with `cursor == None`, i.e. full replay.
        #[serde(default)]
        cursor: Option<(u64, u64)>,
    },

    /// Cleanly detach the single client from a session — tears down its
    /// forwarder. The session and its agent keep running with no client
    /// attached; a later `Attach` resumes from the durable `event_log`.
    #[serde(rename = "detach")]
    Detach { session_id: ServerSessionId },

    #[serde(rename = "prompt")]
    Prompt {
        session_id: ServerSessionId,
        text: String,
    },

    /// Interrupt the in-flight turn (ACP `session/cancel`). The session stays
    /// alive; the current turn resolves with `StopReason::Cancelled`.
    #[serde(rename = "cancel")]
    Cancel { session_id: ServerSessionId },

    /// Hard recovery: kill + respawn the agent subprocess, resuming the same
    /// ACP session. The escalation when a graceful `Cancel` won't unstick a
    /// turn wedged on a hung upstream request.
    #[serde(rename = "restart_session")]
    RestartSession { session_id: ServerSessionId },

    #[serde(rename = "set_permission_mode")]
    SetPermissionMode {
        session_id: ServerSessionId,
        mode: PermissionMode,
    },

    /// Switch the session's model. The server forwards `model_id` to the
    /// session's channel, which issues an ACP `session/set_config_option` for
    /// the `model` option. The updated selector comes back on the reply stream
    /// as `ReplyEvent::ModelChanged` + `ModelsAvailable`.
    #[serde(rename = "set_model")]
    SetModel {
        session_id: ServerSessionId,
        model_id: String,
    },

    #[serde(rename = "close_session")]
    CloseSession { session_id: ServerSessionId },

    #[serde(rename = "rename_session")]
    RenameSession {
        session_id: ServerSessionId,
        label: String,
    },

    /// Diagnostic snapshot of every managed session's live server-side state
    /// (ownership, subscribers, channel generation, etc.). Additive,
    /// read-only — no breaking changes to existing verbs.
    #[serde(rename = "admin_status")]
    AdminStatus,

    /// Headless "start-work" verb (ADR-0015): enqueue a prompt to an EXISTING
    /// session WITHOUT owning it. Unlike `Prompt` there is no owner gate — a
    /// non-GUI caller (CLI / cron / automation) can drive a turn on an unowned
    /// session and the agent runs it to completion with no GUI attached. It
    /// does NOT take a lease: the prompt is appended to the session's input
    /// queue (same WAL-durable path as `Prompt`) and the server drives the turn
    /// regardless of which connection sent it.
    #[serde(rename = "admin_prompt")]
    AdminPrompt {
        session_id: ServerSessionId,
        text: String,
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
    /// Reply to a successful [`Request::Attach`]. Strict 1:1: the single client
    /// is now attached and its forwarder is streaming the transcript. Carries
    /// no driver/owner flag — there is no ownership negotiation under the
    /// single-subscriber model.
    #[serde(rename = "attached")]
    Attached,
    #[serde(rename = "admin_status")]
    AdminStatus { snapshot: AdminSnapshot },
}

/// Metadata about a managed session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: ServerSessionId,
    pub acp_session_id: Option<String>,
    pub label: String,
    pub cwd: PathBuf,
    pub turns: usize,
    pub connected: bool,
    pub permission_mode: PermissionMode,
}

/// Diagnostic snapshot of the server's live session state (response to
/// [`Request::AdminStatus`]). Read-only; for observability/debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSnapshot {
    pub session_count: usize,
    pub sessions: Vec<AdminSessionInfo>,
}

/// Per-session live server-side state, richer than [`SessionInfo`] — exposes
/// internals (subscriber count, channel generation) useful for diagnosing
/// reconnect issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSessionInfo {
    pub session_id: ServerSessionId,
    pub label: String,
    /// Agent subprocess live (channel is Some).
    pub connected: bool,
    pub turns: usize,
    pub event_log_len: usize,
    /// Lowest logical `seq` still resident in the in-memory `event_log`
    /// ringbuffer (spec-event-stream §6). `0` until a compaction trim advances
    /// it; a non-zero value means earlier events have been trimmed from memory
    /// (a `CompactedSummary` marker fronts the log). `#[serde(default)]` keeps
    /// it additive for pre-trim admin clients.
    #[serde(default)]
    pub log_base: u64,
    /// Active broadcast receivers — `0` or `1` under strict 1:1 (the single
    /// attached client's forwarder, if any).
    pub subscriber_count: usize,
    pub channel_generation: u64,
    pub permission_mode: PermissionMode,
}

// ── Server → GUI notifications (no response expected) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Notification {
    /// Canonical agent fact (spec-event-stream §1) — phase 8 Stage A, ADDITIVE.
    /// Carries an [`AgentEvent`] with its self-attributing identity envelope
    /// `(session_id, generation, turn, seq)` INSIDE the event (spec §2), so the
    /// server never grafts identity at its hop. This variant is broadcast and
    /// WAL-persisted ALONGSIDE the legacy `ReplyEvent` / `TurnEnded` /
    /// `UserPrompt` variants during the additive rollout (spec §9); the legacy
    /// variants are deleted only after real-session soak. The GUI reducer folds
    /// the `Agent` stream while the existing inference still drives finalize,
    /// with the idempotent `(generation, turn)` guard as backstop.
    #[serde(rename = "agent")]
    Agent { event: AgentEvent },

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

    /// A session was created. Broadcast to **every** connection (not just
    /// attached ones) over the manager-level channel so any GUI can keep its
    /// session list in sync without polling `list_sessions`.
    #[serde(rename = "session_created")]
    SessionCreated { session: SessionInfo },

    /// A session was closed/removed server-side. Broadcast to every
    /// connection so each GUI drops the matching slot from every tile,
    /// regardless of which connection initiated the close.
    #[serde(rename = "session_closed")]
    SessionClosed { session_id: ServerSessionId },

    /// A session's label changed. Broadcast to every connection so the new
    /// label propagates to every tile and every GUI instance.
    #[serde(rename = "session_renamed")]
    SessionRenamed {
        session_id: ServerSessionId,
        label: String,
    },

    /// The server REFUSED a prompt (lease not held). Sent only to the
    /// connection that issued the prompt — transient, never recorded.
    ///
    /// Exists because the GUI's `prompt()` is deliberately fire-and-forget
    /// (a round-trip would park the paint thread), so the `Response::Error`
    /// for the request has no waiter and is dropped by the reader. Without
    /// this notification a rejected prompt was COMPLETELY invisible: the GUI
    /// had already rendered the optimistic echo, so the message looked sent
    /// while the server had silently discarded it.
    #[serde(rename = "prompt_rejected")]
    PromptRejected {
        session_id: ServerSessionId,
        reason: String,
        /// The rejected prompt text, so the GUI can offer it back (restore
        /// into the chatbox) instead of making the user retype it.
        text: String,
    },
}

// ── Socket path helpers ────────────────────────────────────────────

/// Default socket path: `/tmp/yalda-session-$USER.sock`.
pub fn default_socket_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    PathBuf::from(format!("/tmp/yalda-session-{user}.sock"))
}

/// Resolved socket path, respecting `YALDA_SESSION_SOCKET` override.
pub fn socket_path() -> PathBuf {
    std::env::var("YALDA_SESSION_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_socket_path())
}

/// PID file path — colocated with the socket. Derived from [`socket_path`] by
/// swapping the extension, so it honors `YALDA_SESSION_SOCKET`: a server on a
/// custom socket gets its own PID file (and thus its own single-instance
/// guard) rather than sharing the default one. For the default socket this
/// resolves to `/tmp/yalda-session-$USER.pid`, unchanged.
pub fn pid_file_path() -> PathBuf {
    socket_path().with_extension("pid")
}

/// Path to the JSON file where the session server persists session metadata
/// across restarts. When the socket is overridden (tests, alternate
/// instances) the state file lives next to that socket so instances never
/// share persistence; otherwise it lives in the durable yalda home
/// (`~/.yalda`, ADR-0018) alongside other yalda state.
pub fn session_server_persist_path() -> Option<PathBuf> {
    if std::env::var_os("YALDA_SESSION_SOCKET").is_some() {
        return Some(socket_path().with_extension("state.json"));
    }
    crate::paths::yalda_home().map(|d| d.join("session_server.json"))
}

/// Directory holding the durable per-session write-ahead logs (ADR-0009). Like
/// [`session_server_persist_path`], it follows `YALDA_SESSION_SOCKET` so test
/// and alternate instances never share durable state; otherwise it lives in the
/// durable yalda home (`~/.yalda`, ADR-0018). `None` only if no home dir exists.
pub fn session_wal_dir() -> Option<PathBuf> {
    if std::env::var_os("YALDA_SESSION_SOCKET").is_some() {
        return Some(socket_path().with_extension("wal"));
    }
    crate::paths::yalda_home().map(|d| d.join("wal"))
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

    /// Single-subscriber model: the retired lease/owner control notifications
    /// (`lease_changed`, the older `owner_changed`) are gone — neither tag
    /// deserializes. A persisted WAL carrying one is discarded on recovery.
    #[test]
    fn retired_lease_tags_no_longer_deserialize() {
        for tag in ["lease_changed", "owner_changed"] {
            let old = format!(r#"{{"type":"{tag}","session_id":"s1"}}"#);
            assert!(
                serde_json::from_str::<Notification>(&old).is_err(),
                "the retired {tag} tag must fail to deserialize"
            );
        }
    }
}
