//! External chat bridge (spec-external-chat-bridge.md).
//!
//! An in-process task inside `yalda-session-server` that mirrors agent sessions
//! into an external chat app (Telegram first) as **one forum topic per
//! session**. It taps the server's own event stream and drives sessions through
//! the same ungated `admin_prompt` path (ADR-0015) — so a phone and the desktop
//! GUI are peers over one session with one durable source of truth.
//!
//! Layering (all decoupled from the Manager internals so it's unit-testable):
//! - [`transport`] — the `ChatTransport` seam + `FakeTransport` for tests.
//! - [`router`] — the pure `session_id ⇄ ThreadId` map + lifecycle decisions.
//! - this module — config, the `SessionDriver` seam, the `BridgeEvent` stream,
//!   and the async task that wires them together.
//!
//! The bridge is spawned only when configured (a Telegram token is present);
//! absent config it costs nothing and exposes no surface.

mod command;
mod fold;
mod router;
mod telegram;
mod transport;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use yalda::acp_channel::{AgentProvider, PermissionMode};
use yalda::session_proto::{Notification, ServerSessionId, SessionInfo};

use command::Command;
use fold::{ChatOp, EventFolder};
use router::{TopicAction, TopicMapSnapshot, TopicRouter};
use transport::{ChatTransport, MessageId, ThreadId};

#[cfg(test)]
mod tests;

// ── Config ──────────────────────────────────────────────────────────

/// Resolved bridge configuration. Constructed only when a Telegram token is
/// present; an empty/absent token disables the bridge entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub token: String,
    /// The forum supergroup the bot posts into (spec §4a).
    pub chat_id: i64,
    /// Allow-listed sender ids. **Non-empty is mandatory** — a bridge with no
    /// allowlist is a wide-open remote-code-execution surface (spec §7), so we
    /// refuse to start one.
    pub allowed_user_ids: Vec<i64>,
    /// Working directory for sessions created via `/new` with no explicit path.
    pub default_cwd: PathBuf,
}

impl BridgeConfig {
    /// Load from the environment first, then `~/.yalda/bridge.json`. Returns:
    /// - `Ok(None)` — no token ⇒ bridge disabled (the common case).
    /// - `Ok(Some(cfg))` — a valid, allow-listed config.
    /// - `Err(msg)` — a token was configured but the rest is invalid (e.g. no
    ///   allowlist, no chat id); we surface it loudly rather than silently
    ///   running an unsafe or broken bridge.
    pub fn load() -> Result<Option<BridgeConfig>, String> {
        let file = load_config_file();
        let token = env_str("YALDA_TELEGRAM_TOKEN").or(file.token);
        let chat_id = env_i64("YALDA_TELEGRAM_CHAT_ID")?.or(file.chat_id);
        let allowed = match env_id_list("YALDA_TELEGRAM_ALLOWED_IDS")? {
            Some(v) => v,
            None => file.allowed_user_ids.unwrap_or_default(),
        };
        let default_cwd = env_str("YALDA_BRIDGE_DEFAULT_CWD")
            .map(PathBuf::from)
            .or(file.default_cwd.map(PathBuf::from));
        build_config(token, chat_id, allowed, default_cwd)
    }
}

/// Pure config-assembly rules, split out so they're unit-testable without env
/// or filesystem. Encodes the safety gates from spec §7.
fn build_config(
    token: Option<String>,
    chat_id: Option<i64>,
    allowed_user_ids: Vec<i64>,
    default_cwd: Option<PathBuf>,
) -> Result<Option<BridgeConfig>, String> {
    let token = match token.map(|t| t.trim().to_string()) {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(None), // no token ⇒ disabled
    };
    if allowed_user_ids.is_empty() {
        return Err(
            "bridge token set but no allowed_user_ids — refusing to start an \
             un-allowlisted bridge (it would let anyone drive agents; spec §7)"
                .to_string(),
        );
    }
    let chat_id = chat_id.ok_or_else(|| {
        "bridge token set but no chat_id — set YALDA_TELEGRAM_CHAT_ID or bridge.json".to_string()
    })?;
    let default_cwd = default_cwd
        .or_else(|| yalda::paths::yalda_home())
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Some(BridgeConfig {
        token,
        chat_id,
        allowed_user_ids,
        default_cwd,
    }))
}

