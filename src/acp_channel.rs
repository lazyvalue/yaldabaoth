//! ACP (Agent Client Protocol) channel for sketch.
//!
//! This module is an alternative path to the existing
//! [`claude_channel`](crate::claude_channel) UNIX-socket integration. Where the
//! sketch-channel route requires Claude Code to be running and to have spawned
//! sketch-channel as an MCP server, the ACP route lets sketch *itself* spawn a
//! local agent subprocess and talk to it directly over JSON-RPC stdio (the
//! [Agent Client Protocol](https://agentclientprotocol.com/)). That means
//! sketch can ride the user's Claude Max subscription via the
//! `claude-agent-acp` (formerly `@zed-industries/claude-code-acp`) adapter
//! without ever touching an API key — Claude Code handles auth itself.
//!
//! ## Architecture
//!
//! The official `agent-client-protocol` crate is async/Tokio-based, but the
//! rest of sketch is sync (single-threaded `App::run` loop with
//! `crossterm::event::poll`). To bridge this without rewriting `app.rs`, this
//! module follows the same pattern as `claude_channel.rs`:
//!
//! 1. Spawn a dedicated background **worker thread** that owns a multi-thread
//!    Tokio runtime.
//! 2. Inside that runtime, spawn the agent subprocess and run the ACP
//!    `Client.builder().connect_with(...)` driver loop. The closure stays
//!    alive for the lifetime of the connection — when sketch's drop signal
//!    fires, the closure returns and the worker thread tears the runtime
//!    down.
//! 3. Communicate between sketch (sync) and the worker (async) via two
//!    `std::sync::mpsc` channels:
//!       - **outbound**: `(prompt: String) -> ()` — `App` pushes prompts here
//!         on `:claude-acp-send`. The worker async-loop drains this channel
//!         and forwards each prompt to the active ACP session.
//!       - **inbound**: `(reply_chunk: String) -> ()` — the worker pushes
//!         each `agent_message_chunk` text payload here as it arrives. `App`
//!         drains it on every tick (`pump_acp_replies`) and splices into
//!         the `*claude*` buffer via the same `append_to_claude_buffer` path
//!         used by the UNIX-socket variant.
//!
//! ## What's wired up vs. what's not
//!
//! - Initialize handshake, `session/new`, `session/prompt`, streaming
//!   `agent_message_chunk` — all working.
//! - Cancellation: dropping `AcpChannelClient` signals the worker to exit,
//!   which kills the child process. There is also an explicit `detach()`.
//! - **Permission requests**: the agent may ask the client to approve tool
//!   use (`session/request_permission`). For now we auto-decline so the agent
//!   can't surprise-write files; future work could surface a prompt in the
//!   sketch UI. The agent can still respond with text (its own commentary)
//!   even without tool permission.
//! - **Tool calls / thoughts / plans**: ignored at this layer — only
//!   `agent_message_chunk` text is plumbed back to the buffer. Extending this
//!   to render tool calls inline would mean picking a richer reply format and
//!   teaching `append_to_claude_buffer` to handle it.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread::{self, JoinHandle};

use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PermissionOptionKind, ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionNotification,
    SessionUpdate,
};
// Re-exported via this module so consumers (App / GPUI) don't need a
// direct dependency on the agent-client-protocol schema crate just to
// match on tool-call events. `pub use` also brings these into local
// scope, so anything below this line can refer to them unqualified.
pub use agent_client_protocol::schema::{
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Emit a diagnostic line to stderr when SKETCH_ACP_DEBUG is set in the env.
/// Gated this way so the chatter doesn't corrupt sketch's TUI in normal use
/// but is one env-var away when something looks wrong.
macro_rules! acp_debug {
    ($($arg:tt)*) => {
        if std::env::var("SKETCH_ACP_DEBUG").is_ok() {
            eprintln!("[sketch-acp] {}", format_args!($($arg)*));
        }
    };
}

/// Default agent command, kept for backwards compatibility (e.g. callers
/// that want to display "the default" somewhere). Real spawning uses
/// [`DEFAULT_AGENT_FALLBACKS`] so users on either binary name still work.
pub const DEFAULT_AGENT_COMMAND: &str = "claude-code-acp";

/// Which sketch frontend is hosting this ACP session. Threaded into the
/// system-prompt append so Claude knows whether it's running inside the
/// terminal TUI or the GPUI desktop app — affects nothing protocol-side,
/// only the host-description sentence at the top of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchFrontend {
    /// Terminal frontend (ratatui + crossterm). The default — preserves
    /// existing behaviour for any caller that didn't opt in to the
    /// frontend-aware spawn variants.
    Tui,
    /// Desktop frontend (GPUI). Selected by `sketch-gpui`.
    Gpui,
}

impl Default for SketchFrontend {
    fn default() -> Self {
        Self::Tui
    }
}

impl SketchFrontend {
    /// Sentence describing the host — interpolated into the system-prompt
    /// append so the model can adapt phrasing if it cares (most behaviour
    /// is identical between the two).
    fn host_description(self) -> &'static str {
        match self {
            Self::Tui => "the ratatui/crossterm terminal frontend (`sketch` binary)",
            Self::Gpui => "the GPUI desktop frontend (`sketch-gpui` binary)",
        }
    }
}

/// Events drained by the App from the ACP worker. Replaces the previous
/// "stream of text chunks" channel so we can also report tool-call
/// activity (announcements + status/output updates) in chronological
/// order — that order is what makes inline tool-call rendering match what
/// the model actually did.
#[derive(Debug, Clone)]
pub enum ReplyEvent {
    /// Streamed text from `AgentMessageChunk`. Splice into the *claude*
    /// buffer the same way as before.
    Chunk(String),
    /// New tool call announced by the agent. The App stores it keyed by
    /// `tool_call_id`; later `ToolCallUpdated` events merge into the same
    /// entry.
    ToolCallStarted(ToolCall),
    /// Incremental update (status change, content/output additions) for a
    /// previously-announced tool call.
    ToolCallUpdated(ToolCallUpdate),
}

