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
use crate::agent_event::AgentEvent;

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

/// A write-ownership lease on a session (spec-session-server-actor §Phase 4).
///
/// Replaces the old per-connection `owner: conn_id` model. A lease grants drive
/// rights (prompt / cancel / restart / set-permission / close / promote) to a
/// *stable* `client_id` — a GUI install id that survives socket reconnect and
/// app restart — so a returning client resumes its lease with zero contention
/// (no `attach_owner_with_retry` race).
///
/// `expires_at_unix_ms` is **display/diagnostic only** on the wire: the server
/// drives expiry off a monotonic `tokio::time::Instant` (immune to NTP steps /
/// sleep-wake skew) and computes this wall-clock millis stamp purely so a GUI
/// can show "leased until …". It is NEVER fed back into an expiry comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lease {
    /// Stable GUI install id (UUID v4 string). The lease holder.
    pub client_id: String,
    /// Server wall-clock millis at which the lease expires. DISPLAY ONLY.
    pub expires_at_unix_ms: u64,
}

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

    #[serde(rename = "attach")]
    Attach {
        session_id: ServerSessionId,
        /// Owner (drives the session) or Observer (read-only mirror).
        mode: AttachMode,
        /// Stable client identity (spec phase 4). The server uses this to
        /// acquire / resume a lease: a same-`client_id` Owner attach always
        /// resumes (live → renew, expired → re-grant) with zero contention; a
        /// different live-leased `client_id` silently downgrades to Observer.
        /// Headless callers (ADR-0015) and pre-phase-4 clients send `""` /
        /// omit it — `#[serde(default)]` keeps the field purely additive and an
        /// empty `client_id` never acquires a lease.
        #[serde(default)]
        client_id: String,
        /// Cursor-based incremental reconnect (spec phase 5): the client's
        /// last-seen transcript position as `(generation, index)`, where
        /// `index` is the number of `event_log` entries already received on
        /// channel `generation`. When the cursor's generation matches the
        /// session's current `channel_generation` and the index is in range,
        /// the server streams ONLY the tail `[index..]` rather than the full
        /// log. Otherwise (None, generation mismatch, or out-of-range index)
        /// it falls back to today's behavior: a full replay from index 0.
        ///
        /// `#[serde(default)]` is what makes this purely additive — every
        /// existing client (incl. the GUI) and every pre-cursor persisted
        /// message deserializes with `cursor == None`, i.e. unchanged full
        /// replay.
        #[serde(default)]
        cursor: Option<(u64, u64)>,
    },

    /// Cleanly release a session (phase 4). When `client_id` matches the lease
    /// holder the lease is released IMMEDIATELY (clean blue-green handoff — no
    /// 15s TTL wait); a socket EOF, by contrast, leaves the lease to expire on
    /// its TTL so a fast same-`client_id` reconnect resumes with zero
    /// contention. Empty `client_id` only tears down the forwarder.
    #[serde(rename = "detach")]
    Detach {
        session_id: ServerSessionId,
        #[serde(default)]
        client_id: String,
    },

    /// Renew a held lease (spec phase 4). The GUI beats this every
    /// `HEARTBEAT_INTERVAL` for each session it drives; the server pushes the
    /// lease expiry forward by `LEASE_TTL`. The client drives the heartbeat
    /// because the socket is the server's only liveness signal — a lease with
    /// no client renewal would either never expire (breaking blue-green
    /// promote) or expire under a live owner (breaking ownership). A Heartbeat
    /// for a lease the caller no longer holds returns an error so the GUI
    /// re-attaches (resumes-or-observes).
    #[serde(rename = "heartbeat")]
    Heartbeat {
        session_id: ServerSessionId,
        client_id: String,
    },

    /// An attached `Observer` claims the lease on a session. Succeeds only when
    /// the session is currently unleased / expired (the previous holder cleanly
    /// detached or its lease lapsed). Used by a candidate GUI to take over after
    /// the old instance closes. Carries the promoter's `client_id` so the new
    /// lease records a stable holder (phase 4).
    #[serde(rename = "promote")]
    Promote {
        session_id: ServerSessionId,
        #[serde(default)]
        client_id: String,
    },

    #[serde(rename = "prompt")]
    Prompt {
        session_id: ServerSessionId,
        text: String,
        /// Lease holder identity (phase 4) — gated: only the lease holder may
        /// prompt. `#[serde(default)]` keeps it additive.
        #[serde(default)]
        client_id: String,
    },

    /// Interrupt the in-flight turn (ACP `session/cancel`). Lease-holder-only,
    /// like `Prompt`. The session stays alive; the current turn resolves
    /// with `StopReason::Cancelled`.
    #[serde(rename = "cancel")]
    Cancel {
        session_id: ServerSessionId,
        #[serde(default)]
        client_id: String,
    },

    /// Hard recovery: kill + respawn the agent subprocess, resuming the same
    /// ACP session. Lease-holder-only. The escalation when a graceful `Cancel`
    /// won't unstick a turn wedged on a hung upstream request.
    #[serde(rename = "restart_session")]
    RestartSession {
        session_id: ServerSessionId,
        #[serde(default)]
        client_id: String,
    },

    #[serde(rename = "set_permission_mode")]
    SetPermissionMode {
        session_id: ServerSessionId,
        mode: PermissionMode,
        #[serde(default)]
        client_id: String,
    },

    #[serde(rename = "close_session")]
    CloseSession {
        session_id: ServerSessionId,
        #[serde(default)]
        client_id: String,
    },

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
    /// Reply to a successful [`Request::Attach`] (spec phase 4). `driver` is
    /// `true` when this attach acquired/resumed the lease (full drive rights),
    /// `false` when it silently downgraded to Observer because a different live
    /// `client_id` holds the lease. The GUI sets its local role from this flag
    /// instead of inferring it from an "already own" error string (which the
    /// retired retry loop matched). Additive: it's a new `ResponseData`
    /// variant, so the older `Ack` reply for every other verb is unchanged.
    #[serde(rename = "attached")]
    Attached { driver: bool },
    #[serde(rename = "admin_status")]
    AdminStatus { snapshot: AdminSnapshot },
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