/// Raw config as read from `bridge.json` (all optional; env can supply/override).
#[derive(Debug, Default, serde::Deserialize)]
struct FileConfig {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    allowed_user_ids: Option<Vec<i64>>,
    #[serde(default)]
    default_cwd: Option<String>,
}

fn load_config_file() -> FileConfig {
    let Some(path) = bridge_config_path() else {
        return FileConfig::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!(path = %path.display(), error = %e, "bad bridge.json — ignoring");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(), // absent file is normal
    }
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn env_i64(key: &str) -> Result<Option<i64>, String> {
    match env_str(key) {
        Some(s) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| format!("{key} must be an integer, got {s:?}")),
        None => Ok(None),
    }
}

fn env_id_list(key: &str) -> Result<Option<Vec<i64>>, String> {
    match env_str(key) {
        Some(s) => {
            let mut ids = Vec::new();
            for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                ids.push(
                    part.parse::<i64>()
                        .map_err(|_| format!("{key} entry {part:?} is not an integer"))?,
                );
            }
            Ok(Some(ids))
        }
        None => Ok(None),
    }
}

// ── Persistence paths (honor the test-isolation seam) ───────────────

/// `bridge.json` location: next to the socket when `YALDA_SESSION_SOCKET` is
/// overridden (tests / alternate instances), else `~/.yalda/bridge.json`.
/// `None` under `cfg(test)` so a unit test never reads the real user config.
fn bridge_config_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    if std::env::var_os("YALDA_SESSION_SOCKET").is_some() {
        return Some(yalda::session_proto::socket_path().with_extension("bridge.json"));
    }
    yalda::paths::yalda_home().map(|d| d.join("bridge.json"))
}

/// Persisted `session_id ⇄ thread_id` map (spec §8). Same seam as above.
fn topic_map_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    if std::env::var_os("YALDA_SESSION_SOCKET").is_some() {
        return Some(yalda::session_proto::socket_path().with_extension("bridge-topics.json"));
    }
    yalda::paths::yalda_home().map(|d| d.join("bridge_topics.json"))
}

fn load_topic_map() -> TopicRouter {
    let Some(path) = topic_map_path() else {
        return TopicRouter::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<TopicMapSnapshot>(&s) {
            Ok(snap) => TopicRouter::from_snapshot(&snap),
            Err(e) => {
                tracing::warn!(error = %e, "bad bridge_topics.json — starting fresh");
                TopicRouter::new()
            }
        },
        Err(_) => TopicRouter::new(),
    }
}

fn persist_topic_map(router: &TopicRouter) {
    let Some(path) = topic_map_path() else {
        return;
    };
    if let Ok(json) = serde_json::to_string_pretty(&router.snapshot()) {
        if let Err(e) = std::fs::write(&path, json) {
            tracing::warn!(error = %e, "failed to persist bridge topic map");
        }
    }
}

// ── SessionDriver seam ──────────────────────────────────────────────