/// How sketch responds to `session/request_permission` from the agent.
///
/// The Claude Agent SDK already auto-approves read-only tools (Read, Grep,
/// Glob, LS) without firing a permission request — those work in every
/// mode. This enum only controls what we do when the agent asks to
/// Edit/Write/Move/Delete/Execute/Fetch.
///
/// Stored as `u8` in an [`AtomicU8`] so the worker thread can read it
/// without locking from inside the permission-request callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PermissionMode {
    /// Decline every gated tool. Equivalent to the original behaviour —
    /// the agent can browse the codebase but can't change it.
    ReadOnly = 0,
    /// Allow file-mutation tools (Edit/Write/Move) and Search; decline
    /// Bash (Execute), Delete, and external Fetch. The "iterating on the
    /// editor while reading my code" sweet spot.
    AutoEdit = 1,
    /// Ask the user — currently equivalent to ReadOnly until a real UI
    /// lands. Kept in the cycle so the muscle memory is set when we add
    /// inline approval prompts.
    AskEachTime = 2,
    /// Allow everything, including Bash. Trust the agent fully.
    Yolo = 3,
}

impl PermissionMode {
    /// Cycle to the next mode. Order chosen so the safest comes first and
    /// each tap relaxes restrictions one notch.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::ReadOnly => Self::AutoEdit,
            Self::AutoEdit => Self::AskEachTime,
            Self::AskEachTime => Self::Yolo,
            Self::Yolo => Self::ReadOnly,
        }
    }

    /// Short label for chrome (header / status bar).
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::AutoEdit => "auto-edit",
            Self::AskEachTime => "ask-each",
            Self::Yolo => "yolo",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::AutoEdit,
            2 => Self::AskEachTime,
            3 => Self::Yolo,
            _ => Self::ReadOnly,
        }
    }
}

/// Decide whether `kind` is allowed under `mode`. Centralised so both
/// sides of the worker boundary use identical logic.
fn allow_tool_kind(mode: PermissionMode, kind: ToolKind) -> bool {
    match mode {
        PermissionMode::ReadOnly | PermissionMode::AskEachTime => false,
        PermissionMode::AutoEdit => matches!(
            kind,
            ToolKind::Read | ToolKind::Edit | ToolKind::Move | ToolKind::Search | ToolKind::Think
        ),
        PermissionMode::Yolo => true,
    }
}

/// Ordered candidate list for an empty `command_str`. Tried in sequence;
/// the first that successfully spawns wins. The real npm package today is
/// `@zed-industries/claude-code-acp` which installs as `claude-code-acp`.
/// `claude-agent-acp` is kept as a forward-compat alias in case the
/// package is republished under that name.
pub const DEFAULT_AGENT_FALLBACKS: &[&str] = &["claude-code-acp", "claude-agent-acp"];

/// A live ACP connection to a locally-spawned agent subprocess.
///
/// API mirrors `claude_channel::ChannelClient` so that `app.rs` can drive
/// either by trait-like sniffing without inheriting any of the protocol
/// details.
pub struct AcpChannelClient {
    /// Outbound prompts: `App::claude_acp_send_text` → worker.
    prompt_tx: std_mpsc::Sender<String>,
    /// Inbound timeline events (text chunks + tool-call activity):
    /// worker → `App::pump_acp_replies`. Order on the channel is the
    /// chronological order in which the agent emitted them, so the
    /// renderer can interleave text and tool-call blocks faithfully.
    reply_rx: std_mpsc::Receiver<ReplyEvent>,
    /// Shared liveness flag. Worker flips this to false on EOF/error/exit;
    /// `App` checks it before sending and on each pump tick.
    connected: Arc<AtomicBool>,
    /// Count of completed turns — incremented by the worker every time the
    /// agent's `session/prompt` response resolves (successfully or not). The
    /// protocol doesn't surface a turn number, so we derive it locally.
    turns: Arc<AtomicUsize>,
    /// Live session id, populated by the worker after `session/new` (or
    /// `session/load`) completes. Persisted to disk so the next run of
    /// sketch can resume the same Claude session via `session/load`.
    session_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Wake-channel receiver. The worker pushes a `()` here after every
    /// reply event; the App side consumes it on first use (via
    /// [`take_wake_receiver`]) to drive an event-driven pump. With this
    /// in place the GPUI foreground task no longer has to rely on a
    /// polling timer surviving idle throttling — a chunk arrives and
    /// the pump task is woken directly.
    wake_rx: std::sync::Mutex<Option<futures::channel::mpsc::UnboundedReceiver<()>>>,
    /// Current permission policy. Read by the worker's
    /// `on_receive_request` closure on every gated tool call; written by
    /// the App side via [`set_permission_mode`].
    permission_mode: Arc<AtomicU8>,
    /// Joined on Drop so the worker has a chance to clean up the runtime
    /// (kill the child, drop streams) before sketch exits.
    worker: Option<JoinHandle<()>>,
    /// Cosmetic: surfaced via `:claude-acp-status` so the user can see what
    /// command was spawned.
    command: String,
    /// Cosmetic: working directory the session was started in (echoed to the
    /// user; the agent itself sees this via `session/new`).
    cwd: PathBuf,
}