/// Diagnostic snapshot of the server's live session state (response to
/// [`Request::AdminStatus`]). Read-only; for observability/debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSnapshot {
    pub session_count: usize,
    pub sessions: Vec<AdminSessionInfo>,
}

/// Per-session live server-side state, richer than [`SessionInfo`] — exposes
/// internals (owner conn id, subscriber/receiver count, channel generation)
/// useful for diagnosing reconnect/ownership issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSessionInfo {
    pub session_id: ServerSessionId,
    pub label: String,
    /// Agent subprocess live (channel is Some).
    pub connected: bool,
    /// True when a non-expired lease is held (phase 4: `lease_holder` present
    /// and live). Convenience for callers that only care whether *someone*
    /// drives the session, mirroring the old `has_owner`.
    pub has_owner: bool,
    /// The current lease holder (spec phase 4), or `None` if unleased / expired.
    /// Replaces the old `owner_conn_id: Option<u64>` diagnostic — a stable
    /// `client_id` plus the (display-only) expiry is far more useful than an
    /// ephemeral connection id for monitoring blue-green handoff.
    pub lease_holder: Option<Lease>,
    pub turns: usize,
    pub event_log_len: usize,
    /// Lowest logical `seq` still resident in the in-memory `event_log`
    /// ringbuffer (spec-event-stream §6, phase-8 Stage B). `0` until a
    /// compaction trim advances it; a non-zero value means earlier events have
    /// been trimmed from memory (a `CompactedSummary` marker fronts the log).
    /// `#[serde(default)]` keeps it additive for pre-Stage-B admin clients.
    #[serde(default)]
    pub log_base: u64,
    /// Active broadcast receivers = attached connections (owner + observers).
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

    /// The write-ownership lease of a session changed (spec phase 4 — the
    /// breaking rename of the old `OwnerChanged`/`owner_changed`). Broadcast to
    /// all attached connections (lease holder + observers). `None` == unleased:
    /// an observer/candidate may `Promote`. `Some(lease)` == `lease.client_id`
    /// holds drive rights until `expires_at_unix_ms`. This is the signal a
    /// candidate GUI waits for after the previous holder cleanly exits or its
    /// lease expires. BREAKING: old `"owner_changed"` logs/clients cannot
    /// deserialize `"lease_changed"` and vice-versa — handled by the WAL v1→v2
    /// discard (no converter, per locked decision).
    #[serde(rename = "lease_changed")]
    LeaseChanged {
        session_id: ServerSessionId,
        lease: Option<Lease>,
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

/// PID file path — colocated with the socket. Derived from [`socket_path`] by
/// swapping the extension, so it honors `SKETCH_SESSION_SOCKET`: a server on a
/// custom socket gets its own PID file (and thus its own single-instance
/// guard) rather than sharing the default one. For the default socket this
/// resolves to `/tmp/sketch-session-$USER.pid`, unchanged.
pub fn pid_file_path() -> PathBuf {
    socket_path().with_extension("pid")
}

/// Path to the JSON file where the session server persists session metadata
/// across restarts. When the socket is overridden (tests, alternate
/// instances) the state file lives next to that socket so instances never
/// share persistence; otherwise it lives in the durable sketch home
/// (`~/.sketch`, ADR-0018) alongside other sketch state.
pub fn session_server_persist_path() -> Option<PathBuf> {
    if std::env::var_os("SKETCH_SESSION_SOCKET").is_some() {
        return Some(socket_path().with_extension("state.json"));
    }
    crate::paths::sketch_home().map(|d| d.join("session_server.json"))
}

/// Directory holding the durable per-session write-ahead logs (ADR-0009). Like
/// [`session_server_persist_path`], it follows `SKETCH_SESSION_SOCKET` so test
/// and alternate instances never share durable state; otherwise it lives in the
/// durable sketch home (`~/.sketch`, ADR-0018). `None` only if no home dir exists.
pub fn session_wal_dir() -> Option<PathBuf> {
    if std::env::var_os("SKETCH_SESSION_SOCKET").is_some() {
        return Some(socket_path().with_extension("wal"));
    }
    crate::paths::sketch_home().map(|d| d.join("wal"))
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

    /// Phase 4: `LeaseChanged` carries `Option<Lease>` and uses the
    /// `"lease_changed"` tag. Round-trip both the `None` (unleased) and
    /// `Some` (held) shapes.
    #[test]
    fn lease_changed_round_trips() {
        let none = Notification::LeaseChanged {
            session_id: "s1".into(),
            lease: None,
        };
        let json = serde_json::to_string(&none).unwrap();
        assert!(
            json.contains("\"lease_changed\""),
            "tag must be lease_changed: {json}"
        );
        assert!(matches!(
            serde_json::from_str::<Notification>(&json).unwrap(),
            Notification::LeaseChanged { lease: None, .. }
        ));

        let held = Notification::LeaseChanged {
            session_id: "s1".into(),
            lease: Some(Lease {
                client_id: "client-A".into(),
                expires_at_unix_ms: 123_456,
            }),
        };
        let json = serde_json::to_string(&held).unwrap();
        let back: Notification = serde_json::from_str(&json).unwrap();
        match back {
            Notification::LeaseChanged { lease: Some(l), .. } => {
                assert_eq!(l.client_id, "client-A");
                assert_eq!(l.expires_at_unix_ms, 123_456);
            }
            other => panic!("expected LeaseChanged Some, got {other:?}"),
        }
    }

    /// The breaking rename is documented by a failing deserialize: the old
    /// `"owner_changed"` discriminator no longer parses (no converter).
    #[test]
    fn old_owner_changed_tag_no_longer_deserializes() {
        let old = r#"{"type":"owner_changed","session_id":"s1","has_owner":true}"#;
        assert!(
            serde_json::from_str::<Notification>(old).is_err(),
            "the retired owner_changed tag must fail to deserialize on v2"
        );
    }
}