/// The subset of session-server operations the bridge drives. A trait (rather
/// than a direct `Arc<SessionManager>`) so the bridge task is unit-testable
/// with a fake in-process driver — no real actor, socket, or agent needed.
pub trait SessionDriver: Send + Sync + 'static {
    /// Create a new session (driven by `/new`); its topic auto-opens off the
    /// resulting `SessionCreated` broadcast.
    fn create(&self, label: String, cwd: PathBuf)
    -> impl Future<Output = SessionInfo> + Send;
    /// Ungated enqueue (ADR-0015 `admin_prompt`): drive a turn without owning
    /// the session, so we never fight an attached GUI for the tile.
    fn admin_prompt(
        &self,
        sid: String,
        text: String,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// Cancel the session's in-flight turn (driven by `/stop`).
    fn cancel(&self, sid: String) -> impl Future<Output = Result<(), String>> + Send;
    /// Set the session's permission mode (driven by `/mode` and the `/new`
    /// read-only fail-safe).
    fn set_permission_mode(
        &self,
        sid: String,
        mode: PermissionMode,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn list(&self) -> impl Future<Output = Vec<SessionInfo>> + Send;
}

impl SessionDriver for Arc<crate::SessionManager> {
    async fn create(&self, label: String, cwd: PathBuf) -> SessionInfo {
        self.send_create(cwd, label, AgentProvider::Claude, None)
            .await
    }
    async fn admin_prompt(&self, sid: String, text: String) -> Result<(), String> {
        self.send_admin_prompt(&sid, &text).await
    }
    async fn cancel(&self, sid: String) -> Result<(), String> {
        self.send_cancel(&sid).await
    }
    async fn set_permission_mode(&self, sid: String, mode: PermissionMode) -> Result<(), String> {
        self.send_set_permission_mode(&sid, mode).await
    }
    async fn list(&self) -> Vec<SessionInfo> {
        self.send_list_sessions().await
    }
}

// ── Event stream ────────────────────────────────────────────────────

/// The unified event stream the bridge consumes. Session-list events come from
/// the manager broadcast; transcript events (T-004) come from the `push_event`
/// tap. Keeping one enum lets the task `select!` over a single channel.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    SessionCreated(SessionInfo),
    SessionClosed(ServerSessionId),
    SessionRenamed {
        session_id: ServerSessionId,
        label: String,
    },
    /// A per-session transcript notification (spec §5 fold), tapped from the
    /// manager's `push_event` chokepoint (T-004) and folded into the session's
    /// topic.
    Transcript {
        session_id: ServerSessionId,
        note: Box<Notification>,
    },
}