impl AcpChannelClient {
    /// Spawn an ACP agent process and complete the initialize/new-session
    /// handshake.
    ///
    /// `command_str` is parsed shell-style: `"claude-agent-acp --debug"` →
    /// argv `["claude-agent-acp", "--debug"]`. If empty, the default
    /// fallback chain ([`DEFAULT_AGENT_FALLBACKS`]) is tried in order;
    /// the first that successfully spawns wins. This lets users on either
    /// the new (`claude-agent-acp`) or old (`claude-code-acp`) npm package
    /// name "just work" without setting `SKETCH_ACP_AGENT`.
    ///
    /// Each candidate is also resolved through a login shell (`$SHELL -lc
    /// 'command -v <name>'`) when direct PATH lookup fails. GUI processes
    /// launched outside an interactive shell often miss tool-manager paths
    /// (nvm, asdf, mise, brew shellenv, …); the login-shell hop sources
    /// the user's shell init so binaries installed via those managers can
    /// still be located.
    ///
    /// Returns once the initial handshake (initialize → new session) has
    /// completed; subsequent `send`/`try_recv` calls drive prompts in and
    /// pull streamed text out.
    pub fn spawn(command_str: &str, cwd: Option<PathBuf>) -> io::Result<Self> {
        Self::spawn_with_resume(command_str, cwd, None)
    }

    /// Like [`spawn`] but tries `session/load` with `resume_session_id`
    /// instead of `session/new`. Falls back to a fresh session if loading
    /// errors (the agent may have GC'd it, or it may not support the
    /// `loadSession` capability at all).
    pub fn spawn_with_resume(
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
    ) -> io::Result<Self> {
        Self::spawn_with_resume_in(command_str, cwd, resume_session_id, SketchFrontend::Tui)
    }

    /// Frontend-aware variant of [`spawn_with_resume`]. The `frontend`
    /// argument is woven into the system-prompt append so Claude knows
    /// which sketch host is driving it — used by `sketch-gpui` to
    /// announce itself as the GPUI desktop frontend rather than the
    /// default TUI. All other behaviour is identical to
    /// [`spawn_with_resume`].
    pub fn spawn_with_resume_in(
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: SketchFrontend,
    ) -> io::Result<Self> {
        let candidates: Vec<String> = if command_str.trim().is_empty() {
            DEFAULT_AGENT_FALLBACKS
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            vec![command_str.trim().to_string()]
        };

        let mut last_err: Option<io::Error> = None;
        let mut tried: Vec<String> = Vec::new();
        for command in candidates {
            tried.push(command.clone());
            // First try: direct PATH lookup (cheap, works for absolute
            // paths and binaries on the inherited PATH).
            match Self::try_spawn(&command, cwd.clone(), resume_session_id.clone(), frontend) {
                Ok(client) => return Ok(client),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }

            // Second try: ask the user's login shell where the binary
            // lives. nvm / asdf / mise typically set PATH via shell init
            // that GUI processes don't load.
            if let Some(resolved) = resolve_via_login_shell(&command) {
                if resolved != command {
                    tried.push(resolved.clone());
                }
                match Self::try_spawn(&resolved, cwd.clone(), resume_session_id.clone(), frontend) {
                    Ok(client) => return Ok(client),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        last_err = Some(e);
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // All candidates exhausted with NotFound. Surface a single error
        // that names everything we tried so the user knows what to install.
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no ACP agent on PATH (tried {}). Install with `npm i -g @zed-industries/claude-code-acp`, or set SKETCH_ACP_AGENT=/path/to/agent. Last error: {}",
                tried.join(", "),
                last_err
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "<none>".into()),
            ),
        ))
    }

    /// Single-attempt spawn — used internally by `spawn` to walk the
    /// candidate chain. Same handshake semantics as the public `spawn`,
    /// just without the fallback loop.
    fn try_spawn(
        command: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: SketchFrontend,
    ) -> io::Result<Self> {
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        });

        let parts = shell_words::split(command).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("could not parse agent command: {e}"),
            )
        })?;
        if parts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent command was empty",
            ));
        }

        let (prompt_tx, prompt_rx) = std_mpsc::channel::<String>();
        let (reply_tx, reply_rx) = std_mpsc::channel::<ReplyEvent>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<io::Result<()>>();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_for_worker = connected.clone();
        let turns = Arc::new(AtomicUsize::new(0));
        let turns_for_worker = turns.clone();
        let session_id: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let session_id_for_worker = session_id.clone();
        // Default to Yolo so the agent can run its full edit→build→test
        // loop without being silently cut off at the Bash boundary. The
        // mode toggle (`<space> k m`) cycles to ReadOnly / AutoEdit when
        // the user wants tighter limits per session.
        let permission_mode = Arc::new(AtomicU8::new(PermissionMode::Yolo as u8));
        let permission_mode_for_worker = permission_mode.clone();

        // Wake channel: the worker pushes `()` every time it forwards a
        // reply event, so the foreground pump task can `select!` on it
        // and wake immediately when a chunk arrives — instead of waiting
        // for the next polling tick. Receiver is taken (once) by the
        // pump after attach succeeds.
        let (wake_tx, wake_rx) = futures::channel::mpsc::unbounded::<()>();

        let worker_cwd = cwd.clone();
        let worker = thread::Builder::new()
            .name("sketch-acp-worker".into())
            .spawn(move || {
                run_worker(
                    parts,
                    worker_cwd,
                    prompt_rx,
                    reply_tx,
                    ready_tx,
                    connected_for_worker,
                    turns_for_worker,
                    session_id_for_worker,
                    resume_session_id,
                    permission_mode_for_worker,
                    wake_tx,
                    frontend,
                );
            })?;

        // Wait for the initialize+new-session handshake to either succeed or
        // fail. We drop the channel afterwards — readiness is signalled once.
        let initial = ready_rx.recv().map_err(|_| {
            io::Error::other("acp worker exited before reporting readiness")
        })?;
        if let Err(e) = initial {
            // The worker has bailed; tear it down before returning.
            connected.store(false, Ordering::SeqCst);
            let _ = worker.join();
            return Err(e);
        }

        Ok(Self {
            prompt_tx,
            reply_rx,
            connected,
            turns,
            session_id,
            permission_mode,
            wake_rx: std::sync::Mutex::new(Some(wake_rx)),
            worker: Some(worker),
            command: command.to_string(),
            cwd,
        })
    }

    /// Take the wake-channel receiver. Returns `Some` exactly once per
    /// client; subsequent calls return `None`. Caller (typically the
    /// GPUI pump task) uses it to await events without polling.
    pub fn take_wake_receiver(
        &self,
    ) -> Option<futures::channel::mpsc::UnboundedReceiver<()>> {
        self.wake_rx.lock().ok().and_then(|mut g| g.take())
    }

    /// Read the current permission policy. The worker uses this on every
    /// `session/request_permission` callback to decide allow vs decline.
    pub fn permission_mode(&self) -> PermissionMode {
        PermissionMode::from_u8(self.permission_mode.load(Ordering::SeqCst))
    }

    /// Switch the permission policy. Takes effect immediately for any
    /// permission requests that fire after this call returns.
    pub fn set_permission_mode(&self, mode: PermissionMode) {
        self.permission_mode.store(mode as u8, Ordering::SeqCst);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// How many turns have completed on this session. Useful for chrome
    /// ("turn 3/…") and for detecting when a reply has finished landing.
    pub fn turn_count(&self) -> usize {
        self.turns.load(Ordering::SeqCst)
    }

    /// The agent-assigned session id, populated once the initialize +
    /// `session/new` (or `session/load`) handshake completes. Persist this
    /// across sketch runs so a future invocation can pick up the same
    /// Claude session via `loadSession`.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|g| g.clone())
    }

    /// User-facing label for `:claude-acp-status`.
    pub fn description(&self) -> String {
        format!("ACP agent: {} (cwd: {})", self.command, self.cwd.display())
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// Send a prompt to the agent. Returns Err if the worker has died (e.g.
    /// the child crashed) so the caller can drop the connection.
    pub fn send(&mut self, prompt: &str) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        self.prompt_tx.send(prompt.to_string()).map_err(|_| {
            self.connected.store(false, Ordering::SeqCst);
            io::Error::new(io::ErrorKind::BrokenPipe, "ACP worker channel closed")
        })
    }

    /// Pull one queued reply event (text chunk or tool-call activity) if
    /// any are pending. Non-blocking — safe to call every tick.
    pub fn try_recv(&self) -> Option<ReplyEvent> {
        self.reply_rx.try_recv().ok()
    }
}

impl Drop for AcpChannelClient {
    fn drop(&mut self) {
        // **Order matters**: we must drop `prompt_tx` BEFORE waiting on the
        // worker, otherwise the worker's bridge task is still blocked on
        // `prompt_rx.recv()` and will never let the driver loop unwind.
        // Replace the Sender with a fresh dummy that we then drop, which
        // guarantees the kernel-side close happens here regardless of
        // field-drop ordering.
        self.connected.store(false, Ordering::SeqCst);
        // Dropping prompt_tx by replacing it: we can't move out of `&mut self`
        // for a `Sender<String>`, but we *can* swap it with a fresh disposable
        // pair whose receiver we throw away — that releases the original tx.
        let (dummy_tx, _dummy_rx) = std_mpsc::channel::<String>();
        // Note: replaces only `prompt_tx` (still String). `reply_rx` lives
        // on the App side and gets dropped naturally when AcpChannelClient is
        // dropped — no manual swap needed.
        let real_tx = std::mem::replace(&mut self.prompt_tx, dummy_tx);
        drop(real_tx);

        if let Some(handle) = self.worker.take() {
            // Now safe to join: the worker's blocking recv is unblocked,
            // bridge_task exits, async_prompt_rx returns None, the driver
            // loop returns, connect_with returns, and the runtime drops.
            let _ = handle.join();
        }
    }
}

/// Internal: messages forwarded from the agent's notification handler to the
/// async driver task that owns the std-mpsc reply channel.
enum WorkerEvent {
    /// One reply event, ready to relay to the App. Wraps [`ReplyEvent`]
    /// so the notification handler stays free of channel-implementation
    /// detail.
    Reply(ReplyEvent),
}