/// Translate the manager-level broadcast (`subscribe_events`) into the bridge's
/// event stream. Runs as its own small task so the bridge sees create/close/
/// rename without touching Manager internals.
async fn forward_manager_events(
    mut rx: broadcast::Receiver<Notification>,
    tx: mpsc::UnboundedSender<BridgeEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(note) => {
                let ev = match note {
                    Notification::SessionCreated { session } => {
                        Some(BridgeEvent::SessionCreated(session))
                    }
                    Notification::SessionClosed { session_id } => {
                        Some(BridgeEvent::SessionClosed(session_id))
                    }
                    Notification::SessionRenamed { session_id, label } => {
                        Some(BridgeEvent::SessionRenamed { session_id, label })
                    }
                    _ => None,
                };
                if let Some(ev) = ev {
                    if tx.send(ev).is_err() {
                        return; // bridge gone
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "bridge lagged manager broadcast");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

// ── The bridge task ─────────────────────────────────────────────────

/// How often to poll the transport for inbound messages.
fn inbound_poll_interval() -> Duration {
    Duration::from_millis(700)
}

/// The bridge event loop: reconcile topics on startup, then react to session
/// lifecycle (open/close/rename topics) and inbound chat (drive turns). Generic
/// over transport + driver so tests inject fakes.
///
/// Inbound polling runs on its OWN task feeding `inbound_rx`, so a long-poll
/// (`getUpdates` blocks server-side) can never stall topic-lifecycle handling.
async fn run_bridge<T: ChatTransport + Clone, D: SessionDriver>(
    config: BridgeConfig,
    transport: T,
    driver: D,
    mut events: mpsc::UnboundedReceiver<BridgeEvent>,
) {
    let mut router = load_topic_map();

    // Startup reconciliation: open a topic for any live session lacking one,
    // close any topic whose session is gone (spec §8).
    let live: Vec<(ServerSessionId, String)> = driver
        .list()
        .await
        .into_iter()
        .map(|i| (i.session_id, i.label))
        .collect();
    for action in router.reconcile(&live) {
        apply_topic_action(&transport, &mut router, action).await;
    }
    persist_topic_map(&router);

    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel();
    tokio::spawn(poll_inbound_loop(transport.clone(), inbound_tx));

    // Outbound fold state (T-004): one `EventFolder` per session, and the running
    // (live-edited) message id per topic. Owned here so the fold is per-session
    // stateful across the event stream without a lock.
    let mut folders: HashMap<ServerSessionId, EventFolder> = HashMap::new();
    let mut running: HashMap<ThreadId, MessageId> = HashMap::new();

    loop {
        tokio::select! {
            maybe_ev = events.recv() => {
                let Some(ev) = maybe_ev else { return; };
                handle_event(&transport, &mut router, &mut folders, &mut running, ev).await;
            }
            maybe_in = inbound_rx.recv() => {
                let Some(msg) = maybe_in else { continue; };
                handle_inbound(&config, &transport, &driver, &router, msg).await;
            }
        }
    }
}

/// Poll the transport for inbound messages and forward them into the bridge
/// loop. Its own task so a blocking long-poll doesn't stall lifecycle events.
async fn poll_inbound_loop<T: ChatTransport>(
    transport: T,
    tx: mpsc::UnboundedSender<transport::InboundMsg>,
) {
    let mut poll = tokio::time::interval(inbound_poll_interval());
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        match transport.poll_inbound().await {
            Ok(msgs) => {
                for msg in msgs {
                    if tx.send(msg).is_err() {
                        return; // bridge gone
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "bridge poll_inbound failed"),
        }
    }
}

/// React to a session-lifecycle event by driving topic ops, or fold a
/// transcript event into the session's topic (T-004).
async fn handle_event<T: ChatTransport>(
    transport: &T,
    router: &mut TopicRouter,
    folders: &mut HashMap<ServerSessionId, EventFolder>,
    running: &mut HashMap<ThreadId, MessageId>,
    ev: BridgeEvent,
) {
    // Transcript events drive no topic-lifecycle op — they fold straight into
    // the bound topic — so handle them on their own path.
    if let BridgeEvent::Transcript { session_id, note } = &ev {
        handle_transcript(transport, router, folders, running, session_id, note).await;
        return;
    }

    let action = match &ev {
        BridgeEvent::SessionCreated(info) => {
            router.on_session_created(&info.session_id, &info.label)
        }
        BridgeEvent::SessionClosed(sid) => router.on_session_closed(sid),
        BridgeEvent::SessionRenamed { session_id, label } => {
            router.on_session_renamed(session_id, label)
        }
        BridgeEvent::Transcript { .. } => unreachable!("handled above"),
    };

    // A closed session's fold state must not leak — drop its folder and any
    // running message (reading the thread BEFORE the close unbinds it).
    if let BridgeEvent::SessionClosed(sid) = &ev {
        folders.remove(sid);
        if let Some(thread) = router.thread_of(sid) {
            running.remove(&thread);
        }
    }

    if !matches!(action, TopicAction::Noop) {
        apply_topic_action(transport, router, action).await;
        persist_topic_map(router);
    }
}

/// Fold one transcript notification into its session's topic (spec §5). Only
/// `Notification::Agent` facts drive the fold — the legacy
/// `ReplyEvent`/`TurnEnded`/`UserPrompt` variants are ignored so a turn is never
/// double-rendered. Transport errors are logged and swallowed (never panic).
async fn handle_transcript<T: ChatTransport>(
    transport: &T,
    router: &TopicRouter,
    folders: &mut HashMap<ServerSessionId, EventFolder>,
    running: &mut HashMap<ThreadId, MessageId>,
    session_id: &ServerSessionId,
    note: &Notification,
) {
    // Only a bound session has a topic to fold into.
    let Some(thread) = router.thread_of(session_id) else {
        return;
    };
    // Only the canonical agent stream carries fold facts.
    let Notification::Agent { event } = note else {
        return;
    };

    let folder = folders.entry(session_id.clone()).or_default();
    for op in folder.on_event(&event.kind) {
        match op {
            ChatOp::Post(text) => match transport.send(thread, &text).await {
                Ok(id) => {
                    running.insert(thread, id);
                }
                Err(e) => tracing::warn!(error = %e, "bridge fold send failed"),
            },
            ChatOp::Edit(text) => {
                if let Some(id) = running.get(&thread).copied() {
                    if let Err(e) = transport.edit(thread, id, &text).await {
                        tracing::warn!(error = %e, "bridge fold edit failed");
                    }
                }
            }
            ChatOp::Finalize => {
                running.remove(&thread);
            }
        }
    }
}

/// Execute a [`TopicAction`] against the transport and record the resulting
/// binding in the router.
async fn apply_topic_action<T: ChatTransport>(
    transport: &T,
    router: &mut TopicRouter,
    action: TopicAction,
) {
    match action {
        TopicAction::Open { session, name } => match transport.open_thread(&name).await {
            Ok(thread) => router.bind(session, thread),
            Err(e) => tracing::warn!(error = %e, "open_thread failed"),
        },
        TopicAction::Close { session, thread } => {
            if let Err(e) = transport.close_thread(thread).await {
                tracing::warn!(error = %e, "close_thread failed");
            }
            router.unbind(&session);
        }
        TopicAction::Rename { thread, name } => {
            if let Err(e) = transport.rename_thread(thread, &name).await {
                tracing::warn!(error = %e, "rename_thread failed");
            }
        }
        TopicAction::Noop => {}
    }
}

/// Handle one inbound chat message: allowlist gate FIRST (spec §7), then parse
/// the slash-command grammar and dispatch it against the locale — the thread it
/// arrived in *is* the addressing. The General topic (mapped to no session)
/// takes control commands (`/new`, `/sessions`); a session topic takes the
/// per-session commands (`/stop`, `/mode`, `/status`) and plain-text injection.
async fn handle_inbound<T: ChatTransport, D: SessionDriver>(
    config: &BridgeConfig,
    transport: &T,
    driver: &D,
    router: &TopicRouter,
    msg: transport::InboundMsg,
) {
    // Security boundary (spec §7): a message from a non-allow-listed sender is
    // dropped silently — no reply, don't confirm the bot exists. This gate runs
    // BEFORE any parse/dispatch so a stranger can never reach a command.
    if !config.allowed_user_ids.contains(&msg.from_user) {
        return;
    }
    let text = msg.text.trim();
    if text.is_empty() {
        return;
    }

    let command = command::parse(text);
    match router.session_of(msg.thread) {
        // In a session's own topic: per-session commands + plain injection.
        Some(session) => {
            let session = session.clone();
            handle_session_command(transport, driver, msg.thread, &session, command).await;
        }
        // The General topic (or any unmapped thread): control commands only.
        None => handle_general_command(config, transport, driver, router, msg.thread, command).await,
    }
}

/// The General topic is the control channel (spec §4): only `/new` and
/// `/sessions` are meaningful here. Everything else (per-session verbs, plain
/// text) gets the nudge toward the right gesture.
async fn handle_general_command<T: ChatTransport, D: SessionDriver>(
    config: &BridgeConfig,
    transport: &T,
    driver: &D,
    router: &TopicRouter,
    thread: ThreadId,
    command: Command,
) {
    match command {
        Command::New { label, cwd } => {
            let cwd = cwd.unwrap_or_else(|| config.default_cwd.clone());
            // The topic auto-opens off the SessionCreated broadcast — don't open
            // one here. New sessions start read-only as a §7 fail-safe.
            let info = driver.create(label, cwd).await;
            if let Err(e) = driver
                .set_permission_mode(info.session_id.clone(), PermissionMode::ReadOnly)
                .await
            {
                tracing::warn!(error = %e, "bridge set_permission_mode(ReadOnly) on /new failed");
            }
        }
        Command::Sessions => {
            let _ = transport.send(thread, &format_sessions(driver, router).await).await;
        }
        // Per-session verbs and plain text don't belong in General.
        _ => {
            let _ = transport
                .send(
                    thread,
                    "Send messages inside a session's topic, or use /new to start one.",
                )
                .await;
        }
    }
}

/// A session's own topic (spec §4, §6): `/stop`, `/mode`, `/status`, and plain
/// text injected as a turn. `/new` and `/sessions` here point back to General.
async fn handle_session_command<T: ChatTransport, D: SessionDriver>(
    transport: &T,
    driver: &D,
    thread: ThreadId,
    session: &str,
    command: Command,
) {
    match command {
        Command::Stop => {
            if let Err(e) = driver.cancel(session.to_string()).await {
                tracing::warn!(error = %e, "bridge cancel failed");
                let _ = transport.send(thread, &format!("⚠️ couldn't stop: {e}")).await;
            }
        }
        Command::Mode(mode) => {
            if let Err(e) = driver.set_permission_mode(session.to_string(), mode).await {
                tracing::warn!(error = %e, "bridge set_permission_mode failed");
                let _ = transport
                    .send(thread, &format!("⚠️ couldn't set mode: {e}"))
                    .await;
            }
        }
        Command::Status => {
            let _ = transport.send(thread, &format_status(driver, session).await).await;
        }
        Command::Message(text) => {
            // The §6 injection path: drive an ungated turn on the bound session.
            if let Err(e) = driver.admin_prompt(session.to_string(), text).await {
                tracing::warn!(error = %e, "bridge admin_prompt failed");
                let _ = transport
                    .send(thread, &format!("⚠️ couldn't send: {e}"))
                    .await;
            }
        }
        Command::New { .. } | Command::Sessions => {
            let _ = transport
                .send(thread, "Use the General topic for /new and /sessions.")
                .await;
        }
        Command::Unknown(cmd) => {
            let _ = transport
                .send(thread, &format!("Unknown command: {cmd}"))
                .await;
        }
    }
}

/// Format the `/sessions` roster: each live session's label + which topic it is
/// bound to (a recovery aid, spec §4). `router` supplies the topic mapping.
async fn format_sessions<D: SessionDriver>(driver: &D, router: &TopicRouter) -> String {
    let list = driver.list().await;
    if list.is_empty() {
        return "No active sessions.".to_string();
    }
    let mut out = String::from("Sessions:");
    for info in list {
        let topic = match router.thread_of(&info.session_id) {
            Some(t) => format!("topic {}", t.0),
            None => "no topic".to_string(),
        };
        out.push_str(&format!("\n• {} — {}", info.label, topic));
    }
    out
}

/// Format the `/status` line for one session: label, permission mode, turn
/// count, and whether the agent subprocess is currently connected.
async fn format_status<D: SessionDriver>(driver: &D, session: &str) -> String {
    match driver.list().await.into_iter().find(|i| i.session_id == session) {
        Some(info) => format!(
            "{} · mode {} · {} turns · {}",
            info.label,
            info.permission_mode.short_label(),
            info.turns,
            if info.connected { "connected" } else { "idle" },
        ),
        None => "Session not found.".to_string(),
    }
}

// ── Startup wiring (production) ─────────────────────────────────────

/// Spawn the bridge iff configured. Returns immediately; the bridge and its
/// event-forwarder run as detached tasks. Absent config, does nothing. Called
/// from `main` after the manager actor is up.
///
/// The bridge task handles every transport error internally (logs + continues),
/// so it exits only when the server shuts down. A panic in it dies with the task
/// and leaves the server running (spec §6: a bridge hiccup can't wedge the
/// daemon). Backoff-restart-on-panic is a deferred hardening.
pub fn maybe_spawn_bridge(
    manager: Arc<crate::SessionManager>,
    transcript_rx: mpsc::UnboundedReceiver<(ServerSessionId, Notification)>,
) {
    let config = match BridgeConfig::load() {
        // Disabled — the common case. Dropping `transcript_rx` here makes the
        // Manager's per-session sends error immediately (no buffering), so the
        // tap costs nothing when the bridge is off.
        Ok(None) => return,
        Ok(Some(c)) => c,
        Err(e) => {
            tracing::error!(error = %e, "bridge configured but invalid — not starting");
            return;
        }
    };
    tracing::info!(
        chat_id = config.chat_id,
        allowed = config.allowed_user_ids.len(),
        "starting Telegram bridge"
    );

    // Both session-list events (manager broadcast) and per-session transcript
    // events (the `push_event` tap) fan into ONE mpsc so `run_bridge` selects
    // over a single channel.
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    tokio::spawn(forward_manager_events(
        manager.subscribe_events(),
        event_tx.clone(),
    ));
    tokio::spawn(forward_transcript_events(transcript_rx, event_tx));

    let transport = telegram::TelegramTransport::new(config.token.clone(), config.chat_id);
    tokio::spawn(run_bridge(config, transport, manager, event_rx));
}

/// Map each tapped `(session_id, note)` from the manager's `push_event`
/// chokepoint into a [`BridgeEvent::Transcript`] on the unified bridge stream.
/// Its own task so the tap never blocks the manager actor.
async fn forward_transcript_events(
    mut rx: mpsc::UnboundedReceiver<(ServerSessionId, Notification)>,
    tx: mpsc::UnboundedSender<BridgeEvent>,
) {
    while let Some((session_id, note)) = rx.recv().await {
        let ev = BridgeEvent::Transcript {
            session_id,
            note: Box::new(note),
        };
        if tx.send(ev).is_err() {
            return; // bridge gone
        }
    }
}