/// Ask the user's login shell to locate `command_word` (the first
/// whitespace-separated token of `command`). Returns the resolved command
/// string with the bare name swapped for an absolute path, or `None` if
/// the shell can't find it either.
///
/// Why this exists: nvm, asdf, mise, brew shellenv, and similar tool
/// managers add to `PATH` via shell init (`.bashrc` / `.zshrc` /
/// `.bash_profile`). GUI processes launched outside an interactive shell
/// don't run that init and miss those paths. Running `$SHELL -lc 'command
/// -v X'` re-runs the user's shell init in a login shell so the lookup
/// matches what the user sees in their terminal.
///
/// Times out at 3 s so a misconfigured shell can't hang the UI thread.
fn resolve_via_login_shell(command: &str) -> Option<String> {
    let mut parts = shell_words::split(command).ok()?;
    if parts.is_empty() {
        return None;
    }
    let bare_name = parts.remove(0);
    // If the user already gave an absolute / relative path, there's no
    // PATH-lookup to do — just return the input unchanged.
    if bare_name.contains('/') {
        return Some(command.to_string());
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    // `command -v` is POSIX and prints the resolved path on stdout. `-l`
    // forces a login shell so init scripts (where nvm etc. live) run.
    let output = std::process::Command::new(&shell)
        .arg("-lc")
        .arg(format!("command -v {}", shell_words::quote(&bare_name)))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if resolved.is_empty() {
        return None;
    }

    // Re-attach the original args (already shell-quoted by the caller).
    if parts.is_empty() {
        Some(resolved)
    } else {
        let quoted_args: Vec<String> = parts.iter().map(|a| shell_words::quote(a).into_owned()).collect();
        Some(format!("{} {}", shell_words::quote(&resolved), quoted_args.join(" ")))
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<ReplyEvent>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
    turns: Arc<AtomicUsize>,
    session_id_slot: Arc<std::sync::Mutex<Option<String>>>,
    resume_session_id: Option<String>,
    permission_mode: Arc<AtomicU8>,
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
    frontend: SketchFrontend,
) {
    // Build a small multi-thread runtime — the ACP crate spawns several
    // tasks internally (read loop, write loop, response router) and a
    // current-thread runtime can deadlock if any of them blocks the only
    // worker. Two threads is plenty.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            connected.store(false, Ordering::SeqCst);
            let _ = ready_tx.send(Err(io::Error::other(format!("tokio runtime: {e}"))));
            return;
        }
    };

    let connected_for_async = connected.clone();
    let result = rt.block_on(async move {
        worker_async(
            parts,
            cwd,
            prompt_rx,
            reply_tx,
            ready_tx,
            connected_for_async,
            turns,
            session_id_slot,
            resume_session_id,
            permission_mode,
            wake_tx,
            frontend,
        )
        .await
    });
    if let Err(e) = result {
        // Errors after the readiness handshake bubble out here — log to
        // stderr (sketch is in alt-screen, so this is mostly diagnostic for
        // people running with `2>log`).
        connected.store(false, Ordering::SeqCst);
        eprintln!("[sketch-acp] worker exited with error: {e}");
    }
    // Runtime drops here, killing any straggling tokio tasks (and the child
    // process via Drop on tokio::process::Child).
    connected.store(false, Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
async fn worker_async(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<ReplyEvent>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
    turns: Arc<AtomicUsize>,
    session_id_slot: Arc<std::sync::Mutex<Option<String>>>,
    resume_session_id: Option<String>,
    permission_mode: Arc<AtomicU8>,
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
    frontend: SketchFrontend,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1) Spawn the agent process.
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Pipe-and-discard the agent's stderr by default. Agents (including
        // claude-agent-acp) may log diagnostics there; keeping it inherit
        // would corrupt sketch's TUI. Set SKETCH_ACP_AGENT_STDERR=inherit to
        // surface it for debugging.
        .stderr(if std::env::var("SKETCH_ACP_AGENT_STDERR").as_deref() == Ok("inherit") {
            std::process::Stdio::inherit()
        } else {
            std::process::Stdio::null()
        })
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to spawn '{}': {}", parts[0], e),
            )));
            return Ok(());
        }
    };
    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = ready_tx.send(Err(io::Error::other("failed to open child stdin")));
            return Ok(());
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = ready_tx.send(Err(io::Error::other("failed to open child stdout")));
            return Ok(());
        }
    };

    // 2) Set up an inbound text-chunk channel from the notification handler
    //    into the driver task. We ferry through tokio::mpsc so the
    //    notification callback (async) and the prompt driver loop (also
    //    async, in the same runtime) can both touch it without locks.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<WorkerEvent>();

    // The inbound-chunk pump: drains event_rx and pushes onto the std mpsc
    // that App::pump_acp_replies polls. Run as a separate task so it stays
    // alive even while the driver is awaiting send_request.
    //
    // After every successful forward we also push `()` onto the wake
    // channel so the GPUI foreground pump task can wake immediately
    // instead of waiting for its next polling tick. `unbounded_send`
    // here is non-blocking and never errors when the receiver is still
    // alive; if the receiver has been dropped (e.g. screen torn down)
    // we just ignore the send error, the run loop will eventually
    // notice via `is_connected`.
    let reply_tx_for_pump = reply_tx.clone();
    let connected_for_pump = connected.clone();
    let wake_tx_for_pump = wake_tx.clone();
    let pump_task = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            match ev {
                WorkerEvent::Reply(reply) => {
                    if reply_tx_for_pump.send(reply).is_err() {
                        // App side dropped the receiver — connection torn
                        // down. Stop pumping; the driver loop will notice
                        // when it tries to read prompts.
                        connected_for_pump.store(false, Ordering::SeqCst);
                        break;
                    }
                    let _ = wake_tx_for_pump.unbounded_send(());
                }
            }
        }
    });

    // 3) Bridge the std mpsc prompt channel into a tokio mpsc the async
    //    driver loop can await on. spawn_blocking holds the std recv() call.
    let (async_prompt_tx, mut async_prompt_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let bridge_task = tokio::task::spawn_blocking(move || {
        while let Ok(prompt) = prompt_rx.recv() {
            if async_prompt_tx.send(prompt).is_err() {
                break;
            }
        }
        // Sender side dropped → done. Closing async_prompt_tx (by drop here)
        // signals the driver loop to exit cleanly.
    });

    // 4) Run the ACP client. The closure passed to connect_with stays alive
    //    until we explicitly return — that's our "session lifetime".
    let event_tx_for_handlers = event_tx.clone();
    let connect_result = Client
        .builder()
        .name("sketch")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                acp_debug!("notification: {:?}", notification.update);
                // Forward the variants the renderer knows how to interleave:
                // streamed text + tool-call activity. Everything else
                // (thoughts, plans, mode/info updates) is dropped at this
                // layer for now — those would each need their own UI.
                match notification.update {
                    SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(text),
                        ..
                    }) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::Chunk(text.text)));
                    }
                    SessionUpdate::ToolCall(tc) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::ToolCallStarted(tc)));
                    }
                    SessionUpdate::ToolCallUpdate(upd) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::ToolCallUpdated(upd)));
                    }
                    _ => {}
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request({
            let permission_mode = permission_mode.clone();
            async move |req: RequestPermissionRequest, responder, _cx| {
                // Decide allow vs decline using the user-controlled mode +
                // the tool's semantic kind. The Claude Agent SDK tags
                // every gated tool call with a `kind` (Edit/Execute/etc)
                // — we map that to a fixed allow-list per mode.
                let mode =
                    PermissionMode::from_u8(permission_mode.load(Ordering::SeqCst));
                let kind = req.tool_call.fields.kind.unwrap_or(ToolKind::Other);
                let outcome = if allow_tool_kind(mode, kind) {
                    // Pick the agent-suggested AllowOnce option so the
                    // approval doesn't accidentally stick across
                    // categories of tool. Fall back to the first non-
                    // reject option if AllowOnce isn't offered (older
                    // agent versions); ultimately decline if the agent
                    // didn't surface anything we can pick.
                    let pick = req
                        .options
                        .iter()
                        .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce))
                        .or_else(|| {
                            req.options.iter().find(|o| {
                                !matches!(
                                    o.kind,
                                    PermissionOptionKind::RejectOnce
                                        | PermissionOptionKind::RejectAlways
                                )
                            })
                        });
                    match pick {
                        Some(opt) => RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(opt.option_id.clone()),
                        ),
                        None => RequestPermissionOutcome::Cancelled,
                    }
                } else {
                    RequestPermissionOutcome::Cancelled
                };
                acp_debug!(
                    "permission {:?} for kind {:?} → {:?}",
                    mode,
                    kind,
                    outcome
                );
                responder.respond(RequestPermissionResponse::new(outcome))
            }
        },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat()),
            move |connection: ConnectionTo<Agent>| {
                let cwd = cwd.clone();
                let ready_tx = ready_tx;
                let turns = turns.clone();
                let resume_session_id = resume_session_id.clone();
                let session_id_slot = session_id_slot.clone();
                async move {
                    acp_debug!("sending initialize");
                    // === Initialize handshake ===
                    let init_resp = match connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            let _ = ready_tx.send(Err(io::Error::other(format!(
                                "ACP initialize failed: {e}"
                            ))));
                            return Err(e);
                        }
                    };
                    let supports_load = init_resp.agent_capabilities.load_session;
                    acp_debug!(
                        "initialize ok; loadSession capability: {supports_load}, resume id: {:?}",
                        resume_session_id
                    );

                    // Two jobs for this `_meta.systemPrompt.append`:
                    //
                    // 1. Flip `hasAppendSystemPrompt` to `true` so the SDK
                    //    picks the "Claude Code… running within the Claude
                    //    Agent SDK" identity (`pA7`) over the more verbose
                    //    "Claude agent built on the SDK" default (`dA7`).
                    //    Any non-empty string achieves this.
                    //
                    // 2. Restore the sections the SDK composer drops vs
                    //    the TUI composer — most importantly `# Task
                    //    Management` (with the worked "run the build, fix
                    //    errors" example), `# Tool usage policy`, `# Code
                    //    References`, and `# Professional objectivity`/
                    //    `# No time estimates`. The text below is pulled
                    //    verbatim from `cli.js` (functions xwz, mwz, Qwz,
                    //    Iwz, Fwz) with placeholder tool refs filled in
                    //    with their canonical Claude Code names. The
                    //    sketch-specific verify-after-edit clause stays
                    //    on the front so it's the first thing the model
                    //    sees in the append.
                    // Body of the system-prompt append. The first sentence
                    // is built separately so it can name the active sketch
                    // frontend (TUI vs GPUI) — everything below is shared.
                    const CLAUDE_CODE_APPEND_BODY: &str = r#"Treat this as an interactive coding session, not a one-shot agent run.

# Tone and style
The user has explicitly asked for this voice; it overrides any earlier tone guidance:

- Be succinct. Summarize what happened — don't narrate every step you took to get there.
- Status updates while you're working should be one short line ("Reading X.", "Running tests.", "Editing Y."). The user wants to know what you're doing in flight, but in headline form.
- Reserve full prose for the moment you actually reach a solid conclusion or finish the task. That's when the user wants the writeup — not before.
- Don't think out loud in the message channel. Internal reasoning, exploration, and intermediate thoughts belong inside tool calls and your own reasoning, not in the user-facing response. The user is looking at sketch's chat, not a transcript of your inner monologue.
- Default response length is 1–3 sentences. Expand only when the task genuinely requires more (e.g., explaining a complex tradeoff or summarizing many files at once).
- Skip preambles like "I'll go ahead and", "Let me", or restating the question. Skip postambles like "Let me know if you'd like me to" or "I hope this helps". Get to the answer.
- After completing work, report only: what changed, where, and any caveats. Don't recap the journey.

# Task Management
You have access to the TodoWrite tool to help you manage and plan tasks. Use these tools VERY frequently to ensure that you are tracking your tasks and giving the user visibility into your progress. These tools are also EXTREMELY helpful for planning tasks, and for breaking down larger complex tasks into smaller steps. If you do not use this tool when planning, you may forget to do important tasks - and that is unacceptable.

It is critical that you mark todos as completed as soon as you are done with a task. Do not batch up multiple tasks before marking them as completed.

<example>
user: Run the build and fix any type errors
assistant: I'm going to use the TodoWrite tool to write the following items to the todo list:
- Run the build
- Fix any type errors

I'm now going to run the build using Bash.

Looks like I found 10 type errors. I'm going to use the TodoWrite tool to write 10 items to the todo list.

marking the first todo as in_progress

Let me start working on the first item...

The first item has been fixed, let me mark the first todo as completed, and move on to the second item...
</example>

After you change code, complete the loop yourself: run the project's build and tests (e.g. `cargo check` / `cargo test`, `npm test`, `pytest`) before reporting the task done. If a check fails, iterate until it passes or you have a concrete reason to stop and ask the user.

# Tool usage policy
- You can call multiple tools in a single response. If you intend to call multiple tools and there are no dependencies between them, make all independent tool calls in parallel. Maximize use of parallel tool calls where possible to increase efficiency. However, if some tool calls depend on previous calls to inform dependent values, do NOT call these tools in parallel and instead call them sequentially. For instance, if one operation must complete before another starts, run these operations sequentially instead. Never use placeholders or guess missing parameters in tool calls.
- Use specialized tools instead of bash commands when possible, as this provides a better user experience. For file operations, use dedicated tools: Read for reading files instead of cat/head/tail, Edit for editing instead of sed/awk, and Write for creating files instead of cat with heredoc or echo redirection. Reserve bash tools exclusively for actual system commands and terminal operations that require shell execution.

# Code References
When referencing specific functions or pieces of code include the pattern `file_path:line_number` to allow the user to easily navigate to the source code location.

<example>
user: Where are errors from the client handled?
assistant: Clients are marked as failed in the `connectToServer` function in src/services/process.ts:712.
</example>

# Professional objectivity
Prioritize technical accuracy and truthfulness over validating the user's beliefs. Focus on facts and problem-solving, providing direct, objective technical info without any unnecessary superlatives, praise, or emotional validation. It is best for the user if Claude honestly applies the same rigorous standards to all ideas and disagrees when necessary, even if it may not be what the user wants to hear. Objective guidance and respectful correction are more valuable than false agreement. Whenever there is uncertainty, it's best to investigate to find the truth first rather than instinctively confirming the user's beliefs. Avoid using over-the-top validation or excessive praise when responding to users such as "You're absolutely right" or similar phrases.

# No time estimates
Never give time estimates or predictions for how long tasks will take, whether for your own work or for users planning their projects. Avoid phrases like "this will take me a few minutes," "should be done in about 5 minutes," "this is a quick fix," "this will take 2-3 weeks," or "we can do this later." Focus on what needs to be done, not how long it might take. Break work into actionable steps and let users judge timing for themselves.

IMPORTANT: Always use the TodoWrite tool to plan and track tasks throughout the conversation."#;
                    let claude_code_append = format!(
                        "You are running inside the sketch editor's Claude Code surface — host: {host}. {body}",
                        host = frontend.host_description(),
                        body = CLAUDE_CODE_APPEND_BODY,
                    );
                    let claude_code_meta = || {
                        let mut m = serde_json::Map::new();
                        m.insert(
                            "systemPrompt".to_string(),
                            serde_json::json!({"append": claude_code_append.as_str()}),
                        );
                        m
                    };

                    // === Bring up a session: try resume first if we were
                    //     given an id and the agent supports it; otherwise
                    //     fall through to a fresh session/new. We auto-fall
                    //     back on load failure so a stale or GC'd id never
                    //     leaves the user without an attached agent.
                    let session_id: SessionId = if let (true, Some(id)) =
                        (supports_load, resume_session_id.as_ref())
                    {
                        let load_req = LoadSessionRequest::new(
                            SessionId::new(id.clone()),
                            cwd.clone(),
                        )
                        .meta(claude_code_meta());
                        match connection.send_request(load_req).block_task().await {
                            Ok(_resp) => {
                                acp_debug!("session/load ok: {id}");
                                // Bump the turn counter so the App side
                                // detects "turn ended" on the next pump
                                // tick and runs `finalize_claude_turn`.
                                // session/load synthesises a whole prior
                                // conversation via session/update
                                // notifications without ever firing a
                                // session/prompt response — without
                                // this bump the buffer would end with
                                // the cursor stranded on a frozen
                                // line and no editable space below for
                                // the user to type their next prompt.
                                turns.fetch_add(1, Ordering::SeqCst);
                                SessionId::new(id.clone())
                            }
                            Err(e) => {
                                acp_debug!("session/load failed ({e}); falling back to session/new");
                                match connection
                                    .send_request(
                                        NewSessionRequest::new(cwd.clone())
                                            .meta(claude_code_meta()),
                                    )
                                    .block_task()
                                    .await
                                {
                                    Ok(r) => r.session_id,
                                    Err(e2) => {
                                        let _ = ready_tx.send(Err(io::Error::other(format!(
                                            "ACP new session failed (after load fallback): {e2}"
                                        ))));
                                        return Err(e2);
                                    }
                                }
                            }
                        }
                    } else {
                        match connection
                            .send_request(
                                NewSessionRequest::new(cwd.clone()).meta(claude_code_meta()),
                            )
                            .block_task()
                            .await
                        {
                            Ok(r) => r.session_id,
                            Err(e) => {
                                let _ = ready_tx.send(Err(io::Error::other(format!(
                                    "ACP new session failed: {e}"
                                ))));
                                return Err(e);
                            }
                        }
                    };
                    acp_debug!("session ready: {session_id:?}");
                    if let Ok(mut slot) = session_id_slot.lock() {
                        *slot = Some(session_id.0.to_string());
                    }

                    // Handshake done — App can start sending.
                    let _ = ready_tx.send(Ok(()));

                    // === Driver loop: forward prompts as session/prompt
                    //     requests until the App side closes the channel.  ===
                    while let Some(prompt) = async_prompt_rx.recv().await {
                        acp_debug!("prompt → agent: {prompt:?}");
                        // Each prompt is a turn — fire and forget the
                        // request future. Streamed chunks reach the user
                        // through the notification handler, which doesn't
                        // need to live inside this loop.
                        let req = agent_client_protocol::schema::PromptRequest::new(
                            session_id.clone(),
                            vec![ContentBlock::Text(
                                agent_client_protocol::schema::TextContent::new(prompt),
                            )],
                        );
                        // Block on the prompt response (turn end) before
                        // accepting the next prompt — simpler than
                        // managing concurrent in-flight turns and matches
                        // how a chat UI flows.
                        match connection.send_request(req).block_task().await {
                            Ok(resp) => acp_debug!("prompt response: {resp:?}"),
                            Err(e) => eprintln!("[sketch-acp] prompt failed: {e}"),
                        }
                        // Bump the turn counter regardless of success — the
                        // turn is "complete" either way (the user can send
                        // again). Errors are surfaced via stderr above.
                        turns.fetch_add(1, Ordering::SeqCst);
                    }
                    acp_debug!("driver loop exiting");
                    Ok::<_, agent_client_protocol::Error>(())
                }
            },
        )
        .await;

    acp_debug!("connect_with returned: {:?}", connect_result.is_ok());
    // Tear down regardless of outcome. Drop the local event_tx, then abort
    // pump_task — even if the SDK is still holding a handler closure (and
    // therefore a clone of `event_tx_for_handlers`), abort makes sure we
    // don't dangle. Same for bridge_task: spawn_blocking can outlive the
    // runtime drop, and aborting is a no-op for already-finished tasks.
    drop(event_tx);
    pump_task.abort();
    bridge_task.abort();
    let _ = pump_task.await;
    // bridge_task is spawn_blocking; abort signals its JoinHandle but the
    // OS thread inside isn't actually killable. Hence we don't await it
    // here — its still-blocked recv() will return Err once prompt_tx (held
    // by `AcpChannelClient`) is dropped, then the closure returns and the
    // OS thread exits naturally on the next runtime tick.
    drop(child);

    if let Err(e) = connect_result {
        return Err(Box::new(io::Error::other(format!("acp connection: {e}"))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    /// Build a tiny Python script that pretends to be an ACP agent: it
    /// answers `initialize` and `session/new`, then for each `session/prompt`
    /// streams two `agent_message_chunk` notifications and a final
    /// `PromptResponse` with `endTurn`. Everything is line-delimited JSON
    /// per ACP framing.
    ///
    /// This is intentionally hand-rolled rather than going through the
    /// agent-side of the SDK so the test exercises the wire format we
    /// actually care about — what real agents emit.
    fn write_fake_agent_script(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake_acp_agent.py");
        let script = r#"#!/usr/bin/env python3
import sys, json

def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

# Use readline (not `for line in sys.stdin:`) so we react as each request
# arrives rather than buffering until EOF.
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    method = msg.get("method", "")
    msg_id = msg.get("id")
    if method == "initialize":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt":
        # Stream two chunks, then return.
        emit({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "hello "}}}})
        emit({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "world"}}}})
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"stopReason": "end_turn"}})
    # else: ignore (notifications, unknown methods)
"#;
        let mut f = std::fs::File::create(&path).expect("create script");
        f.write_all(script.as_bytes()).expect("write");
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn round_trip_with_fake_agent() {
        // Skip if the test host has no python3 — the fake-agent script
        // depends on it for JSON parsing. CI machines reliably have it.
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            eprintln!("python3 not available — skipping ACP fake-agent round-trip");
            return;
        }

        let tmp = tempfile::tempdir().expect("tmpdir");
        let script = write_fake_agent_script(tmp.path());

        let mut client = AcpChannelClient::spawn(
            script.to_str().unwrap(),
            Some(tmp.path().to_path_buf()),
        )
        .expect("spawn ACP agent");
        assert!(client.is_connected());
        assert!(client.description().contains("fake_acp_agent"));

        client.send("hi there").expect("send prompt");

        // Poll for chunks. The fake agent emits two: "hello " and "world".
        let mut got = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(ReplyEvent::Chunk(chunk)) = client.try_recv() {
                got.push_str(&chunk);
                if got.contains("hello world") {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            got.contains("hello world"),
            "expected streamed reply 'hello world', got {got:?}"
        );
    }

    #[test]
    fn spawn_fails_with_missing_binary() {
        let err = match AcpChannelClient::spawn(
            "/no/such/binary/that/exists-please",
            None,
        ) {
            Ok(_) => panic!("expected spawn failure for missing binary"),
            Err(e) => e,
        };
        // Either the spawn itself failed, or readiness reported failure;
        // both surface the same way.
        let msg = err.to_string();
        assert!(
            msg.contains("failed to spawn") || msg.contains("No such file"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    #[ignore = "actually runs the default agent if installed; the handshake \
                can hang indefinitely on machines with claude-code-acp present \
                but in an unauthenticated state. Run explicitly with \
                `cargo test --ignored empty_command_uses_default` when needed."]
    fn empty_command_uses_default() {
        // We don't actually expect this to succeed (claude-agent-acp may
        // not be installed in CI), but spawn() should at least try to
        // run the default binary, not panic on empty input.
        let result = AcpChannelClient::spawn("", None);
        // If it succeeded, drop the client; if it failed, the failure
        // message should reference the default command name.
        match result {
            Ok(c) => drop(c),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains(DEFAULT_AGENT_COMMAND) || msg.contains("No such file"),
                    "expected error to mention default command, got: {msg}"
                );
            }
        }
    }
}
