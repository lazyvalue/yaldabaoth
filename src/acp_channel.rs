//! ACP (Agent Client Protocol) channel for yalda.
//!
//! This module is an alternative path to the existing
//! [`claude_channel`](crate::claude_channel) UNIX-socket integration. Where the
//! yalda-channel route requires Claude Code to be running and to have spawned
//! yalda-channel as an MCP server, the ACP route lets yalda *itself* spawn a
//! local agent subprocess and talk to it directly over JSON-RPC stdio (the
//! [Agent Client Protocol](https://agentclientprotocol.com/)). That means
//! yalda can ride the user's Claude Max subscription via the
//! `claude-agent-acp` (formerly `@zed-industries/claude-code-acp`) adapter
//! without ever touching an API key — Claude Code handles auth itself.
//!
//! ## Architecture
//!
//! The official `agent-client-protocol` crate is async/Tokio-based, but the
//! frontend drives it synchronously (the GPUI app pumps events on each tick).
//! To bridge async-to-sync, this module follows the same pattern as
//! `claude_channel.rs`:
//!
//! 1. Spawn a dedicated background **worker thread** that owns a multi-thread
//!    Tokio runtime.
//! 2. Inside that runtime, spawn the agent subprocess and run the ACP
//!    `Client.builder().connect_with(...)` driver loop. The closure stays
//!    alive for the lifetime of the connection — when yalda's drop signal
//!    fires, the closure returns and the worker thread tears the runtime
//!    down.
//! 3. Communicate between yalda (sync) and the worker (async) via two
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
//!   yalda UI. The agent can still respond with text (its own commentary)
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
    ContentBlock, ContentChunk, ImageContent, InitializeRequest, InitializeResponse,
    LoadSessionRequest, McpServer, McpServerStdio, NewSessionRequest, PermissionOptionKind,
    ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionId, SessionNotification, SessionUpdate, TextContent,
};
// Re-exported via this module so consumers (App / GPUI) don't need a
// direct dependency on the agent-client-protocol schema crate just to
// match on tool-call events. `pub use` also brings these into local
// scope, so anything below this line can refer to them unqualified.
pub use agent_client_protocol::schema::{
    Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionModeId, ToolCall, ToolCallContent,
    ToolCallId, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, JsonRpcRequest};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Emit a diagnostic line to stderr when YALDA_ACP_DEBUG is set in the env.
/// Gated this way so the chatter doesn't pollute stderr in normal use
/// but is one env-var away when something looks wrong.
macro_rules! acp_debug {
    ($($arg:tt)*) => {
        if std::env::var("YALDA_ACP_DEBUG").is_ok() {
            eprintln!("[yalda-acp] {}", format_args!($($arg)*));
        }
    };
}

/// Default agent command, kept for backwards compatibility (e.g. callers
/// that want to display "the default" somewhere). Real spawning uses
/// [`DEFAULT_AGENT_FALLBACKS`] so users on either binary name still work.
pub const DEFAULT_AGENT_COMMAND: &str = "claude-agent-acp";

/// Which coding-agent backend owns an ACP session. This identity is persisted
/// with the server session so a restart always resumes a thread with the same
/// adapter that created it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    #[default]
    Claude,
    Codex,
}

impl AgentProvider {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    pub fn default_command(self) -> &'static str {
        match self {
            Self::Claude => "claude-agent-acp",
            Self::Codex => "codex-acp",
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            Self::Claude => "npm i -g @agentclientprotocol/claude-agent-acp",
            Self::Codex => "npm i -g @agentclientprotocol/codex-acp",
        }
    }
}

/// Resolve the configured adapter command for one provider. The legacy
/// `YALDA_ACP_AGENT` override remains a Claude-only fallback so existing setups
/// keep working without accidentally routing a Codex session through Claude.
pub fn configured_agent_command(provider: AgentProvider) -> String {
    let provider_key = match provider {
        AgentProvider::Claude => "YALDA_CLAUDE_ACP_AGENT",
        AgentProvider::Codex => "YALDA_CODEX_ACP_AGENT",
    };
    std::env::var(provider_key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            (provider == AgentProvider::Claude)
                .then(|| std::env::var("YALDA_ACP_AGENT").ok())
                .flatten()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_default()
}

/// Resolve the absolute path to the `yalda-mcp` control binary. Prefers a
/// sibling of the current executable (the normal layout — all yalda bins build
/// into the same dir), falling back to the bare name `yalda-mcp` so a PATH
/// lookup by the agent still has a chance. Returns `None` only if the current
/// exe path can't be read AND we have no name to fall back to (never, in
/// practice — the bare-name fallback is always available).
fn yalda_mcp_binary_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("yalda-mcp");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    // Fall back to the bare name; the spawned agent resolves it via PATH.
    Some(PathBuf::from("yalda-mcp"))
}

/// The MCP servers to register on every agent session Yalda spawns. Injecting
/// the `yalda-mcp` stdio server here (provider-agnostic — applied on both
/// `session/new` and `session/load`) lets an agent running *inside* Yalda
/// recursively control Yalda (e.g. spin up new sessions via the `create_session`
/// tool). Returns an empty vec if the binary can't be resolved, so a missing
/// control binary never breaks session creation.
pub fn yalda_mcp_servers() -> Vec<McpServer> {
    match yalda_mcp_binary_path() {
        Some(path) => vec![McpServer::Stdio(McpServerStdio::new("yalda", path))],
        None => Vec::new(),
    }
}

/// Which yalda frontend is hosting this ACP session. Threaded into the
/// system-prompt append so Claude knows which host it's running inside —
/// affects nothing protocol-side, only the host-description sentence at the
/// top of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YaldaFrontend {
    /// Desktop frontend (GPUI). Selected by `yalda-gpui`.
    #[default]
    Gpui,
}

impl YaldaFrontend {
    /// Sentence describing the host — interpolated into the system-prompt
    /// append so the model can adapt phrasing if it cares.
    fn host_description(self) -> &'static str {
        match self {
            Self::Gpui => "the GPUI desktop frontend (`yalda-gpui` binary)",
        }
    }
}

/// Yalda-side flattening of ACP's `UsageUpdate` (which is feature-gated
/// behind `unstable_session_usage`). Carrying our own struct means the
/// `ReplyEvent::UsageUpdated` variant stays unconditional regardless of
/// whether the upstream feature is enabled — only the emitter in the
/// notification handler is feature-gated (spec-agent-window.md §31).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageSnapshot {
    /// Tokens currently in the context window.
    pub tokens_used: u64,
    /// Total context window size in tokens.
    pub tokens_total: u64,
    /// Cumulative session cost in USD, if the upstream provided one.
    pub cost_usd: Option<f64>,
}

/// A single image attachment on a user prompt (e.g. pasted from the
/// clipboard). Crosses the GUI↔session-server wire (`session_proto`) and is
/// turned into an ACP `ContentBlock::Image` in the worker driver. `data` is
/// standard base64 of the raw image bytes (NO `data:` URI prefix); `mime_type`
/// is e.g. `"image/png"`. Ephemeral for now — not persisted in the WAL, so a
/// resumed/replayed transcript shows the prompt text but not the image.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub mime_type: String,
}

/// A user prompt as it travels through the ACP channel worker: prompt text
/// plus any image attachments. Bundled here (rather than the bare `String` the
/// channel used to carry) so the driver can build a mixed
/// `[Text, Image, …]` content-block vector for `session/prompt`.
#[derive(Debug, Clone, Default)]
pub struct PromptPayload {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

/// Ordered interaction stream used only when an adapter advertises native
/// steering. Keeping the initial prompt, follow-up steering requests, and
/// explicit Stop together prevents any later user action from overtaking an
/// earlier one.
#[derive(Debug)]
enum NativeSteeringCommand {
    Prompt(PromptPayload),
    Steer(PromptPayload),
    Cancel,
}

/// Codex ACP extension request advertised by initialize root
/// `_meta.steering.supported`. The adapter owns the per-session FIFO and
/// injects each payload into the live turn (or starts one if the boundary raced
/// us). A raw JSON response keeps us forward-compatible with additional
/// outcomes beyond today's `injected` / `startedNewTurn` / `failed` values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
struct NativeSteeringRequest {
    session_id: SessionId,
    prompt: Vec<ContentBlock>,
}

impl NativeSteeringRequest {
    fn new(session_id: SessionId, payload: &PromptPayload) -> Self {
        Self {
            session_id,
            prompt: payload.content_blocks(),
        }
    }
}

fn supports_native_steering(response: &InitializeResponse) -> bool {
    response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("steering"))
        .and_then(|steering| steering.get("supported"))
        .and_then(|supported| supported.as_bool())
        .unwrap_or(false)
}

impl PromptPayload {
    /// Convenience for the common text-only path.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    /// Build the ACP content-block vector for `session/prompt`: the text block
    /// (when non-empty) followed by one `Image` block per attachment. ACP
    /// requires at least one block, so an all-empty payload still yields a
    /// single empty text block.
    fn content_blocks(&self) -> Vec<ContentBlock> {
        let mut blocks = Vec::new();
        if !self.text.is_empty() {
            blocks.push(ContentBlock::Text(TextContent::new(self.text.clone())));
        }
        for img in &self.images {
            blocks.push(ContentBlock::Image(ImageContent::new(
                img.data.clone(),
                img.mime_type.clone(),
            )));
        }
        if blocks.is_empty() {
            blocks.push(ContentBlock::Text(TextContent::new(String::new())));
        }
        blocks
    }
}

/// One selectable model advertised by the agent's `model` config-option
/// `Select`. `id` is the wire value passed back to `session/set_config_option`
/// (e.g. `"default"`, `"sonnet"`, `"claude-fable-5[1m]"`); `label` is the
/// human name (e.g. `"Default (recommended)"`, `"Sonnet"`, `"Fable"`).
/// Serializable because it rides `ReplyEvent::ModelsAvailable` across the
/// session-server boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

/// Events drained by the App from the ACP worker. Replaces the previous
/// "stream of text chunks" channel so we can also report tool-call
/// activity (announcements + status/output updates) in chronological
/// order — that order is what makes inline tool-call rendering match what
/// the model actually did.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Full-snapshot replacement of the agent's current plan. ACP's `Plan`
    /// notification is always a full plan (not a delta) per the protocol.
    /// Consumed by the Tasklist sidebar (spec-agent-window.md §21).
    PlanUpdated(Plan),
    /// The agent switched session modes (e.g. between Claude Code's
    /// `default` / `plan` / `learn` modes). Consumed by the Status Strip
    /// (spec-agent-window.md §30).
    ModeChanged(SessionModeId),
    /// The session's active model id (e.g. `claude-opus-4-8`). Sourced from
    /// the `model` entry in the `session/new` response's `config_options`
    /// (the modern `claude-agent-acp` adapter advertises the model selector
    /// there). Consumed by the Status Strip to replace the old best-effort
    /// label. Carries the current model id only; the available-models list
    /// (for an in-app switcher) is a follow-up.
    ModelChanged(String),
    /// The full model picklist advertised by the `model` config-option
    /// `Select`: the current selection plus every selectable model. Sourced
    /// from the same `session/new` / `session/load` / `session/set_config_option`
    /// responses (and `ConfigOptionUpdate` notifications) that drive
    /// `ModelChanged`, but carries the whole option list so the App can render
    /// an in-app model switcher. `current` duplicates the `ModelChanged`
    /// payload emitted alongside it (kept separate so the status-strip label
    /// path is unchanged). Recorded as a plain reply event (no AgentEvent
    /// mapping) so it replays on reconnect without entering the transcript.
    ModelsAvailable {
        current: String,
        options: Vec<ModelOption>,
    },
    /// Updated context-window utilization and cost. Variant is unconditional;
    /// the *emitter* in the notification handler is gated on the upstream
    /// `unstable_session_usage` feature (spec-agent-window.md §31).
    UsageUpdated(UsageSnapshot),
    /// Transient status line, not part of the transcript. Emitted by the
    /// driver loop when a turn hits a retryable API error ("overloaded",
    /// rate limit, …) and is being retried, or when it finally fails. The
    /// App surfaces it in the footer status slot rather than splicing it
    /// into the buffer.
    Notice(String),
    /// A user-authored turn observed on the replay stream (`SessionUpdate::
    /// UserMessageChunk`). Unconditional so the dropped user role is
    /// unrepresentable via match-exhaustiveness: the replay consumer must
    /// decide how to reconstruct it. On session/load the agent re-emits the
    /// whole prior conversation, including the user's own prompts; without
    /// this variant those turns vanish on resume (Finding 1 / defect B,
    /// INV-1, INV-6). The App freezes it as a `TurnId::User(k)` turn, with
    /// a trimmed-suffix dedupe so a *live* echo of a just-submitted prompt
    /// is not double-inserted.
    UserMessage(String),
    /// End-of-replay marker emitted exactly once when `session/load` finishes
    /// its notification burst (Finding 13, INV-4). On resume the agent
    /// re-emits the whole prior conversation as `SessionUpdate` notifications
    /// (the `Chunk` / `UserMessage` events above) and only *then* returns the
    /// `session/load` response — so this event, sent right after that
    /// response, is ordered strictly after the last replayed chunk. The pump
    /// gates `finalize_agent_turn` on it instead of inferring turn-end from a
    /// transiently-empty queue, so finalize runs once after the last replayed
    /// chunk and never mid-replay.
    ReplayComplete,
    /// The authoritative turn boundary: emitted by the worker once the
    /// `session/prompt` RPC resolves (right after `turns.fetch_add`) — the one
    /// point that stands on the real boundary (ADR-0006 / D1, item 8b).
    /// `count` is the post-increment turn count.
    ///
    /// **Additive rollout (this stage):** emitted only when
    /// `YALDA_EMIT_TURN_ENDED=1`, and consumers treat it as inert — the three
    /// pumps still INFER turn-end ("queue empty + counter climbed") and that
    /// inference still drives `finalize`. The consuming arm only logs whether
    /// the explicit signal agrees with the inferred boundary, so agreement can
    /// be confirmed on real sessions (incl. a resume + a tool-only turn) before
    /// the inference is deleted in the final stage. No `generation` field — it's
    /// a server-side respawn counter, stamped where it's authoritative: the
    /// server already carries it on `Notification::TurnEnded { count, generation }`
    /// (A.8a) when it forwards this boundary.
    TurnEnded { count: usize },
}

/// Pure turn-attribution state machine for the replay stream (Findings 3 &
/// 13, INV-3 / INV-4). Lives in the lib crate (rather than open-coded in the
/// bin's `apply_reply_events`) so the exact rules — boundary advance,
/// finalize gate, single source of `k` — are unit-testable; `apply_reply_
/// events` drives this so the bin can't drift from what the tests pin.
///
/// `last_seen` counts settled live turns. `replay_turn` is the replay cursor:
/// 0 outside a replay, otherwise the turn of the most-recent replayed user
/// boundary. The current in-flight turn `k` is `replay_turn` when replaying,
/// else `last_seen + 1` (the turn one past the last settled one) — one source
/// shared by live submit and replay so gutter tags agree in both regimes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReplayTurns {
    pub last_seen: usize,
    pub replay_turn: usize,
}

impl ReplayTurns {
    pub fn new(last_seen: usize) -> Self {
        Self {
            last_seen,
            replay_turn: 0,
        }
    }

    /// The in-flight turn number `k` used to tag chunks/tools. Single source
    /// of `k` for live and replay (INV-3).
    pub fn current_turn(&self) -> usize {
        if self.replay_turn > 0 {
            self.replay_turn
        } else {
            self.last_seen + 1
        }
    }

    /// A replayed user-message boundary opens the next turn. The first
    /// boundary seeds from the live counter; each later one steps `k` by one,
    /// so a 2-user/2-agent replay tags User(1),Llm(1),User(2),Llm(2) rather
    /// than collapsing onto a single turn (INV-3). Returns the new `k`.
    pub fn advance_user_boundary(&mut self) -> usize {
        self.replay_turn = if self.replay_turn == 0 {
            self.last_seen + 1
        } else {
            self.replay_turn + 1
        };
        self.replay_turn
    }

    /// End of replay: fold the replay cursor back into the live counter so
    /// the next live turn resumes from the right `k`, then leave replay mode.
    pub fn finish_replay(&mut self) {
        self.last_seen = self.replay_turn.max(self.last_seen);
        self.replay_turn = 0;
    }
}

/// How yalda responds to `session/request_permission` from the agent.
///
/// The Claude Agent SDK already auto-approves read-only tools (Read, Grep,
/// Glob, LS) without firing a permission request — those work in every
/// mode. This enum only controls what we do when the agent asks to
/// Edit/Write/Move/Delete/Execute/Fetch.
///
/// Stored as `u8` in an [`AtomicU8`] so the worker thread can read it
/// without locking from inside the permission-request callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    /// Parse a mode from a config string. Accepts the `short_label()` forms
    /// plus a few friendly aliases; case-insensitive. Returns `None` for
    /// anything unrecognised so the caller can surface a config error.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "readonly" | "read-only" => Some(Self::ReadOnly),
            "autoedit" | "auto-edit" => Some(Self::AutoEdit),
            "askeachtime" | "ask-each-time" | "ask-each" | "ask" => Some(Self::AskEachTime),
            "yolo" => Some(Self::Yolo),
            _ => None,
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

/// The permission mode a session starts in when nothing overrides it. The
/// default is now `Yolo` (auto-approve every gated tool) — the safe modes
/// proved too annoying without an inline-approval UI. This is config-overridable
/// via `default-permission-mode` in the config file; the safe modes
/// (read-only / auto-edit / ask-each) remain available and become the
/// recommended default once an inline-approval UI lands. The 0600 owner-only
/// socket still gates other local users from reaching the session surface, so
/// auto-approve here does not widen who can drive the agent — only what the
/// owner's own sessions do by default. Flipping this constant changes the
/// hard-coded fallback everywhere; the config node is the user-facing knob.
pub const DEFAULT_PERMISSION_MODE: PermissionMode = PermissionMode::Yolo;

/// Pull the current model id AND the full advertised model picklist out of a
/// `session/new` / `session/load` / `session/set_config_option` response's
/// `config_options` (or a `ConfigOptionUpdate` notification). The model is a
/// `Select` categorised `Model` (id `"model"`); its `.options` enumerate every
/// selectable model, flattened across any groups. Returns `(current_id,
/// options)` or `None` if no model selector is present.
fn model_state_from_config_options(
    opts: &[SessionConfigOption],
) -> Option<(String, Vec<ModelOption>)> {
    opts.iter().find_map(|o| {
        let is_model = matches!(o.category, Some(SessionConfigOptionCategory::Model))
            || o.id.0.as_ref() == "model";
        match (is_model, &o.kind) {
            (true, SessionConfigKind::Select(sel)) => {
                let current = sel.current_value.0.to_string();
                let options = flatten_select_options(&sel.options);
                Some((current, options))
            }
            _ => None,
        }
    })
}

/// Flatten a `Select`'s options (grouped or ungrouped) into a flat
/// `[ModelOption]`, preserving advertised order.
fn flatten_select_options(
    options: &agent_client_protocol::schema::SessionConfigSelectOptions,
) -> Vec<ModelOption> {
    use agent_client_protocol::schema::SessionConfigSelectOptions as O;
    let to_opt = |opt: &agent_client_protocol::schema::SessionConfigSelectOption| ModelOption {
        id: opt.value.0.to_string(),
        label: opt.name.clone(),
    };
    match options {
        O::Ungrouped(list) => list.iter().map(to_opt).collect(),
        O::Grouped(groups) => groups
            .iter()
            .flat_map(|g| g.options.iter().map(to_opt))
            .collect(),
        // `SessionConfigSelectOptions` is `#[non_exhaustive]`; an unknown
        // future shape yields no models rather than failing to compile.
        _ => Vec::new(),
    }
}

/// Build the `(ModelChanged, ModelsAvailable)` reply-event pair from a set of
/// config options, if a model selector is present. Emitting BOTH keeps the
/// existing status-strip `ModelChanged` path untouched while adding the
/// picklist. Returns an empty vec when there is no model selector.
fn model_reply_events(opts: &[SessionConfigOption]) -> Vec<ReplyEvent> {
    match model_state_from_config_options(opts) {
        Some((current, options)) => vec![
            ReplyEvent::ModelChanged(current.clone()),
            ReplyEvent::ModelsAvailable { current, options },
        ],
        None => Vec::new(),
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
/// the first that successfully spawns wins. The only supported adapter is
/// `@agentclientprotocol/claude-agent-acp` (binary `claude-agent-acp`), which
/// bundles the modern Agent SDK 0.3.x runtime (Workflow + the full multi-agent
/// toolset) and honors our `_meta.systemPrompt.append` tuning. The legacy
/// `@zed-industries/claude-code-acp` (binary `claude-code-acp`, SDK 0.2.x) is
/// deliberately NOT a fallback: it ignores the append and runs the untuned
/// SDK prompt, so a stale install must fail loud ("no ACP agent on PATH")
/// rather than silently launch a worse agent.
pub const DEFAULT_AGENT_FALLBACKS: &[&str] = &["claude-agent-acp"];
pub const DEFAULT_CODEX_AGENT_FALLBACKS: &[&str] = &["codex-acp"];

/// Command-name needles identifying an ACP adapter subprocess, for the
/// orphan reaper. Covers the current binary + the legacy one.
pub const ADAPTER_PROCESS_NEEDLES: &[&str] = &["claude-agent-acp", "claude-code-acp", "codex-acp"];

/// Authentication variables Yalda must remove from an ACP adapter's inherited
/// environment. `ANTHROPIC_API_KEY` is always private to Yalda's autonaming
/// request: forwarding it can switch Claude/MCP integrations away from their
/// interactive OAuth credentials. Codex keys retain their existing opt-in.
pub fn agent_auth_env_vars_to_remove(
    provider: AgentProvider,
    allow_codex_api_key: bool,
) -> Vec<&'static str> {
    let mut vars = vec!["ANTHROPIC_API_KEY"];
    if provider == AgentProvider::Codex && !allow_codex_api_key {
        vars.extend(["OPENAI_API_KEY", "CODEX_API_KEY", "DEFAULT_AUTH_REQUEST"]);
    }
    vars
}

/// Parse `ps -axo pid=,ppid=,command=` output and return the PIDs of ORPHANED
/// ACP adapter processes — those whose parent is PID 1 (the spawner died and the
/// kernel reparented them) AND whose command matches an adapter needle. Pure +
/// testable: the side-effecting reaper feeds it real `ps` output.
///
/// `ppid == 1` is the safety property: an orphan has no live yalda owning it, so
/// killing it can never touch a running session's adapter. (A live adapter's
/// parent is its yalda-gpui / yalda-session-server process, never 1.)
pub fn orphaned_adapter_pids(ps_output: &str, needles: &[&str]) -> Vec<i32> {
    let mut pids = Vec::new();
    for line in ps_output.lines() {
        let mut it = line.split_whitespace();
        let pid = it.next().and_then(|s| s.parse::<i32>().ok());
        let ppid = it.next().and_then(|s| s.parse::<i32>().ok());
        let (Some(pid), Some(ppid)) = (pid, ppid) else {
            continue;
        };
        if ppid != 1 || pid <= 1 {
            continue;
        }
        if needles.iter().any(|n| line.contains(n)) {
            pids.push(pid);
        }
    }
    pids
}

/// Best-effort startup reaper: SIGKILL ACP adapter subprocesses orphaned by a
/// crashed/killed parent (a graceful exit already reaps them via
/// `kill_on_drop`; a SIGKILL/panic does not, leaving the adapter reparented to
/// PID 1 — the observed ~70-adapter accumulation). Call once at the start of
/// `main()` in both binaries. Unix-only; a no-op (returns 0) elsewhere or if
/// `ps` is unavailable. Returns the number killed.
#[cfg(unix)]
pub fn reap_orphaned_adapters() -> usize {
    let output = match std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return 0,
    };
    let text = String::from_utf8_lossy(&output);
    let pids = orphaned_adapter_pids(&text, ADAPTER_PROCESS_NEEDLES);
    for &pid in &pids {
        // SAFETY: a plain kill(2) syscall with a validated positive pid.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    pids.len()
}

#[cfg(not(unix))]
pub fn reap_orphaned_adapters() -> usize {
    0
}

/// How long to wait for `session/load` (resume) before giving up and spawning a
/// fresh `session/new`. `session/load` returns only AFTER the agent re-emits the
/// session's ENTIRE prior conversation as `session/update` notifications, so a
/// big session (verified: ~700 replayed events) legitimately takes a while —
/// the timeout must be generous enough to let such a replay COMPLETE so the
/// resumed context is kept, and only fire on a TRUE hang (observed: a stale id
/// left the adapter stuck in `session/load` for 20+ min). 5 minutes comfortably
/// clears even a very large progressing replay while still bounding a real hang
/// to one bounded wait per (re)spawn. (An idle/progress-based timeout would be
/// tighter, but the load future is opaque here; a generous total bound is the
/// safe, simple choice that never falsely discards a recoverable session.)
const SESSION_LOAD_TIMEOUT_SECS: u64 = 300;
/// Child-thread inspection is user-initiated and read-only. It must fail fast
/// enough to leave the UI usable; unlike durable session recovery it never
/// falls back to creating a new session.
const INSPECT_SESSION_LOAD_TIMEOUT_SECS: u64 = 30;

/// Constructor seam for an [`AgentTransport`] (Phase 6, spec-session-server-actor
/// §Rollout). The pump thread never builds the client — the session-server's
/// three spawn workers (create / restart / resume) do. Abstracting *spawning*
/// behind this object-safe factory lets a test inject an in-process fake without
/// touching the `YALDA_ACP_AGENT` env or forking the real binary, while the
/// production path stays byte-for-byte identical via [`RealAgentSpawner`].
///
/// `Send + Sync` so it can live behind an `Arc<dyn AgentSpawner>` shared by the
/// actor and cloned into each (off-actor) spawn thread.
pub trait AgentSpawner: Send + Sync {
    /// Spawn (or resume) an agent and complete its blocking handshake, returning
    /// the owning transport. Mirrors [`AcpChannelClient::spawn_with_resume_in`]:
    /// `command` empty ⇒ default fallback chain; `resume` `Some` ⇒ `session/load`.
    /// Runs on a dedicated OS spawn thread (the handshake blocks), never the actor.
    fn spawn(
        &self,
        provider: AgentProvider,
        command: &str,
        cwd: Option<PathBuf>,
        resume: Option<String>,
        frontend: YaldaFrontend,
    ) -> io::Result<Box<dyn AgentTransport>>;
}

/// Production [`AgentSpawner`]: forwards to [`AcpChannelClient::spawn_with_resume_in`]
/// and boxes the resulting real client. Zero behaviour change — this is the only
/// spawner the shipping binary installs.
pub struct RealAgentSpawner;

impl AgentSpawner for RealAgentSpawner {
    fn spawn(
        &self,
        provider: AgentProvider,
        command: &str,
        cwd: Option<PathBuf>,
        resume: Option<String>,
        frontend: YaldaFrontend,
    ) -> io::Result<Box<dyn AgentTransport>> {
        AcpChannelClient::spawn_with_resume_in_for(provider, command, cwd, resume, frontend)
            .map(|c| Box::new(c) as Box<dyn AgentTransport>)
    }
}

// ── In-process fake transport (Phase 6 test substrate) ─────────────────────
//
// Gated on `test-support` so it ships in test builds only. The fake reproduces
// the worker's framing/ordering using the SAME channel + atomic types the real
// client holds (`std::sync::mpsc::Receiver<ReplyEvent>` + the four shared
// atomics), so the pump's `try_recv()` / `turn_count()` / `is_connected()` reads
// see identical semantics. The ONLY thing it skips is the subprocess /
// JSON-RPC serialization — `ReplyEvent` is already the post-deserialize currency
// the pump consumes, so the fake injects at exactly the layer where real and
// fake are indistinguishable to the pump. It therefore proves reducer /
// forwarder / pump logic — NOT wire framing (which the real-agent transcript
// tests still cover).
#[cfg(any(test, feature = "test-support"))]
mod fake {
    use super::*;
    use futures::channel::mpsc as f_mpsc;

    /// In-process [`AgentTransport`] standing in for [`AcpChannelClient`]: no OS
    /// process, no ACP protocol — events arrive on a plain `std::sync::mpsc`
    /// channel a test scenario drives via [`FakeAgentControls`].
    pub struct FakeTransport {
        reply_rx: std_mpsc::Receiver<ReplyEvent>,
        connected: Arc<AtomicBool>,
        turns: Arc<AtomicUsize>,
        permission_mode: Arc<AtomicU8>,
        session_id: Arc<std::sync::Mutex<Option<String>>>,
        steering_supported: Arc<AtomicBool>,
        prompt_tx: std_mpsc::Sender<PromptPayload>,
        steer_tx: std_mpsc::Sender<NativeSteeringCommand>,
        set_model_tx: std_mpsc::Sender<String>,
        cancel_tx: f_mpsc::UnboundedSender<()>,
    }

    /// Drive-side controls for a [`FakeTransport`]: push events, advance the turn
    /// counter, flip liveness, and observe outbound prompts/cancels. Holds clones
    /// of the same atomics + the sender end of the reply channel, mirroring how a
    /// real agent's notification handler feeds the pump.
    pub struct FakeAgentControls {
        reply_tx: std_mpsc::Sender<ReplyEvent>,
        connected: Arc<AtomicBool>,
        turns: Arc<AtomicUsize>,
        permission_mode: Arc<AtomicU8>,
        session_id: Arc<std::sync::Mutex<Option<String>>>,
        /// Receiver for prompts the transport side enqueues — lets a scenario
        /// assert a prompt arrived (e.g. admin_prompt) before auto-emitting a turn.
        pub prompt_rx: std_mpsc::Receiver<PromptPayload>,
        /// Receiver for native steering payloads.
        steer_rx: std_mpsc::Receiver<NativeSteeringCommand>,
        /// Receiver for model-switch requests the transport side enqueues — lets
        /// a scenario assert `set_model` reached the transport.
        pub set_model_rx: std_mpsc::Receiver<String>,
        /// Receiver for cancel signals.
        pub cancel_rx: f_mpsc::UnboundedReceiver<()>,
    }

    impl FakeTransport {
        /// Build a fake transport paired with its drive-side controls. The
        /// session id is pre-populated (mirroring how the real worker fills it
        /// after `session/new`) so `handle().session_id()` and restart's
        /// resume-id computation behave like the real path.
        pub fn new() -> (FakeTransport, FakeAgentControls) {
            Self::with_session_id("fake-sess-0001")
        }

        /// Like [`new`] but with a caller-chosen synthetic session id.
        pub fn with_session_id(sid: &str) -> (FakeTransport, FakeAgentControls) {
            Self::with_session_id_and_steering(sid, false)
        }

        /// Native-steering-capable fake for ordering regressions.
        pub fn with_session_id_and_steering(
            sid: &str,
            steering_supported: bool,
        ) -> (FakeTransport, FakeAgentControls) {
            let (reply_tx, reply_rx) = std_mpsc::channel::<ReplyEvent>();
            let (prompt_tx, prompt_rx) = std_mpsc::channel::<PromptPayload>();
            let (steer_tx, steer_rx) = std_mpsc::channel::<NativeSteeringCommand>();
            let (set_model_tx, set_model_rx) = std_mpsc::channel::<String>();
            let (cancel_tx, cancel_rx) = f_mpsc::unbounded::<()>();
            let connected = Arc::new(AtomicBool::new(true));
            let turns = Arc::new(AtomicUsize::new(0));
            let permission_mode = Arc::new(AtomicU8::new(DEFAULT_PERMISSION_MODE as u8));
            let session_id = Arc::new(std::sync::Mutex::new(Some(sid.to_string())));
            let steering_supported = Arc::new(AtomicBool::new(steering_supported));

            let transport = FakeTransport {
                reply_rx,
                connected: Arc::clone(&connected),
                turns: Arc::clone(&turns),
                permission_mode: Arc::clone(&permission_mode),
                session_id: Arc::clone(&session_id),
                steering_supported,
                prompt_tx,
                steer_tx,
                set_model_tx,
                cancel_tx,
            };
            let controls = FakeAgentControls {
                reply_tx,
                connected,
                turns,
                permission_mode,
                session_id,
                prompt_rx,
                steer_rx,
                set_model_rx,
                cancel_rx,
            };
            (transport, controls)
        }
    }

    impl AgentTransport for FakeTransport {
        fn try_recv(&self) -> Option<ReplyEvent> {
            self.reply_rx.try_recv().ok()
        }
        fn is_connected(&self) -> bool {
            self.connected.load(Ordering::SeqCst)
        }
        fn turn_count(&self) -> usize {
            self.turns.load(Ordering::SeqCst)
        }
        fn handle(&self) -> TransportHandle {
            TransportHandle {
                prompt_tx: self.prompt_tx.clone(),
                steer_tx: self.steer_tx.clone(),
                set_model_tx: self.set_model_tx.clone(),
                cancel_tx: self.cancel_tx.clone(),
                connected: Arc::clone(&self.connected),
                turns: Arc::clone(&self.turns),
                permission_mode: Arc::clone(&self.permission_mode),
                session_id: Arc::clone(&self.session_id),
                steering_supported: Arc::clone(&self.steering_supported),
                generation: 0,
            }
        }
        fn session_id(&self) -> Option<String> {
            self.session_id.lock().ok().and_then(|g| g.clone())
        }
    }

    impl FakeAgentControls {
        /// Consume the next ordered native-control item as a steer.
        pub fn try_recv_native_steer(&mut self) -> Option<PromptPayload> {
            match self.steer_rx.try_recv() {
                Ok(NativeSteeringCommand::Steer(payload)) => Some(payload),
                Ok(NativeSteeringCommand::Prompt(_)) => {
                    panic!("expected native steer, got ordered prompt")
                }
                Ok(NativeSteeringCommand::Cancel) => {
                    panic!("expected native steer, got ordered cancel")
                }
                Err(_) => None,
            }
        }

        /// Consume the next ordered native-control item as an explicit Stop.
        pub fn try_recv_native_cancel(&mut self) -> bool {
            match self.steer_rx.try_recv() {
                Ok(NativeSteeringCommand::Cancel) => true,
                Ok(NativeSteeringCommand::Prompt(_)) => {
                    panic!("expected ordered cancel, got ordered prompt")
                }
                Ok(NativeSteeringCommand::Steer(_)) => {
                    panic!("expected ordered cancel, got native steer")
                }
                Err(_) => false,
            }
        }

        /// Consume the next ordered native-control item as the initial prompt.
        pub fn try_recv_native_prompt(&mut self) -> Option<PromptPayload> {
            match self.steer_rx.try_recv() {
                Ok(NativeSteeringCommand::Prompt(payload)) => Some(payload),
                Ok(NativeSteeringCommand::Steer(_)) => {
                    panic!("expected ordered prompt, got native steer")
                }
                Ok(NativeSteeringCommand::Cancel) => {
                    panic!("expected ordered prompt, got ordered cancel")
                }
                Err(_) => None,
            }
        }

        /// Push one reply event onto the transport's inbound stream. FIFO order is
        /// preserved exactly as the real notification-handler → pump path does.
        pub fn push(&self, event: ReplyEvent) {
            let _ = self.reply_tx.send(event);
        }

        /// Push a streamed text chunk (convenience for `push(ReplyEvent::Chunk)`).
        pub fn push_chunk(&self, text: &str) {
            self.push(ReplyEvent::Chunk(text.to_string()));
        }

        /// Mirror the real DEFAULT worker turn boundary: bump the turn counter
        /// ONLY. The default worker does NOT push `ReplyEvent::TurnEnded` into the
        /// reply stream — that variant is gated behind `YALDA_EMIT_TURN_ENDED=1`
        /// and is inert by default; the pump detects the boundary purely via
        /// `turn_count() > last_turns`. Emitting a TurnEnded here would make a
        /// fake-driven turn produce an extra eventlog record the production path
        /// never emits (false confidence for reducer/forwarder tests). Use
        /// [`emit_turn_ended_event`](Self::emit_turn_ended_event) to exercise the
        /// opt-in `YALDA_EMIT_TURN_ENDED=1` mode.
        pub fn complete_turn(&self) {
            self.turns.fetch_add(1, Ordering::SeqCst);
        }

        /// Opt-in: reproduce the `YALDA_EMIT_TURN_ENDED=1` worker mode — bump the
        /// turn counter AND push a `TurnEnded{count}` event into the reply stream.
        /// Only for scenarios deliberately exercising that gated path; the default
        /// boundary is [`complete_turn`](Self::complete_turn) (counter-only).
        pub fn emit_turn_ended_event(&self) {
            let count = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
            self.push(ReplyEvent::TurnEnded { count });
        }

        /// Flip liveness false (worker EOF/exit) to drive the `AgentDisconnected`
        /// path the pump emits when `is_connected()` goes false.
        pub fn disconnect(&self) {
            self.connected.store(false, Ordering::SeqCst);
        }

        /// Read the current permission policy the actor pushed via the handle.
        pub fn permission_mode(&self) -> PermissionMode {
            PermissionMode::from_u8(self.permission_mode.load(Ordering::SeqCst))
        }

        /// Overwrite the synthetic session id (e.g. to simulate a resume landing
        /// on a different id).
        pub fn set_session_id(&self, sid: Option<String>) {
            if let Ok(mut g) = self.session_id.lock() {
                *g = sid;
            }
        }

        /// Non-blocking pull of the next prompt the transport enqueued, if any.
        pub fn try_recv_prompt(&self) -> Option<PromptPayload> {
            self.prompt_rx.try_recv().ok()
        }
    }

    /// A pluggable [`AgentSpawner`] whose `spawn` returns a pre-built
    /// [`FakeTransport`] (no subprocess). The factory closure is invoked per
    /// spawn so a scenario can hand out a fresh fake (and capture its controls)
    /// or fail on demand to exercise the `SpawnFailed` branch.
    pub struct FakeAgentSpawner {
        #[allow(clippy::type_complexity)]
        factory: std::sync::Mutex<
            Box<
                dyn FnMut(
                        &str,
                        Option<PathBuf>,
                        Option<String>,
                    ) -> io::Result<Box<dyn AgentTransport>>
                    + Send,
            >,
        >,
    }

    impl FakeAgentSpawner {
        /// Build a spawner from a factory closure. The closure receives the same
        /// (command, cwd, resume) the real spawner would and returns either a
        /// boxed transport or an `io::Error` (to drive `SpawnFailed`).
        pub fn new<F>(factory: F) -> Self
        where
            F: FnMut(&str, Option<PathBuf>, Option<String>) -> io::Result<Box<dyn AgentTransport>>
                + Send
                + 'static,
        {
            Self {
                factory: std::sync::Mutex::new(Box::new(factory)),
            }
        }
    }

    impl AgentSpawner for FakeAgentSpawner {
        fn spawn(
            &self,
            _provider: AgentProvider,
            command: &str,
            cwd: Option<PathBuf>,
            resume: Option<String>,
            _frontend: YaldaFrontend,
        ) -> io::Result<Box<dyn AgentTransport>> {
            let mut f = self
                .factory
                .lock()
                .map_err(|_| io::Error::other("fake spawner poisoned"))?;
            f(command, cwd, resume)
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeAgentControls, FakeAgentSpawner, FakeTransport};

/// A live ACP connection to a locally-spawned agent subprocess.
///
/// API mirrors `claude_channel::ChannelClient` so the frontend can drive
/// either by trait-like sniffing without inheriting any of the protocol
/// details.
pub struct AcpChannelClient {
    /// Outbound prompts: `App::claude_acp_send_text` → worker.
    prompt_tx: std_mpsc::Sender<PromptPayload>,
    /// Outbound capable-Codex prompts, native steering, and Stop controls.
    /// One stream preserves the user's action order while prompt responses are
    /// awaited independently.
    steer_tx: std_mpsc::Sender<NativeSteeringCommand>,
    /// Outbound model switches: each model id pushed here makes the worker
    /// driver issue an ACP `session/set_config_option` for the `model`
    /// option. Separate from `prompt_tx` so a switch never rides the prompt
    /// queue (it applies out-of-band, mid-turn if needed).
    set_model_tx: std_mpsc::Sender<String>,
    /// Cancel signal: `App::stop_agent` → worker driver loop. Each `()`
    /// pushed here makes the driver send an ACP `session/cancel` for the
    /// in-flight turn. `unbounded_send` is callable from the sync App side
    /// without an async context, mirroring [`wake_rx`].
    cancel_tx: futures::channel::mpsc::UnboundedSender<()>,
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
    /// yalda can resume the same Claude session via `session/load`.
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
    /// Capability advertised at initialize as root `_meta.steering.supported`.
    steering_supported: Arc<AtomicBool>,
    /// Joined on Drop so the worker has a chance to clean up the runtime
    /// (kill the child, drop streams) before yalda exits.
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
    /// name "just work" without setting `YALDA_ACP_AGENT`.
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
        Self::spawn_with_resume_in(command_str, cwd, resume_session_id, YaldaFrontend::Gpui)
    }

    /// Frontend-aware variant of [`spawn_with_resume`]. The `frontend`
    /// argument is woven into the system-prompt append so Claude knows
    /// which yalda host is driving it. All other behaviour is identical to
    /// [`spawn_with_resume`].
    pub fn spawn_with_resume_in(
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: YaldaFrontend,
    ) -> io::Result<Self> {
        Self::spawn_with_resume_in_for(
            AgentProvider::Claude,
            command_str,
            cwd,
            resume_session_id,
            frontend,
        )
    }

    /// Provider-aware spawn used by the session server. The legacy public
    /// helpers above remain Claude defaults for direct-mode compatibility.
    pub fn spawn_with_resume_in_for(
        provider: AgentProvider,
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: YaldaFrontend,
    ) -> io::Result<Self> {
        Self::spawn_with_resume_policy_in_for(
            provider,
            command_str,
            cwd,
            resume_session_id,
            frontend,
            false,
        )
    }

    /// Open an existing ACP session for read-only replay without ever falling
    /// back to `session/new`. This is used for Codex child-agent threads: a
    /// stale child id should show "unavailable", not silently create an empty
    /// replacement thread merely because the inspector tried to open it.
    pub fn spawn_resume_only_in_for(
        provider: AgentProvider,
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: String,
        frontend: YaldaFrontend,
    ) -> io::Result<Self> {
        Self::spawn_with_resume_policy_in_for(
            provider,
            command_str,
            cwd,
            Some(resume_session_id),
            frontend,
            true,
        )
    }

    fn spawn_with_resume_policy_in_for(
        provider: AgentProvider,
        command_str: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: YaldaFrontend,
        resume_only: bool,
    ) -> io::Result<Self> {
        let candidates: Vec<String> = if command_str.trim().is_empty() {
            let fallbacks = match provider {
                AgentProvider::Claude => DEFAULT_AGENT_FALLBACKS,
                AgentProvider::Codex => DEFAULT_CODEX_AGENT_FALLBACKS,
            };
            fallbacks
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
            match Self::try_spawn(
                provider,
                &command,
                cwd.clone(),
                resume_session_id.clone(),
                frontend,
                resume_only,
            ) {
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
                match Self::try_spawn(
                    provider,
                    &resolved,
                    cwd.clone(),
                    resume_session_id.clone(),
                    frontend,
                    resume_only,
                ) {
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
                "no {} ACP agent on PATH (tried {}). Install with `{}`, or set {}=/path/to/agent. Last error: {}",
                provider.label(),
                tried.join(", "),
                provider.install_hint(),
                match provider {
                    AgentProvider::Claude => "YALDA_CLAUDE_ACP_AGENT",
                    AgentProvider::Codex => "YALDA_CODEX_ACP_AGENT",
                },
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
        provider: AgentProvider,
        command: &str,
        cwd: Option<PathBuf>,
        resume_session_id: Option<String>,
        frontend: YaldaFrontend,
        resume_only: bool,
    ) -> io::Result<Self> {
        let cwd =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

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

        let (prompt_tx, prompt_rx) = std_mpsc::channel::<PromptPayload>();
        let (steer_tx, steer_rx) = std_mpsc::channel::<NativeSteeringCommand>();
        let (set_model_tx, set_model_rx) = std_mpsc::channel::<String>();
        let (reply_tx, reply_rx) = std_mpsc::channel::<ReplyEvent>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<io::Result<()>>();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_for_worker = connected.clone();
        let turns = Arc::new(AtomicUsize::new(0));
        let turns_for_worker = turns.clone();
        let session_id: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let session_id_for_worker = session_id.clone();
        // Sessions begin in DEFAULT_PERMISSION_MODE (now Yolo — auto-approve,
        // so the agent runs its full edit→build→test loop without prompts).
        // The effective default is config-driven on the server side
        // (`default-permission-mode` in config.kdl); this constant is the
        // no-config fallback. The user de-escalates to a safe mode
        // (read-only / auto-edit / ask-each) with the mode toggle
        // (`<space> c m`). The 0600 socket remains the gate against other
        // local users.
        let permission_mode = Arc::new(AtomicU8::new(DEFAULT_PERMISSION_MODE as u8));
        let steering_supported = Arc::new(AtomicBool::new(false));
        let permission_mode_for_worker = permission_mode.clone();
        let steering_supported_for_worker = steering_supported.clone();

        // Wake channel: the worker pushes `()` every time it forwards a
        // reply event, so the foreground pump task can `select!` on it
        // and wake immediately when a chunk arrives — instead of waiting
        // for the next polling tick. Receiver is taken (once) by the
        // pump after attach succeeds.
        let (wake_tx, wake_rx) = futures::channel::mpsc::unbounded::<()>();

        // Cancel channel: the App pushes `()` to interrupt the in-flight
        // turn; the worker's driver loop selects on the receiver and sends
        // ACP `session/cancel`.
        let (cancel_tx, cancel_rx) = futures::channel::mpsc::unbounded::<()>();

        let worker_cwd = cwd.clone();
        let worker = thread::Builder::new()
            .name("yalda-acp-worker".into())
            .spawn(move || {
                run_worker(
                    parts,
                    worker_cwd,
                    prompt_rx,
                    steer_rx,
                    set_model_rx,
                    reply_tx,
                    ready_tx,
                    connected_for_worker,
                    turns_for_worker,
                    session_id_for_worker,
                    resume_session_id,
                    permission_mode_for_worker,
                    steering_supported_for_worker,
                    wake_tx,
                    cancel_rx,
                    frontend,
                    provider,
                    resume_only,
                );
            })?;

        // Wait for the initialize+new-session handshake to either succeed or
        // fail. We drop the channel afterwards — readiness is signalled once.
        let initial = ready_rx
            .recv()
            .map_err(|_| io::Error::other("acp worker exited before reporting readiness"))?;
        if let Err(e) = initial {
            // The worker has bailed; tear it down before returning.
            connected.store(false, Ordering::SeqCst);
            let _ = worker.join();
            return Err(e);
        }

        Ok(Self {
            prompt_tx,
            steer_tx,
            set_model_tx,
            cancel_tx,
            reply_rx,
            connected,
            turns,
            session_id,
            permission_mode,
            steering_supported,
            wake_rx: std::sync::Mutex::new(Some(wake_rx)),
            worker: Some(worker),
            command: command.to_string(),
            cwd,
        })
    }

    /// Take the wake-channel receiver. Returns `Some` exactly once per
    /// client; subsequent calls return `None`. Caller (typically the
    /// GPUI pump task) uses it to await events without polling.
    pub fn take_wake_receiver(&self) -> Option<futures::channel::mpsc::UnboundedReceiver<()>> {
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
    /// across yalda runs so a future invocation can pick up the same
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

    /// Send a text prompt to the agent. Returns Err if the worker has died
    /// (e.g. the child crashed) so the caller can drop the connection.
    pub fn send(&mut self, prompt: &str) -> io::Result<()> {
        self.send_payload(PromptPayload::text(prompt))
    }

    /// Send a prompt carrying image attachments (pasted images) alongside the
    /// text. The worker builds a mixed `[Text, Image, …]` content-block vector.
    pub fn send_payload(&mut self, payload: PromptPayload) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        let result = if self.supports_steering() {
            self.steer_tx.send(NativeSteeringCommand::Prompt(payload))
        } else {
            self.prompt_tx
                .send(payload)
                .map_err(|error| std_mpsc::SendError(NativeSteeringCommand::Prompt(error.0)))
        };
        result.map_err(|_| {
            self.connected.store(false, Ordering::SeqCst);
            io::Error::new(io::ErrorKind::BrokenPipe, "ACP worker channel closed")
        })
    }

    /// Send one native steering request when the adapter advertised support.
    /// The worker-side bridge is installed by the production steering change;
    /// this synchronous enqueue surface also gives the GUI harness an exact
    /// observation point for routing regressions.
    pub fn steer_payload(&self, payload: PromptPayload) -> io::Result<()> {
        if !self.steering_supported.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ACP agent does not advertise native steering",
            ));
        }
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        self.steer_tx
            .send(NativeSteeringCommand::Steer(payload))
            .map_err(|_| {
                self.connected.store(false, Ordering::SeqCst);
                io::Error::new(io::ErrorKind::BrokenPipe, "ACP steering channel closed")
            })
    }

    pub fn supports_steering(&self) -> bool {
        self.steering_supported.load(Ordering::SeqCst)
    }

    /// Provider-aware Codex follow-up delivery. Capable adapters receive the
    /// native FIFO steering request; older adapters retain the shipped
    /// graceful-cancel + replacement-prompt compatibility behavior.
    pub fn steer_or_replace_payload(&mut self, payload: PromptPayload) -> io::Result<()> {
        if self.supports_steering() {
            self.steer_payload(payload)
        } else {
            self.cancel();
            self.send_payload(payload)
        }
    }

    /// Request cancellation of the in-flight turn. Sends ACP
    /// `session/cancel` to the agent, which resolves the current
    /// `session/prompt` with `StopReason::Cancelled` — ending the turn
    /// without killing the session (unlike `Drop`). Best-effort: a no-op if
    /// the worker has already exited or nothing is in flight.
    pub fn cancel(&self) {
        if self.supports_steering() {
            let _ = self.steer_tx.send(NativeSteeringCommand::Cancel);
        } else {
            let _ = self.cancel_tx.unbounded_send(());
        }
    }

    /// Switch the session's model. Enqueues `model_id` for the worker driver,
    /// which issues a `session/set_config_option` for the `model` option. The
    /// agent applies it live (subsequent turns use the new model) and echoes
    /// the updated selector back as `ModelChanged` + `ModelsAvailable` reply
    /// events. Best-effort: a no-op if the worker channel has closed.
    pub fn set_model(&self, model_id: &str) {
        let _ = self.set_model_tx.send(model_id.to_string());
    }

    /// Pull one queued reply event (text chunk or tool-call activity) if
    /// any are pending. Non-blocking — safe to call every tick.
    pub fn try_recv(&self) -> Option<ReplyEvent> {
        self.reply_rx.try_recv().ok()
    }

    /// Derive a [`TransportHandle`] — the `Send` (+`Sync`) subset of this
    /// client's surface — by cloning the sub-handles that don't include the
    /// `!Sync` `reply_rx`. The reply receiver stays owned by the pump thread
    /// that owns the whole client; the actor stores only this handle in its map
    /// and never touches (or drops) the client itself.
    ///
    /// `generation` defaults to 0; the publishing worker/actor stamps the real
    /// generation onto the handle before installing it.
    pub fn handle(&self) -> TransportHandle {
        TransportHandle {
            prompt_tx: self.prompt_tx.clone(),
            steer_tx: self.steer_tx.clone(),
            set_model_tx: self.set_model_tx.clone(),
            cancel_tx: self.cancel_tx.clone(),
            connected: Arc::clone(&self.connected),
            turns: Arc::clone(&self.turns),
            permission_mode: Arc::clone(&self.permission_mode),
            session_id: Arc::clone(&self.session_id),
            steering_supported: Arc::clone(&self.steering_supported),
            generation: 0,
        }
    }

    /// Build an in-process, subprocess-free client for tests. `connected = true`
    /// and the returned [`TestChannelControls`] RETAINS the `prompt_rx` so
    /// [`send`](Self::send) succeeds without a worker (a dropped receiver would
    /// make the sender error and mark the channel disconnected). This is the seam
    /// that closes verification gap #2 for the GUI half: a real `submit` now takes
    /// the `channel.send() == Ok` path (`send_prompt_to_session`), so the REAL
    /// user-turn insert + `turn_phase = begin` transition runs through production
    /// code — instead of tests hand-setting `turn_phase`. Turn OUTPUT (chunks,
    /// `TurnEnded`) is still driven through the real reducer via
    /// `apply_server_batch`, not this channel.
    #[cfg(feature = "test-support")]
    pub fn test_connected() -> (Self, TestChannelControls) {
        Self::test_connected_with_steering(false)
    }

    /// Native-steering-capable variant of [`test_connected`].
    #[cfg(feature = "test-support")]
    pub fn test_connected_with_steering(steering_supported: bool) -> (Self, TestChannelControls) {
        let (prompt_tx, prompt_rx) = std_mpsc::channel::<PromptPayload>();
        let (steer_tx, steer_rx) = std_mpsc::channel::<NativeSteeringCommand>();
        let (set_model_tx, set_model_rx) = std_mpsc::channel::<String>();
        let (reply_tx, reply_rx) = std_mpsc::channel::<ReplyEvent>();
        let (_wake_tx, wake_rx) = futures::channel::mpsc::unbounded::<()>();
        let (cancel_tx, cancel_rx) = futures::channel::mpsc::unbounded::<()>();
        let connected = Arc::new(AtomicBool::new(true));
        let client = Self {
            prompt_tx,
            steer_tx,
            set_model_tx,
            cancel_tx,
            reply_rx,
            connected: Arc::clone(&connected),
            turns: Arc::new(AtomicUsize::new(0)),
            session_id: Arc::new(std::sync::Mutex::new(None)),
            permission_mode: Arc::new(AtomicU8::new(DEFAULT_PERMISSION_MODE as u8)),
            steering_supported: Arc::new(AtomicBool::new(steering_supported)),
            wake_rx: std::sync::Mutex::new(Some(wake_rx)),
            worker: None,
            command: "test-in-process".to_string(),
            cwd: PathBuf::from("."),
        };
        (
            client,
            TestChannelControls {
                prompt_rx,
                steer_rx,
                set_model_rx,
                reply_tx,
                cancel_rx,
                connected,
            },
        )
    }
}

/// Handles the test must keep alive for a [`AcpChannelClient::test_connected`]
/// client to keep working, plus levers to simulate transport events.
#[cfg(feature = "test-support")]
pub struct TestChannelControls {
    /// Retained so `send()` succeeds; drain it to assert what was submitted.
    pub prompt_rx: std_mpsc::Receiver<PromptPayload>,
    /// Ordered native steering/Stop controls emitted by a capable transport.
    steer_rx: std_mpsc::Receiver<NativeSteeringCommand>,
    /// Retained so `set_model()` succeeds without a worker; drain it (via
    /// [`try_recv_set_model`](Self::try_recv_set_model)) to assert a model
    /// switch reached the channel.
    set_model_rx: std_mpsc::Receiver<String>,
    /// Inject `ReplyEvent`s (chunks / `TurnEnded`) the pump will read via
    /// `try_recv` — the seam for driving a turn to completion in-process.
    pub reply_tx: std_mpsc::Sender<ReplyEvent>,
    /// Kept alive so `cancel()` doesn't error and exposed through
    /// [`try_recv_cancel`](Self::try_recv_cancel) so GUI tests can prove a real
    /// submit reached the production cancellation transport.
    cancel_rx: futures::channel::mpsc::UnboundedReceiver<()>,
    /// Flip to `false` to simulate the worker dying (EOF) — the next `send()`
    /// then fails, exercising the "send failed — reconnecting" path.
    pub connected: Arc<AtomicBool>,
}

#[cfg(feature = "test-support")]
impl TestChannelControls {
    /// Consume the next ordered native-control item as a steer.
    pub fn try_recv_native_steer(&mut self) -> Option<PromptPayload> {
        match self.steer_rx.try_recv() {
            Ok(NativeSteeringCommand::Steer(payload)) => Some(payload),
            Ok(NativeSteeringCommand::Prompt(_)) => {
                panic!("expected native steer, got ordered prompt")
            }
            Ok(NativeSteeringCommand::Cancel) => {
                panic!("expected native steer, got ordered cancel")
            }
            Err(_) => None,
        }
    }

    /// Consume the next ordered native-control item as an explicit Stop.
    pub fn try_recv_native_cancel(&mut self) -> bool {
        match self.steer_rx.try_recv() {
            Ok(NativeSteeringCommand::Cancel) => true,
            Ok(NativeSteeringCommand::Prompt(_)) => {
                panic!("expected ordered cancel, got ordered prompt")
            }
            Ok(NativeSteeringCommand::Steer(_)) => {
                panic!("expected ordered cancel, got native steer")
            }
            Err(_) => false,
        }
    }

    /// Consume the next ordered native-control item as the initial prompt.
    pub fn try_recv_native_prompt(&mut self) -> Option<PromptPayload> {
        match self.steer_rx.try_recv() {
            Ok(NativeSteeringCommand::Prompt(payload)) => Some(payload),
            Ok(NativeSteeringCommand::Steer(_)) => {
                panic!("expected ordered prompt, got native steer")
            }
            Ok(NativeSteeringCommand::Cancel) => {
                panic!("expected ordered prompt, got ordered cancel")
            }
            Err(_) => None,
        }
    }

    /// Non-blocking pull of the next model id a `set_model()` enqueued, if any.
    pub fn try_recv_set_model(&self) -> Option<String> {
        self.set_model_rx.try_recv().ok()
    }

    /// Non-blocking observation of one graceful ACP cancel request.
    pub fn try_recv_cancel(&mut self) -> bool {
        self.cancel_rx.try_recv().is_ok()
    }
}

/// The pump-thread-facing surface of an agent connection (Phase 6,
/// spec-session-server-actor §Rollout). This is *exactly* the set of touchpoints
/// the session-server's pump thread uses against the spawned client today, made
/// object-safe so the pump can own a `Box<dyn AgentTransport>` and a test can
/// substitute an in-process fake for the subprocess-backed [`AcpChannelClient`].
///
/// Deliberately minimal:
/// - `try_recv` / `is_connected` / `turn_count` / `session_id` are the loop's
///   reads; `handle` derives the actor-facing [`TransportHandle`].
/// - `send` / `cancel` / `set_permission_mode` are NOT here — those are reached
///   through `TransportHandle` (the actor side); `take_wake_receiver` is GUI-only.
/// - No `async` (the pump drains synchronously) and no `Self`-returning methods,
///   so the trait stays object-safe.
/// - No explicit `shutdown`: teardown is `Drop` (blocking — kill child, join
///   worker). The pump's final `drop(client)` works unchanged whether the boxed
///   concrete type is the real client or a fake.
///
/// `Send` (the pump moves the box onto its OS thread) but intentionally NOT
/// `Sync`: it preserves today's invariant that only the cloned [`TransportHandle`]
/// is `Sync`, while the receiver-owning object stays single-owner on the pump.
pub trait AgentTransport: Send {
    /// Pull one queued reply event if any are pending. Non-blocking; the pump
    /// drains this in a budgeted loop. Identical semantics to
    /// [`AcpChannelClient::try_recv`].
    fn try_recv(&self) -> Option<ReplyEvent>;
    /// Worker liveness. The pump emits `AgentDisconnected` when this flips false.
    fn is_connected(&self) -> bool;
    /// Completed-turn count, compared against the pump's `last_turns` to detect a
    /// turn boundary.
    fn turn_count(&self) -> usize;
    /// Derive the `Send + Sync` actor-facing [`TransportHandle`].
    fn handle(&self) -> TransportHandle;
    /// Live ACP session id (used at restart to compute the resume id).
    fn session_id(&self) -> Option<String>;
}

/// Pure forwarding impl: every method already exists verbatim on the inherent
/// `impl AcpChannelClient`, so the trait is a thin facade over the same object
/// with zero behaviour change. The blocking `Drop for AcpChannelClient`
/// (swap-out prompt_tx, join worker) satisfies the block-on-Drop/kill-child
/// contract automatically — the concrete type is what's boxed.
impl AgentTransport for AcpChannelClient {
    fn try_recv(&self) -> Option<ReplyEvent> {
        AcpChannelClient::try_recv(self)
    }
    fn is_connected(&self) -> bool {
        AcpChannelClient::is_connected(self)
    }
    fn turn_count(&self) -> usize {
        AcpChannelClient::turn_count(self)
    }
    fn handle(&self) -> TransportHandle {
        AcpChannelClient::handle(self)
    }
    fn session_id(&self) -> Option<String> {
        AcpChannelClient::session_id(self)
    }
}

/// The `Send` (+`Sync`) transport surface the session-server actor stores in
/// its map. Built by [`AcpChannelClient::handle`] by cloning the client's Send
/// sub-fields — it NEVER holds the `!Sync` `reply_rx`, which stays owned by the
/// pump thread. This lets the single-writer actor task drive prompt/cancel/
/// permission/liveness/turn-count reads without ever holding (or dropping) an
/// `AcpChannelClient` (whose `Drop` joins an OS thread).
pub struct TransportHandle {
    /// Outbound prompts (Clone+Send). `send`-equivalent of `AcpChannelClient`.
    pub prompt_tx: std_mpsc::Sender<PromptPayload>,
    /// Outbound native steering payloads.
    steer_tx: std_mpsc::Sender<NativeSteeringCommand>,
    /// Outbound model switches (Clone+Send). `set_model`-equivalent.
    pub set_model_tx: std_mpsc::Sender<String>,
    /// Cancel signal (Clone+Send).
    pub cancel_tx: futures::channel::mpsc::UnboundedSender<()>,
    /// Liveness flag, shared with the worker.
    pub connected: Arc<AtomicBool>,
    /// Completed-turn count, shared with the worker.
    pub turns: Arc<AtomicUsize>,
    /// Current permission policy, shared with the worker.
    pub permission_mode: Arc<AtomicU8>,
    /// Live ACP session id, populated by the worker after `session/new`/`load`.
    pub session_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Whether the adapter advertised native steering.
    pub steering_supported: Arc<AtomicBool>,
    /// The generation this transport was published at (stamped by the actor on
    /// install). Lets liveness/diagnostics correlate a handle with its pump.
    pub generation: u64,
}

impl TransportHandle {
    /// Enqueue a prompt onto the owning pump's client. Mirrors
    /// [`AcpChannelClient::send`]: fails (and marks disconnected) if the worker
    /// channel is closed.
    pub fn send(&self, prompt: &str) -> io::Result<()> {
        self.send_payload(PromptPayload::text(prompt))
    }

    /// Send a prompt carrying image attachments alongside the text.
    pub fn send_payload(&self, payload: PromptPayload) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        let result = if self.supports_steering() {
            self.steer_tx.send(NativeSteeringCommand::Prompt(payload))
        } else {
            self.prompt_tx
                .send(payload)
                .map_err(|error| std_mpsc::SendError(NativeSteeringCommand::Prompt(error.0)))
        };
        result.map_err(|_| {
            self.connected.store(false, Ordering::SeqCst);
            io::Error::new(io::ErrorKind::BrokenPipe, "ACP worker channel closed")
        })
    }

    pub fn supports_steering(&self) -> bool {
        self.steering_supported.load(Ordering::SeqCst)
    }

    pub fn steer_payload(&self, payload: PromptPayload) -> io::Result<()> {
        if !self.supports_steering() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ACP agent does not advertise native steering",
            ));
        }
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        self.steer_tx
            .send(NativeSteeringCommand::Steer(payload))
            .map_err(|_| {
                self.connected.store(false, Ordering::SeqCst);
                io::Error::new(io::ErrorKind::BrokenPipe, "ACP steering channel closed")
            })
    }

    pub fn steer_or_replace_payload(&self, payload: PromptPayload) -> io::Result<()> {
        if self.supports_steering() {
            self.steer_payload(payload)
        } else {
            self.cancel();
            self.send_payload(payload)
        }
    }

    /// Request cancellation of the in-flight turn. Best-effort.
    pub fn cancel(&self) {
        if self.supports_steering() {
            let _ = self.steer_tx.send(NativeSteeringCommand::Cancel);
        } else {
            let _ = self.cancel_tx.unbounded_send(());
        }
    }

    /// Set the live permission policy (read by the worker on gated tool calls).
    pub fn set_permission_mode(&self, mode: PermissionMode) {
        self.permission_mode.store(mode as u8, Ordering::SeqCst);
    }

    /// Switch the session's model. Mirrors [`AcpChannelClient::set_model`]:
    /// enqueues `model_id` for the worker driver to issue as a
    /// `session/set_config_option`. Best-effort.
    pub fn set_model(&self, model_id: &str) {
        let _ = self.set_model_tx.send(model_id.to_string());
    }

    /// Worker liveness flag.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Completed-turn count.
    pub fn turn_count(&self) -> usize {
        self.turns.load(Ordering::SeqCst)
    }

    /// Live ACP session id (populated by the worker after handshake).
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|g| g.clone())
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
        let (dummy_tx, _dummy_rx) = std_mpsc::channel::<PromptPayload>();
        // Note: replaces only `prompt_tx` (still String). `reply_rx` lives
        // on the App side and gets dropped naturally when AcpChannelClient is
        // dropped — no manual swap needed.
        let real_tx = std::mem::replace(&mut self.prompt_tx, dummy_tx);
        drop(real_tx);

        // Native steering has its own blocking std→async bridge; release its
        // sender before joining for the same reason as prompts/model switches.
        let (dummy_steer_tx, _dummy_steer_rx) = std_mpsc::channel::<NativeSteeringCommand>();
        let real_steer_tx = std::mem::replace(&mut self.steer_tx, dummy_steer_tx);
        drop(real_steer_tx);

        // Same for `set_model_tx`: the worker's `set_model` bridge is a
        // `spawn_blocking` thread parked on `set_model_rx.recv()`. `abort()`
        // can't kill an OS thread mid-recv, so that recv only returns once the
        // last `set_model_tx` is dropped. If we joined the worker while still
        // holding it here, dropping the worker's tokio runtime would block
        // forever waiting on that blocking task — a deadlock (the sender only
        // drops after this `Drop` returns, but `Drop` is stuck in `join()`).
        // Release it BEFORE the join, exactly like the prompt sender.
        let (dummy_model_tx, _dummy_model_rx) = std_mpsc::channel::<String>();
        let real_model_tx = std::mem::replace(&mut self.set_model_tx, dummy_model_tx);
        drop(real_model_tx);

        if let Some(handle) = self.worker.take() {
            // Now safe to join: the worker's blocking recvs are both unblocked,
            // bridge_task + set_model_bridge_task exit, async_prompt_rx returns
            // None, the driver loop returns, connect_with returns, and the
            // runtime drops.
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
        let quoted_args: Vec<String> = parts
            .iter()
            .map(|a| shell_words::quote(a).into_owned())
            .collect();
        Some(format!(
            "{} {}",
            shell_words::quote(&resolved),
            quoted_args.join(" ")
        ))
    }
}

#[allow(clippy::too_many_arguments)]
/// Whether a failed `session/prompt` is worth retrying. The Claude Agent
/// SDK surfaces upstream API trouble (Anthropic "overloaded_error" / 529,
/// rate limits / 429, gateway 5xx, transient network drops) as the JSON-RPC
/// error's message/data. We match on those substrings so an overloaded API
/// becomes a brief auto-retry instead of a turn that silently hangs.
/// Deterministic failures (bad request, auth, model refusal) are *not*
/// matched — retrying those just burns time.
fn is_retryable_error(e: &agent_client_protocol::Error) -> bool {
    let mut hay = e.message.to_lowercase();
    if let Some(data) = &e.data {
        hay.push(' ');
        hay.push_str(&data.to_string().to_lowercase());
    }
    const NEEDLES: &[&str] = &[
        "overload",
        "rate limit",
        "rate_limit",
        "too many requests",
        "429",
        "529",
        "503",
        "service unavailable",
        "service_unavailable",
        "temporarily unavailable",
        "timeout",
        "timed out",
        "econnreset",
        "connection reset",
        "connection refused",
        "fetch failed",
    ];
    NEEDLES.iter().any(|n| hay.contains(n))
}

/// Trim an ACP error to a single short clause for the footer Notice — the
/// full message can carry a multi-line provider payload we don't want in a
/// status line.
fn short_err(e: &agent_client_protocol::Error) -> String {
    let msg = e.message.replace('\n', " ");
    let msg = msg.trim();
    if msg.chars().count() > 80 {
        let truncated: String = msg.chars().take(77).collect();
        format!("{truncated}…")
    } else {
        msg.to_string()
    }
}

// builder/render fn — arg count is inherent, splitting would obscure
#[allow(clippy::too_many_arguments)]
fn run_worker(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<PromptPayload>,
    steer_rx: std_mpsc::Receiver<NativeSteeringCommand>,
    set_model_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<ReplyEvent>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
    turns: Arc<AtomicUsize>,
    session_id_slot: Arc<std::sync::Mutex<Option<String>>>,
    resume_session_id: Option<String>,
    permission_mode: Arc<AtomicU8>,
    steering_supported: Arc<AtomicBool>,
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
    cancel_rx: futures::channel::mpsc::UnboundedReceiver<()>,
    frontend: YaldaFrontend,
    provider: AgentProvider,
    resume_only: bool,
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
            steer_rx,
            set_model_rx,
            reply_tx,
            ready_tx,
            connected_for_async,
            turns,
            session_id_slot,
            resume_session_id,
            permission_mode,
            steering_supported,
            wake_tx,
            cancel_rx,
            frontend,
            provider,
            resume_only,
        )
        .await
    });
    if let Err(e) = result {
        // Errors after the readiness handshake bubble out here — log to
        // stderr (yalda is in alt-screen, so this is mostly diagnostic for
        // people running with `2>log`).
        connected.store(false, Ordering::SeqCst);
        eprintln!("[yalda-acp] worker exited with error: {e}");
    }
    // Runtime drops here, killing any straggling tokio tasks (and the child
    // process via Drop on tokio::process::Child).
    connected.store(false, Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
async fn worker_async(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<PromptPayload>,
    steer_rx: std_mpsc::Receiver<NativeSteeringCommand>,
    set_model_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<ReplyEvent>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
    turns: Arc<AtomicUsize>,
    session_id_slot: Arc<std::sync::Mutex<Option<String>>>,
    resume_session_id: Option<String>,
    permission_mode: Arc<AtomicU8>,
    steering_supported: Arc<AtomicBool>,
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
    mut cancel_rx: futures::channel::mpsc::UnboundedReceiver<()>,
    frontend: YaldaFrontend,
    provider: AgentProvider,
    resume_only: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1) Spawn the agent process.
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    // The `cwd` argument has long been forwarded to the agent over the wire
    // as `NewSessionRequest::new(cwd)` below — a project-root hint the agent
    // is supposed to respect. But until this line, the OS-level cwd of the
    // spawned subprocess was whatever yalda's own process cwd happened to
    // be: `tokio::process::Command::new` does not inherit any specific
    // working directory. The two paths could silently diverge, so any agent
    // affordance that reads the OS cwd (Bash `pwd`, a subprocess spawned
    // with a relative path) was resolving against yalda's process cwd,
    // not the per-session cwd. `spec-agent-cwd.md` §3 fixes that.
    cmd.current_dir(&cwd);
    // Scrub the Claude Code nesting-detector env var. When yalda is
    // launched from inside a Claude Code session (very common — yalda
    // users tend to use claude-code as their editor), CLAUDECODE=1 is
    // inherited and the spawned `claude-agent-acp` aborts with "Claude
    // Code cannot be launched inside another Claude Code session." The
    // error message explicitly says: "To bypass this check, unset the
    // CLAUDECODE environment variable." We strip only that — other
    // CLAUDE_CODE_* vars (SESSION_ID, ENTRYPOINT, etc.) are passed through
    // since the agent may key behavior off them.
    cmd.env_remove("CLAUDECODE");
    // Keep Yalda-only credentials out of the adapter and every MCP process it
    // launches. Codex sessions also default to interactive ChatGPT login rather
    // than ambient metered API keys; advanced users can opt back into those.
    let allow_codex_api_key = std::env::var("YALDA_CODEX_ALLOW_API_KEY").as_deref() == Ok("1");
    for key in agent_auth_env_vars_to_remove(provider, allow_codex_api_key) {
        cmd.env_remove(key);
    }
    // Reasoning depth is intentionally NOT configured here. The adapter/SDK
    // already default to adaptive thinking at effort "high", so a raw
    // `MAX_THINKING_TOKENS` budget would only fight the adaptive system. Do
    // not re-introduce it — reasoning depth is not the capability lever.
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Pipe-and-discard the agent's stderr by default. Agents (including
        // claude-agent-acp) may log diagnostics there; keeping it inherit
        // would pollute yalda-gpui's stderr. Set YALDA_ACP_AGENT_STDERR=inherit
        // to surface it for debugging.
        .stderr(
            if std::env::var("YALDA_ACP_AGENT_STDERR").as_deref() == Ok("inherit") {
                std::process::Stdio::inherit()
            } else {
                std::process::Stdio::null()
            },
        )
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = e.kind();
            let _ = ready_tx.send(Err(io::Error::new(
                kind,
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
    let (async_prompt_tx, mut async_prompt_rx) = tokio::sync::mpsc::unbounded_channel::<PromptPayload>();
    let bridge_task = tokio::task::spawn_blocking(move || {
        while let Ok(prompt) = prompt_rx.recv() {
            if async_prompt_tx.send(prompt).is_err() {
                break;
            }
        }
        // Sender side dropped → done. Closing async_prompt_tx (by drop here)
        // signals the driver loop to exit cleanly.
    });

    // Native Codex steering is deliberately out-of-band from ordinary
    // prompts: the prompt driver may be awaiting the active turn response,
    // while steering must reach that live turn immediately. Explicit Stop
    // shares this stream so it cannot overtake earlier steering. A single
    // bridge + async consumer preserves order before the adapter's own FIFO.
    let (async_steer_tx, async_steer_rx) =
        tokio::sync::mpsc::unbounded_channel::<NativeSteeringCommand>();
    let steer_bridge_task = tokio::task::spawn_blocking(move || {
        while let Ok(command) = steer_rx.recv() {
            if async_steer_tx.send(command).is_err() {
                break;
            }
        }
    });

    // Same std→async bridge for out-of-band model switches. A model id pushed
    // via `set_model` reaches the driver loop, which issues a
    // `session/set_config_option`. Kept on its own channel so it never queues
    // behind prompts (a switch applies immediately, mid-turn if needed).
    let (async_set_model_tx, async_set_model_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let set_model_bridge_task = tokio::task::spawn_blocking(move || {
        while let Ok(model_id) = set_model_rx.recv() {
            if async_set_model_tx.send(model_id).is_err() {
                break;
            }
        }
    });

    // 4) Run the ACP client. The closure passed to connect_with stays alive
    //    until we explicitly return — that's our "session lifetime".
    let event_tx_for_handlers = event_tx.clone();
    // Separate clone for the driver loop so it can emit transient Notice
    // events (retry/failed status) alongside the handler's stream events.
    let event_tx_for_driver = event_tx.clone();
    // Clone for the model-switch task so it can emit the refreshed selector.
    let event_tx_for_setmodel = event_tx.clone();
    let event_tx_for_steer = event_tx.clone();
    let connect_result = Client
        .builder()
        .name("yalda")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                acp_debug!("notification: {:?}", notification.update);
                // Forward the variants the agent window knows how to render.
                // Variants still parked (AgentThoughtChunk, AvailableCommands-
                // Update, SessionInfoUpdate, ConfigOptionUpdate) carry explicit
                // drop arms so adding any one of them later is a one-arm change
                // (spec-agent-window.md §31).
                match notification.update {
                    SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(text),
                        ..
                    }) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::Chunk(text.text)));
                    }
                    // User-authored turn echoed on the replay stream. Forward
                    // it so resumed sessions reconstruct the user's own prompts
                    // (Finding 1 / defect B, INV-1, INV-6) — the App dedupes a
                    // live echo of a just-submitted prompt.
                    SessionUpdate::UserMessageChunk(ContentChunk {
                        content: ContentBlock::Text(text),
                        ..
                    }) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::UserMessage(text.text)));
                    }
                    SessionUpdate::ToolCall(tc) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::ToolCallStarted(tc)));
                    }
                    SessionUpdate::ToolCallUpdate(upd) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::ToolCallUpdated(upd)));
                    }
                    SessionUpdate::Plan(plan) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::PlanUpdated(plan)));
                    }
                    SessionUpdate::CurrentModeUpdate(upd) => {
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::ModeChanged(
                                upd.current_mode_id,
                            )));
                    }
                    #[cfg(feature = "unstable_session_usage")]
                    SessionUpdate::UsageUpdate(usage) => {
                        let snap = UsageSnapshot {
                            tokens_used: usage.used,
                            tokens_total: usage.size,
                            // Upstream `Cost` (0.11) carries `amount` + an ISO
                            // `currency` code (it dropped the old `amount_usd`).
                            // `UsageSnapshot.cost_usd` is USD, so only surface
                            // amounts already in USD; other currencies are
                            // dropped rather than mislabeled.
                            cost_usd: usage.cost.as_ref().and_then(|c| {
                                c.currency.eq_ignore_ascii_case("usd").then_some(c.amount)
                            }),
                        };
                        let _ = event_tx_for_handlers
                            .send(WorkerEvent::Reply(ReplyEvent::UsageUpdated(snap)));
                    }
                    // The agent re-advertises its config options (e.g. after a
                    // model or mode change made outside our own request path).
                    // Re-emit the model selector so the switcher label + list
                    // stay live regardless of what triggered the change.
                    SessionUpdate::ConfigOptionUpdate(upd) => {
                        for ev in model_reply_events(&upd.config_options) {
                            let _ = event_tx_for_handlers.send(WorkerEvent::Reply(ev));
                        }
                    }
                    // Parked: explicit no-op arms — promotion is a one-arm
                    // change. AgentMessageChunk's and UserMessageChunk's
                    // non-text content variants (images, etc.) fall through to
                    // the catchall below.
                    SessionUpdate::AgentMessageChunk(_)
                    | SessionUpdate::AgentThoughtChunk(_)
                    | SessionUpdate::AvailableCommandsUpdate(_)
                    | SessionUpdate::SessionInfoUpdate(_) => {}
                    // Future variants added by upstream — drop them rather
                    // than failing to compile, since the enum is
                    // `#[non_exhaustive]`.
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
                    // Mid-turn steering (spec-turn-steering.md): Claude Code's
                    // adapter advertises `_meta.claudeCode.promptQueueing`, meaning
                    // it accepts a `session/prompt` while a turn is in flight and
                    // queues it (processed the instant the current turn finishes).
                    // When set, the driver loop below sends prompts CONCURRENTLY
                    // (it does not wait for the in-flight turn's response before
                    // forwarding the next) so a steer actually reaches the agent
                    // mid-turn instead of after the boundary.
                    let prompt_queueing = init_resp
                        .agent_capabilities
                        .meta
                        .as_ref()
                        .and_then(|m| m.get("claudeCode"))
                        .and_then(|c| c.get("promptQueueing"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let native_steering = supports_native_steering(&init_resp);
                    steering_supported.store(native_steering, Ordering::SeqCst);
                    acp_debug!(
                        "initialize ok; loadSession: {supports_load}, promptQueueing: {prompt_queueing}, nativeSteering: {native_steering}, resume id: {:?}",
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
                    //    yalda-specific verify-after-edit clause stays
                    //    on the front so it's the first thing the model
                    //    sees in the append.
                    // Body of the system-prompt append. The first sentence
                    // is built separately so it can name the active yalda
                    // frontend — everything below is shared.
                    const CLAUDE_CODE_APPEND_BODY: &str = r#"Treat this as an interactive coding session, not a one-shot agent run.

# Tone and style
The user has explicitly asked for this voice; it overrides any earlier tone guidance:

- Be succinct. Summarize what happened — don't narrate every step you took to get there.
- Status updates while you're working should be one short line ("Reading X.", "Running tests.", "Editing Y."). The user wants to know what you're doing in flight, but in headline form.
- Reserve full prose for the moment you actually reach a solid conclusion or finish the task. That's when the user wants the writeup — not before.
- Don't think out loud in the message channel. Internal reasoning, exploration, and intermediate thoughts belong inside tool calls and your own reasoning, not in the user-facing response. The user is looking at yalda's chat, not a transcript of your inner monologue.
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
                    let session_mode = if std::env::var("YALDA_SESSION_MANAGED").as_deref() == Ok("1") {
                        " Session mode: client/server (session survives GUI restarts; managed by yalda-session-server)."
                    } else {
                        " Session mode: direct (GUI owns the agent subprocess)."
                    };
                    let claude_code_append = format!(
                        "You are running inside the yalda editor's Claude Code surface — host: {host}.{session_mode} {body}",
                        host = frontend.host_description(),
                        session_mode = session_mode,
                        body = CLAUDE_CODE_APPEND_BODY,
                    );
                    let agent_meta = || {
                        // `_meta.claudeCode` and `_meta.systemPrompt.append` are
                        // Claude-adapter extensions, not ACP. Codex reads its
                        // durable guidance from AGENTS.md / Codex config, so keep
                        // its session request provider-neutral.
                        if provider == AgentProvider::Codex {
                            return serde_json::Map::new();
                        }
                        let mut m = serde_json::Map::new();
                        m.insert(
                            "systemPrompt".to_string(),
                            serde_json::json!({"append": claude_code_append.as_str()}),
                        );
                        // Pin the SDK's filesystem setting sources so the hosted
                        // agent loads CLAUDE.md + .claude/settings.json (incl. the
                        // `model` pin) exactly like the Claude Code TUI. The
                        // adapter already DEFAULTS to ["user","project","local"]
                        // (acp-agent.js: the hardcoded `settingSources` that
                        // `...userProvidedOptions` only overrides when the client
                        // sends `_meta.claudeCode.options`), but stating it here
                        // makes yalda's intent durable against an adapter default
                        // change. We set ONLY `settingSources` under `options` —
                        // `tools`/`settings` stay unset so they keep the adapter's
                        // own defaults (preset `claude_code` tools, etc.).
                        m.insert(
                            "claudeCode".to_string(),
                            serde_json::json!({
                                "options": {
                                    "settingSources": ["user", "project", "local"]
                                }
                            }),
                        );
                        m
                    };

                    // === Bring up a session: try resume first if we were
                    //     given an id and the agent supports it; otherwise
                    //     fall through to a fresh session/new. We auto-fall
                    //     back on load failure so a stale or GC'd id never
                    //     leaves the user without an attached agent.
                    // The model selector (ModelChanged + ModelsAvailable) is
                    // CAPTURED here, not emitted — on a resume it would land in
                    // the session/load replay burst and be eaten by the server's
                    // replay fence (which discards everything before the
                    // ReplayComplete marker → "models not available yet" on
                    // resumed sessions). We emit it AFTER the marker below so it
                    // is always a live, post-fence event.
                    let mut model_events: Vec<ReplyEvent> = Vec::new();
                    if resume_only && resume_session_id.is_some() && !supports_load {
                        let _ = ready_tx.send(Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "ACP agent does not support loading an existing session",
                        )));
                        return Ok(());
                    }
                    let session_id: SessionId = if let (true, Some(id)) =
                        (supports_load, resume_session_id.as_ref())
                    {
                        let load_req = LoadSessionRequest::new(
                            SessionId::new(id.clone()),
                            cwd.clone(),
                        )
                        .mcp_servers(yalda_mcp_servers())
                        .meta(agent_meta());
                        // A stale / GC'd / otherwise unloadable resume id can make
                        // the agent HANG in `session/load` — it never errors and
                        // never returns, so the existing error-fallback below never
                        // fires and the session is left permanently channel-less
                        // (every prompt queues with no agent to drive it; observed
                        // as a recovered session whose adapter sat in session/load
                        // for 20+ minutes → "no response from claude"). Bound the
                        // load: on TIMEOUT or error, fall back to a fresh
                        // session/new so the session is always drivable. The
                        // transcript is preserved (durable WAL); only the agent's
                        // resumed context is lost — identical to the error path.
                        let load_fut = connection.send_request(load_req).block_task();
                        let load_timeout_secs = if resume_only {
                            INSPECT_SESSION_LOAD_TIMEOUT_SECS
                        } else {
                            SESSION_LOAD_TIMEOUT_SECS
                        };
                        let (loaded, load_failure) = match tokio::time::timeout(
                            std::time::Duration::from_secs(load_timeout_secs),
                            load_fut,
                        )
                        .await
                        {
                            Ok(Ok(resp)) => {
                                acp_debug!("session/load ok: {id}");
                                // A resumed session re-advertises its model
                                // selector in the load response; capture it for
                                // post-fence emission (see the note above).
                                if let Some(opts) = &resp.config_options {
                                    model_events = model_reply_events(opts);
                                }
                                (true, None)
                            }
                            Ok(Err(e)) => {
                                acp_debug!(
                                    "session/load failed ({e}); falling back to session/new"
                                );
                                (false, Some(short_err(&e)))
                            }
                            Err(_elapsed) => {
                                acp_debug!(
                                    "session/load timed out after {load_timeout_secs}s; falling back to session/new"
                                );
                                (
                                    false,
                                    Some(format!(
                                        "timed out after {load_timeout_secs}s"
                                    )),
                                )
                            }
                        };
                        if loaded {
                            SessionId::new(id.clone())
                        } else if resume_only {
                            let detail = load_failure
                                .unwrap_or_else(|| "unknown load failure".to_string());
                            let _ = ready_tx.send(Err(io::Error::other(format!(
                                "ACP session/load failed: {detail}"
                            ))));
                            return Ok(());
                        } else {
                            match connection
                                .send_request(
                                    NewSessionRequest::new(cwd.clone())
                                    .mcp_servers(yalda_mcp_servers())
                                    .meta(agent_meta()),
                                )
                                .block_task()
                                .await
                            {
                                Ok(r) => {
                                    if let Some(opts) = &r.config_options {
                                        model_events = model_reply_events(opts);
                                    }
                                    r.session_id
                                }
                                Err(e2) => {
                                    let _ = ready_tx.send(Err(io::Error::other(format!(
                                        "ACP new session failed (after load fallback): {e2}"
                                    ))));
                                    return Err(e2);
                                }
                            }
                        }
                    } else {
                        match connection
                            .send_request(
                                NewSessionRequest::new(cwd.clone())
                                    .mcp_servers(yalda_mcp_servers())
                                    .meta(agent_meta()),
                            )
                            .block_task()
                            .await
                        {
                            Ok(r) => {
                                if let Some(opts) = &r.config_options {
                                    model_events = model_reply_events(opts);
                                }
                                r.session_id
                            }
                            Err(e) => {
                                let _ = ready_tx.send(Err(io::Error::other(format!(
                                    "ACP new session failed: {e}"
                                ))));
                                return Err(e);
                            }
                        }
                    };
                    // === End-of-replay marker — emitted on EVERY spawn that
                    //     ATTEMPTED a resume, regardless of outcome.
                    //
                    // session/load synthesises the whole prior conversation via
                    // session/update notifications, then returns *after* the
                    // last one — it never fires a session/prompt response. The
                    // marker (Finding 13, INV-4) replaces the old post-load
                    // turn-counter bump: it tells the App to finalize the
                    // replayed prefix exactly once, and never mid-replay. On
                    // the load-ok path the replay burst shares `event_tx` with
                    // this send, so the marker orders strictly after the last
                    // replayed event and strictly before any live one.
                    //
                    // It MUST also fire on the fallback paths (load error /
                    // load timeout / agent without loadSession → session/new):
                    // a consumer that already holds the history — the session-
                    // server's recovered event_log — arms a replay fence and
                    // discards everything before this marker. If the marker
                    // only fired on success, a fallback would leave that fence
                    // up forever and every subsequent live event would be
                    // silently discarded (the resume-hang bug, take two). A
                    // timed-out load's abandoned request may still trickle
                    // late replay notifications in AFTER the marker; those
                    // then record as live events — a known, bounded
                    // duplication hazard, strictly better than the wedge.
                    if resume_session_id.is_some() {
                        let _ = event_tx_for_driver
                            .send(WorkerEvent::Reply(ReplyEvent::ReplayComplete));
                    }
                    // Emit the model selector AFTER the replay marker so it is a
                    // live, post-fence event on both fresh and resumed sessions
                    // (a resume's fence discards everything before the marker).
                    for ev in model_events {
                        let _ = event_tx_for_driver.send(WorkerEvent::Reply(ev));
                    }
                    acp_debug!("session ready: {session_id:?}");
                    if let Ok(mut slot) = session_id_slot.lock() {
                        *slot = Some(session_id.0.to_string());
                    }

                    // Handshake done — App can start sending.
                    let _ = ready_tx.send(Ok(()));

                    // === Model-switch task: independent of the prompt driver ===
                    // Drains `set_model` requests and issues each as a
                    // `session/set_config_option` for the `model` option. Runs
                    // as its own task so a switch applies out-of-band (even
                    // mid-turn) regardless of which prompt-driver variant is
                    // active. The response echoes the refreshed selector, which
                    // we forward as `ModelChanged` + `ModelsAvailable`.
                    let set_model_task = {
                        let connection = connection.clone();
                        let session_id = session_id.clone();
                        let event_tx = event_tx_for_setmodel;
                        let mut rx = async_set_model_rx;
                        tokio::spawn(async move {
                            while let Some(model_id) = rx.recv().await {
                                acp_debug!("set_model → agent: {model_id:?}");
                                let req =
                                    agent_client_protocol::schema::SetSessionConfigOptionRequest::new(
                                        session_id.clone(),
                                        "model",
                                        model_id.clone(),
                                    );
                                match connection.send_request(req).block_task().await {
                                    Ok(resp) => {
                                        for ev in model_reply_events(&resp.config_options) {
                                            let _ = event_tx.send(WorkerEvent::Reply(ev));
                                        }
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(WorkerEvent::Reply(
                                            ReplyEvent::Notice(format!(
                                                "model switch failed: {}",
                                                short_err(&e)
                                            )),
                                        ));
                                    }
                                }
                            }
                        })
                    };

                    // === Native-steering task: independent of prompt driver ===
                    // One ordered consumer means request N+1 is not put on the
                    // wire until N has been accepted by the adapter. The Codex
                    // adapter then serializes these per session and injects each
                    // into the active turn. Explicit `session/cancel` shares
                    // this stream, so it cannot overtake an earlier request.
                    let steer_task = {
                        let connection = connection.clone();
                        let session_id = session_id.clone();
                        let event_tx = event_tx_for_steer;
                        let turns = Arc::clone(&turns);
                        let mut rx = async_steer_rx;
                        tokio::spawn(async move {
                            while let Some(command) = rx.recv().await {
                                match command {
                                    NativeSteeringCommand::Prompt(payload) => {
                                        acp_debug!("ordered prompt → agent: {payload:?}");
                                        let request = connection
                                            .send_request(
                                                agent_client_protocol::schema::PromptRequest::new(
                                                    session_id.clone(),
                                                    payload.content_blocks(),
                                                ),
                                            )
                                            .block_task();
                                        let event_tx = event_tx.clone();
                                        let turns = Arc::clone(&turns);
                                        tokio::spawn(async move {
                                            if let Err(error) = request.await {
                                                let _ = event_tx.send(WorkerEvent::Reply(
                                                    ReplyEvent::Notice(format!(
                                                        "agent error: {}",
                                                        short_err(&error),
                                                    )),
                                                ));
                                            }
                                            let count = turns.fetch_add(1, Ordering::SeqCst) + 1;
                                            if std::env::var("YALDA_EMIT_TURN_ENDED").as_deref()
                                                == Ok("1")
                                            {
                                                let _ = event_tx.send(WorkerEvent::Reply(
                                                    ReplyEvent::TurnEnded { count },
                                                ));
                                            }
                                        });
                                    }
                                    NativeSteeringCommand::Steer(payload) => {
                                        acp_debug!("native steer → agent: {payload:?}");
                                        let req = NativeSteeringRequest::new(
                                            session_id.clone(),
                                            &payload,
                                        );
                                        match connection.send_request(req).block_task().await {
                                            Ok(response) => {
                                                acp_debug!(
                                                    "native steer accepted: {response:?}"
                                                );
                                            }
                                            Err(e) => {
                                                let _ = event_tx.send(WorkerEvent::Reply(
                                                    ReplyEvent::Notice(format!(
                                                        "steering failed: {}",
                                                        short_err(&e)
                                                    )),
                                                ));
                                            }
                                        }
                                    }
                                    NativeSteeringCommand::Cancel => {
                                        acp_debug!("ordered cancel → session/cancel");
                                        let _ = connection.send_notification(
                                            agent_client_protocol::schema::CancelNotification::new(
                                                session_id.clone(),
                                            ),
                                        );
                                    }
                                }
                            }
                        })
                    };

                    // === Driver loop: forward prompts as session/prompt
                    //     requests until the App side closes the channel.  ===
                    use futures::StreamExt as _;
                    // Cap on transient-error retries before giving up on a
                    // turn. Backoff is exponential (0.5s, 1s, 2s, 4s, 8s).
                    const MAX_RETRIES: u32 = 5;

                    // CONCURRENT driver (mid-turn steering, spec-turn-steering.md):
                    // when the agent advertises promptQueueing, send each prompt
                    // the moment it arrives — WITHOUT waiting for the in-flight
                    // turn's response — so a steer reaches the agent mid-turn. The
                    // agent queues it and processes it after the current turn. Each
                    // prompt's response settles independently; `turns` bumps per
                    // settled prompt, preserving the per-turn counter the pump
                    // reads. Used ONLY for capable agents; everything else falls to
                    // the proven sequential loop below, untouched.
                    if prompt_queueing {
                        use futures::stream::FuturesUnordered;
                        let mut inflight = FuturesUnordered::new();
                        let mut intake_open = true;
                        let mut cancel_open = true;
                        // Per-prompt cancel flags. A Stop trips EVERY in-flight
                        // prompt's flag (via these weak handles) AND fires
                        // `session/cancel`, so a prompt that resolves with an
                        // *error* during a cancel is not retried (we must never
                        // resend a cancelled prompt). Per-prompt (not one shared
                        // flag) so a fresh submit can't clear another in-flight
                        // prompt's cancel intent. Weak ⇒ entries self-prune as
                        // their futures complete and drop the owning `Arc`.
                        let mut live_cancels: Vec<std::sync::Weak<AtomicBool>> = Vec::new();
                        loop {
                            tokio::select! {
                                maybe = async_prompt_rx.recv(), if intake_open => {
                                    match maybe {
                                        Some(prompt) => {
                                            let cf = Arc::new(AtomicBool::new(false));
                                            live_cancels.push(Arc::downgrade(&cf));
                                            let connection = connection.clone();
                                            let session_id = session_id.clone();
                                            let event_tx = event_tx_for_driver.clone();
                                            acp_debug!("prompt → agent (queued): {prompt:?}");
                                            // EAGER wire-send, in submit order: send_request
                                            // transmits synchronously here (before the next
                                            // recv), so prompts reach the agent in the order
                                            // submitted regardless of when the response future
                                            // is polled. Only the RESPONSE await is deferred.
                                            let first = agent_client_protocol::schema::PromptRequest::new(
                                                session_id.clone(),
                                                prompt.content_blocks(),
                                            );
                                            let mut resp_fut = connection.send_request(first).block_task();
                                            inflight.push(async move {
                                                let mut attempt: u32 = 0;
                                                loop {
                                                    match resp_fut.await {
                                                        Ok(_) => break,
                                                        Err(e) => {
                                                            // Cancel usually resolves Ok(Cancelled);
                                                            // if it instead races into an error, the
                                                            // per-prompt flag stops a retry/resend.
                                                            if cf.load(Ordering::SeqCst) {
                                                                break;
                                                            }
                                                            if attempt < MAX_RETRIES && is_retryable_error(&e) {
                                                                attempt += 1;
                                                                let backoff_ms = (500u64 << (attempt - 1)).min(8_000);
                                                                let _ = event_tx.send(WorkerEvent::Reply(
                                                                    ReplyEvent::Notice(format!(
                                                                        "API error — retrying {attempt}/{MAX_RETRIES} in {}s ({})",
                                                                        backoff_ms / 1000,
                                                                        short_err(&e),
                                                                    )),
                                                                ));
                                                                tokio::time::sleep(
                                                                    std::time::Duration::from_millis(backoff_ms),
                                                                ).await;
                                                                if cf.load(Ordering::SeqCst) {
                                                                    break; // cancelled during backoff
                                                                }
                                                                let req = agent_client_protocol::schema::PromptRequest::new(
                                                                    session_id.clone(),
                                                                    prompt.content_blocks(),
                                                                );
                                                                resp_fut = connection.send_request(req).block_task();
                                                                continue;
                                                            }
                                                            let _ = event_tx.send(WorkerEvent::Reply(
                                                                ReplyEvent::Notice(format!("agent error: {}", short_err(&e))),
                                                            ));
                                                            break;
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                        None => { intake_open = false; }
                                    }
                                }
                                Some(()) = inflight.next() => {
                                    // One queued prompt settled → advance the turn
                                    // counter (same semantics as the sequential path).
                                    let count = turns.fetch_add(1, Ordering::SeqCst) + 1;
                                    if std::env::var("YALDA_EMIT_TURN_ENDED").as_deref() == Ok("1") {
                                        let _ = event_tx_for_driver
                                            .send(WorkerEvent::Reply(ReplyEvent::TurnEnded { count }));
                                    }
                                }
                                sig = cancel_rx.next(), if cancel_open => {
                                    match sig {
                                        Some(()) => {
                                            // Trip every in-flight prompt's flag (pruning dead
                                            // weaks) so none retries after cancel.
                                            live_cancels.retain(|w| match w.upgrade() {
                                                Some(c) => { c.store(true, Ordering::SeqCst); true }
                                                None => false,
                                            });
                                            acp_debug!("cancel → session/cancel (queued driver)");
                                            let _ = connection.send_notification(
                                                agent_client_protocol::schema::CancelNotification::new(
                                                    session_id.clone(),
                                                ),
                                            );
                                        }
                                        // App dropped the cancel sender (teardown):
                                        // stop selecting on it so we don't spin.
                                        None => { cancel_open = false; }
                                    }
                                }
                            }
                            if !intake_open && inflight.is_empty() {
                                break;
                            }
                        }
                        acp_debug!("queued driver loop exiting");
                        steer_task.abort();
                        set_model_task.abort();
                        return Ok::<_, agent_client_protocol::Error>(());
                    }

                    while let Some(prompt) = async_prompt_rx.recv().await {
                        acp_debug!("prompt → agent: {prompt:?}");
                        // Drop any cancel signal that queued while idle so a
                        // stale Stop click can't abort the turn we're about
                        // to start.
                        while cancel_rx.try_recv().is_ok() {}

                        let mut attempt: u32 = 0;
                        loop {
                            let req = agent_client_protocol::schema::PromptRequest::new(
                                session_id.clone(),
                                prompt.content_blocks(),
                            );
                            // Await the prompt response (turn end), but stay
                            // responsive to a cancel request: on the first
                            // `()` from cancel_rx we fire `session/cancel`,
                            // then keep awaiting — the agent resolves the
                            // turn with StopReason::Cancelled.
                            let resp_fut = connection.send_request(req).block_task();
                            tokio::pin!(resp_fut);
                            let mut cancelled = false;
                            // Once cancel_rx yields (a `()` or channel close)
                            // we disable that select branch so a closed
                            // channel can't spin the loop.
                            let mut cancel_done = false;
                            let outcome = loop {
                                tokio::select! {
                                    r = &mut resp_fut => break r,
                                    sig = cancel_rx.next(), if !cancel_done => {
                                        cancel_done = true;
                                        if sig.is_some() {
                                            cancelled = true;
                                            acp_debug!("cancel → session/cancel");
                                            let _ = connection.send_notification(
                                                agent_client_protocol::schema::CancelNotification::new(
                                                    session_id.clone(),
                                                ),
                                            );
                                        }
                                        // else: App dropped the cancel sender
                                        // (tearing down) — just await resp_fut.
                                    }
                                }
                            };
                            match outcome {
                                Ok(resp) => {
                                    acp_debug!("prompt response: {resp:?}");
                                    break;
                                }
                                Err(e) => {
                                    // A cancel races the agent into an error
                                    // sometimes — treat it as a finished turn,
                                    // not a retryable failure.
                                    if cancelled {
                                        acp_debug!("turn cancelled: {e}");
                                        break;
                                    }
                                    if attempt < MAX_RETRIES && is_retryable_error(&e) {
                                        attempt += 1;
                                        let backoff_ms =
                                            (500u64 << (attempt - 1)).min(8_000);
                                        eprintln!(
                                            "[yalda-acp] prompt failed (retry {attempt}/{MAX_RETRIES} in {backoff_ms}ms): {e}"
                                        );
                                        let _ = event_tx_for_driver.send(WorkerEvent::Reply(
                                            ReplyEvent::Notice(format!(
                                                "API error — retrying {attempt}/{MAX_RETRIES} in {}s ({})",
                                                backoff_ms / 1000,
                                                short_err(&e),
                                            )),
                                        ));
                                        tokio::time::sleep(
                                            std::time::Duration::from_millis(backoff_ms),
                                        )
                                        .await;
                                        continue; // resend the same prompt
                                    }
                                    eprintln!("[yalda-acp] prompt failed: {e}");
                                    let _ = event_tx_for_driver.send(WorkerEvent::Reply(
                                        ReplyEvent::Notice(format!(
                                            "agent error: {}",
                                            short_err(&e),
                                        )),
                                    ));
                                    break;
                                }
                            }
                        }
                        // Bump the turn counter once the turn settles
                        // (success, cancel, or exhausted retries) — the user
                        // can send again either way.
                        let count = turns.fetch_add(1, Ordering::SeqCst) + 1;
                        // 8b additive (ADR-0006): emit the authoritative turn
                        // boundary HERE, where the worker stands on the resolved
                        // `session/prompt`. Gated off by default so the durable
                        // event stream (and the in-flight reconnect/replay work)
                        // is unperturbed; set `YALDA_EMIT_TURN_ENDED=1` to gather
                        // agreement data before the inference is deleted.
                        if std::env::var("YALDA_EMIT_TURN_ENDED").as_deref() == Ok("1") {
                            let _ = event_tx_for_driver
                                .send(WorkerEvent::Reply(ReplyEvent::TurnEnded { count }));
                        }
                    }
                    acp_debug!("driver loop exiting");
                    steer_task.abort();
                    set_model_task.abort();
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
    steer_bridge_task.abort();
    set_model_bridge_task.abort();
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

    /// Codex advertises native steering in the initialize response's root
    /// `_meta`, not under `agentCapabilities`. Negative control: read the
    /// latter (or require any value other than boolean true) and this fails.
    #[test]
    fn initialize_root_meta_enables_native_steering() {
        let capable: InitializeResponse = serde_json::from_value(serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {},
            "_meta": { "steering": { "supported": true } }
        }))
        .expect("deserialize capable initialize response");
        assert!(supports_native_steering(&capable));

        let incapable: InitializeResponse = serde_json::from_value(serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "_meta": { "steering": { "supported": true } }
            }
        }))
        .expect("deserialize incapable initialize response");
        assert!(!supports_native_steering(&incapable));
    }

    /// Pin the installed Codex adapter extension's wire method and camelCase
    /// payload, including images. This guards against silently sending a valid
    /// JSON-RPC request with the wrong extension name or parameter spelling.
    #[test]
    fn native_steering_request_matches_codex_extension_wire_shape() {
        use agent_client_protocol::JsonRpcMessage;

        let request = NativeSteeringRequest::new(
            SessionId::new("codex-session"),
            &PromptPayload {
                text: "second question".into(),
                images: vec![ImageAttachment {
                    data: "AAAA".into(),
                    mime_type: "image/png".into(),
                }],
            },
        );
        assert_eq!(request.method(), "_session/steering");
        let params = serde_json::to_value(request).expect("serialize native steering request");
        assert_eq!(params["sessionId"], "codex-session");
        assert_eq!(params["prompt"][0]["text"], "second question");
        assert_eq!(params["prompt"][1]["data"], "AAAA");
        assert_eq!(params["prompt"][1]["mimeType"], "image/png");
    }

    /// Every spawned agent session must carry exactly one `yalda` stdio MCP
    /// server, so an agent running inside Yalda can recursively control it.
    /// Negative control: return `Vec::new()` from `yalda_mcp_servers` (or drop
    /// the `McpServer::Stdio(...)` element) and this fails — the count / name
    /// assertions no longer hold.
    #[test]
    fn yalda_mcp_servers_yields_one_named_stdio_server() {
        let servers = yalda_mcp_servers();
        assert_eq!(servers.len(), 1, "exactly one injected MCP server");
        match &servers[0] {
            McpServer::Stdio(s) => {
                assert_eq!(s.name, "yalda", "server name must be yalda");
                assert!(
                    s.command.as_os_str().to_string_lossy().contains("yalda-mcp"),
                    "command should point at the yalda-mcp binary, got {:?}",
                    s.command
                );
            }
            other => panic!("expected a stdio MCP server, got {:?}", other),
        }
    }

    /// The MCP server reaches the wire: a `NewSessionRequest` built the way the
    /// spawn path builds it serializes to JSON that carries the `yalda` MCP
    /// server under `mcpServers`. Negative control: drop
    /// `.mcp_servers(yalda_mcp_servers())` at the construction sites (or make
    /// the helper return empty) and `mcpServers` is `[]`, failing the assert.
    #[test]
    fn new_session_request_serializes_yalda_mcp_server() {
        let req = NewSessionRequest::new(std::path::PathBuf::from("/tmp/x"))
            .mcp_servers(yalda_mcp_servers());
        let v = serde_json::to_value(&req).expect("serialize NewSessionRequest");
        let servers = v["mcpServers"].as_array().expect("mcpServers array");
        assert_eq!(servers.len(), 1, "one MCP server on the wire: {v}");
        assert_eq!(servers[0]["name"], "yalda");
        assert!(
            servers[0]["command"]
                .as_str()
                .unwrap_or("")
                .contains("yalda-mcp"),
            "stdio command should be yalda-mcp: {}",
            servers[0]
        );
    }

    /// A prompt carrying image attachments is turned into a mixed content-block
    /// vector: the text block first, then one `ContentBlock::Image` per
    /// attachment carrying the base64 data + mime type the agent reads. This is
    /// the exact payload `session/prompt` sends. Negative control: drop the
    /// `for img in …` push in `content_blocks` and the image assertions fail
    /// (only the text block survives).
    #[test]
    fn prompt_payload_builds_text_then_image_blocks() {
        let payload = PromptPayload {
            text: "look at this".into(),
            images: vec![
                ImageAttachment {
                    data: "AAAA".into(),
                    mime_type: "image/png".into(),
                },
                ImageAttachment {
                    data: "BBBB".into(),
                    mime_type: "image/jpeg".into(),
                },
            ],
        };
        let blocks = payload.content_blocks();
        assert_eq!(blocks.len(), 3, "text + 2 images");
        match &blocks[0] {
            ContentBlock::Text(t) => assert_eq!(t.text, "look at this"),
            other => panic!("expected text block first, got {other:?}"),
        }
        match &blocks[1] {
            ContentBlock::Image(img) => {
                assert_eq!(img.data, "AAAA");
                assert_eq!(img.mime_type, "image/png");
            }
            other => panic!("expected image block, got {other:?}"),
        }
        match &blocks[2] {
            ContentBlock::Image(img) => {
                assert_eq!(img.data, "BBBB");
                assert_eq!(img.mime_type, "image/jpeg");
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    /// An image-only prompt (no typed text) still yields exactly the image
    /// block(s) — no stray empty text block padding the request.
    #[test]
    fn prompt_payload_image_only_omits_empty_text_block() {
        let payload = PromptPayload {
            text: String::new(),
            images: vec![ImageAttachment {
                data: "ZZ".into(),
                mime_type: "image/png".into(),
            }],
        };
        let blocks = payload.content_blocks();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Image(_)));
    }

    /// A fully empty payload still produces the single empty text block ACP
    /// requires (at least one block per `session/prompt`).
    #[test]
    fn prompt_payload_empty_yields_one_text_block() {
        let blocks = PromptPayload::default().content_blocks();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text(_)));
    }

    /// The model `Select` (id `"model"`, category `Model`) is parsed into
    /// `(current, [ModelOption])` preserving advertised order + labels; a
    /// non-model option alongside it is ignored. Mirrors the real
    /// `claude-agent-acp` `session/new` payload observed in the wild.
    #[test]
    fn model_state_parses_select_current_and_options() {
        use agent_client_protocol::schema::{
            SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        };
        // A non-model select that must be skipped.
        let mode_opt = SessionConfigOption::select(
            "mode",
            "Mode",
            "default",
            vec![SessionConfigSelectOption::new("default", "Default")],
        );
        let mut model_opt = SessionConfigOption::select(
            "model",
            "Model",
            "sonnet",
            vec![
                SessionConfigSelectOption::new("default", "Default (recommended)"),
                SessionConfigSelectOption::new("claude-fable-5[1m]", "Fable"),
                SessionConfigSelectOption::new("sonnet", "Sonnet"),
            ],
        );
        model_opt.category = Some(SessionConfigOptionCategory::Model);

        let (current, options) =
            model_state_from_config_options(&[mode_opt, model_opt]).expect("model selector parsed");
        assert_eq!(current, "sonnet");
        assert_eq!(
            options,
            vec![
                ModelOption { id: "default".into(), label: "Default (recommended)".into() },
                ModelOption { id: "claude-fable-5[1m]".into(), label: "Fable".into() },
                ModelOption { id: "sonnet".into(), label: "Sonnet".into() },
            ]
        );

        // model_reply_events emits BOTH a ModelChanged(current) and a
        // ModelsAvailable{current, options} so the status strip + switcher stay
        // in sync from one config payload.
        let mode_only = SessionConfigOption::select(
            "mode",
            "Mode",
            "default",
            vec![SessionConfigSelectOption::new("default", "Default")],
        );
        assert!(
            model_reply_events(&[mode_only]).is_empty(),
            "no model selector ⇒ no model events"
        );
    }

    /// The orphan reaper only targets adapters whose parent is PID 1 (the spawner
    /// died) AND whose command matches an adapter needle — so it can never kill a
    /// live session's adapter (ppid != 1) or an unrelated process.
    #[test]
    fn orphaned_adapter_pids_targets_only_reparented_adapters() {
        let ps = "\
  4321     1 node /opt/homebrew/bin/claude-agent-acp --stdio
  4400  4321 node (child of a live adapter, not an orphan)
  5555     1 node /usr/local/bin/claude-code-acp
  6000     1 /Applications/Foo.app/Contents/MacOS/Foo
  7000  2999 node /opt/homebrew/bin/claude-agent-acp --stdio
     1     0 /sbin/launchd";
        let pids = orphaned_adapter_pids(ps, ADAPTER_PROCESS_NEEDLES);
        // 4321 (agent-acp, ppid 1) and 5555 (code-acp, ppid 1) only.
        assert_eq!(pids, vec![4321, 5555]);
        // 7000 is an adapter but owned (ppid 2999) → never killed.
        assert!(!pids.contains(&7000), "a live (owned) adapter must be spared");
        // 6000 is orphaned but not an adapter → never killed.
        assert!(!pids.contains(&6000), "a non-adapter orphan must be spared");
    }

    #[test]
    fn anthropic_key_is_never_forwarded_to_agent_or_mcp_processes() {
        assert_eq!(
            agent_auth_env_vars_to_remove(AgentProvider::Claude, false),
            vec!["ANTHROPIC_API_KEY"]
        );
        assert!(
            agent_auth_env_vars_to_remove(AgentProvider::Codex, true)
                .contains(&"ANTHROPIC_API_KEY"),
            "the private Anthropic autonaming key must be scrubbed for every provider"
        );
        assert_eq!(
            agent_auth_env_vars_to_remove(AgentProvider::Codex, false),
            vec![
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "CODEX_API_KEY",
                "DEFAULT_AUTH_REQUEST"
            ]
        );
    }

    /// The escalation contract: explicit Yolo DOES allow shell execution.
    /// The default is now Yolo (config-overridable), so this also pins the
    /// no-config default behaviour: gated tools auto-approve out of the box.
    #[test]
    fn explicit_yolo_allows_execute() {
        assert!(allow_tool_kind(PermissionMode::Yolo, ToolKind::Execute));
    }

    /// The current default is Yolo (auto-approve), config-overridable. The
    /// 0600 owner-only socket — not the permission mode — is what gates who can
    /// drive the agent; the safe modes stay available for users who want them.
    #[test]
    fn default_permission_mode_is_yolo() {
        assert_eq!(DEFAULT_PERMISSION_MODE, PermissionMode::Yolo);
        assert!(allow_tool_kind(DEFAULT_PERMISSION_MODE, ToolKind::Execute));
        assert!(allow_tool_kind(DEFAULT_PERMISSION_MODE, ToolKind::Delete));
    }

    /// The safe modes remain available and never auto-approve dangerous tools,
    /// so a user who opts back into them gets the original protective behaviour.
    #[test]
    fn safe_modes_still_decline_dangerous_tools() {
        for mode in [PermissionMode::ReadOnly, PermissionMode::AskEachTime] {
            assert!(!allow_tool_kind(mode, ToolKind::Execute));
            assert!(!allow_tool_kind(mode, ToolKind::Delete));
        }
        // AutoEdit allows edits but still declines shell/delete.
        assert!(!allow_tool_kind(
            PermissionMode::AutoEdit,
            ToolKind::Execute
        ));
        assert!(!allow_tool_kind(PermissionMode::AutoEdit, ToolKind::Delete));
    }

    /// `parse` round-trips every `short_label()` value and rejects garbage, so
    /// the config knob and the chrome label can never drift apart silently.
    #[test]
    fn parse_round_trips_short_labels_and_rejects_garbage() {
        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::AutoEdit,
            PermissionMode::AskEachTime,
            PermissionMode::Yolo,
        ] {
            assert_eq!(
                PermissionMode::parse(mode.short_label()),
                Some(mode),
                "short_label {:?} must parse back to itself",
                mode.short_label()
            );
        }
        assert_eq!(PermissionMode::parse("nonsense"), None);
        assert_eq!(PermissionMode::parse(""), None);
    }

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
        # Replay the user's own turn (as a real agent does on session/load),
        # then stream two agent chunks, then return.
        emit({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "user_message_chunk",
                                    "content": {"type": "text", "text": "remembered prompt"}}}})
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

    /// A steering-capable ACP peer that keeps the initial prompt open, records
    /// extension/cancel traffic, and acknowledges each steering request. The
    /// log lets the test assert the production worker's actual wire order.
    fn write_steering_agent_script(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake_steering_agent.py");
        let script = r#"#!/usr/bin/env python3
import sys, json

log_path = sys.argv[1]
pending_prompt = None

def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def record(value):
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(value + "\n")
        log.flush()

while True:
    line = sys.stdin.readline()
    if not line:
        break
    try:
        msg = json.loads(line)
    except Exception:
        continue
    method = msg.get("method", "")
    msg_id = msg.get("id")
    if method == "initialize":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"protocolVersion": 1, "agentCapabilities": {},
                         "_meta": {"steering": {"supported": True}}}})
    elif method == "session/new":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"sessionId": "steer-sess-1"}})
    elif method == "session/prompt":
        pending_prompt = msg_id
        record("prompt")
    elif method == "_session/steering":
        blocks = msg.get("params", {}).get("prompt", [])
        text = next((b.get("text", "") for b in blocks if b.get("type") == "text"), "")
        record("steer:" + text)
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"outcome": "injected"}})
    elif method == "session/cancel":
        record("cancel")
        if pending_prompt is not None:
            emit({"jsonrpc": "2.0", "id": pending_prompt,
                  "result": {"stopReason": "cancelled"}})
            pending_prompt = None
"#;
        let mut file = std::fs::File::create(&path).expect("create steering script");
        file.write_all(script.as_bytes())
            .expect("write steering script");
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        path
    }

    /// Production-worker regression for bug-0036's recurrence. This crosses
    /// the real subprocess/JSON-RPC boundary and proves Stop cannot overtake
    /// questions already accepted by the GUI-side ordered control stream.
    #[test]
    fn production_worker_sends_native_steers_before_later_stop() {
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
        {
            eprintln!("python3 not available — skipping native steering wire-order test");
            return;
        }

        let temp = tempfile::tempdir().expect("tmpdir");
        let script = write_steering_agent_script(temp.path());
        let log = temp.path().join("wire-order.log");
        let command = format!("{} {}", script.display(), log.display());
        let mut client = AcpChannelClient::spawn(&command, Some(temp.path().to_path_buf()))
            .expect("spawn steering ACP agent");
        assert!(client.supports_steering());

        client.send("initial work").expect("send initial prompt");
        client
            .steer_or_replace_payload(PromptPayload::text("first question"))
            .expect("send first steer");
        client
            .steer_or_replace_payload(PromptPayload::text("second question"))
            .expect("send second steer");
        client.cancel();

        let expected = [
            "prompt",
            "steer:first question",
            "steer:second question",
            "cancel",
        ];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let observed = loop {
            let observed = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if observed.len() >= expected.len() || std::time::Instant::now() >= deadline {
                break observed;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(observed, expected, "wire controls must preserve user order");
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

        let mut client =
            AcpChannelClient::spawn(script.to_str().unwrap(), Some(tmp.path().to_path_buf()))
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

    /// INV-1 / INV-6 (Finding 1, defect B): a `UserMessageChunk` replayed on
    /// the stream (as a real agent emits on session/load) must survive the
    /// channel boundary as `ReplyEvent::UserMessage` *and* reconstruct a
    /// `TurnId::User` frozen line in the editor. Before the fix the channel
    /// dropped `UserMessageChunk` into a no-op arm, so resumed user turns
    /// vanished permanently. This walks the whole replay→freeze path inside
    /// the lib crate (the `apply_reply_events` glue itself lives in the bin).
    #[test]
    fn replayed_user_message_freezes_user_turn() {
        use crate::editor::Editor;

        // Mirror of the bin's `TurnId` — `freeze_as_user_turn` is generic
        // over the tag, so the editor side tags lines with this.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        enum TurnId {
            User(usize),
        }

        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            eprintln!("python3 not available — skipping ACP user-message replay test");
            return;
        }

        let tmp = tempfile::tempdir().expect("tmpdir");
        let script = write_fake_agent_script(tmp.path());

        let mut client =
            AcpChannelClient::spawn(script.to_str().unwrap(), Some(tmp.path().to_path_buf()))
                .expect("spawn ACP agent");
        assert!(client.is_connected());

        client.send("hi there").expect("send prompt");

        // Drain events until the replayed user turn surfaces. The fake agent
        // emits a `user_message_chunk` ahead of its agent chunks.
        let mut user_text: Option<String> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match client.try_recv() {
                Some(ReplyEvent::UserMessage(text)) => {
                    user_text = Some(text);
                    break;
                }
                Some(_) => {}
                None => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        let user_text = user_text.expect(
            "expected ReplyEvent::UserMessage from a replayed user_message_chunk \
             (channel must not drop the user role) — INV-1/INV-6",
        );
        assert_eq!(user_text, "remembered prompt");

        // The replay consumer freezes it as a `TurnId::User` turn. Assert the
        // line lands frozen and carries the User tag, proving the role
        // reconstructs from the replay stream alone.
        let mut editor = Editor::new(String::new(), std::path::PathBuf::from("test.md"));
        editor.freeze_as_user_turn(&user_text, TurnId::User(1));
        assert_eq!(editor.document().full_text(), "remembered prompt\n");
        assert!(
            editor.is_frozen_line(0),
            "replayed user turn line should be frozen"
        );
        let anchor = editor.anchor_for_line(0);
        assert_eq!(
            editor.metadata::<TurnId>().get(anchor),
            Some(&TurnId::User(1)),
            "replayed user turn must be tagged TurnId::User"
        );
    }

    #[test]
    fn spawn_fails_with_missing_binary() {
        let err = match AcpChannelClient::spawn("/no/such/binary/that/exists-please", None) {
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

    // === Findings 3 & 13: replay turn attribution + finalize gate ===
    //
    // These exercise the `ReplayTurns` state machine the bin's
    // `apply_reply_events` delegates to. The driver below mirrors that loop
    // exactly (resolve `current_turn()` per event; `Chunk` → Llm(k);
    // `UserMessage` → advance boundary, User(k); `ReplayComplete` →
    // finish_replay + finalize) so the assertions pin the bin's behavior
    // without the bin's GPUI test harness.

    /// What a frozen line ends up tagged with — a lib-side mirror of the
    /// bin's `TurnId` (which lives in main.rs).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Tag {
        User(usize),
        Llm(usize),
    }

    /// One replayed event, abstracted to just what drives attribution.
    enum Ev {
        User,
        Chunk,
        ReplayComplete,
    }

    /// Replica of the bin's `apply_reply_events` attribution loop. Returns the
    /// per-line `Tag` sequence and the number of finalize signals raised.
    fn drive_replay(start_last_seen: usize, events: &[Ev]) -> (Vec<Tag>, usize) {
        let mut rt = ReplayTurns::new(start_last_seen);
        let mut tags = Vec::new();
        let mut finalizes = 0;
        for ev in events {
            // current_turn() is re-read per event, exactly as the bin does,
            // so a mid-stream UserMessage boundary shifts the chunks after it.
            match ev {
                Ev::Chunk => tags.push(Tag::Llm(rt.current_turn())),
                Ev::User => tags.push(Tag::User(rt.advance_user_boundary())),
                Ev::ReplayComplete => {
                    rt.finish_replay();
                    finalizes += 1;
                }
            }
        }
        (tags, finalizes)
    }

    /// INV-3 (Finding 3): replaying a 2-user/2-agent `session/load` must tag
    /// turns `User(1),Llm(1),User(2),Llm(2)` — NOT collapse the whole history
    /// onto `User(1)/Llm(1)`. Before the fix, attribution read a single
    /// constant `last_seen_turns + 1` for the whole replay batch, so every
    /// line was turn 1.
    #[test]
    fn replay_attribution_advances_per_user_boundary() {
        let (tags, finalizes) = drive_replay(
            0,
            &[
                Ev::User, // first exchange opens turn 1
                Ev::Chunk,
                Ev::User, // second exchange opens turn 2
                Ev::Chunk,
                Ev::ReplayComplete, // end-of-replay marker
            ],
        );
        assert_eq!(
            tags,
            vec![Tag::User(1), Tag::Llm(1), Tag::User(2), Tag::Llm(2),],
            "replayed turns must count up per user boundary, not collapse to all-1s (INV-3)"
        );
        // Sanity: the regression we're guarding against is all-1s.
        assert_ne!(
            tags,
            vec![Tag::User(1), Tag::Llm(1), Tag::User(1), Tag::Llm(1)],
            "regression: whole replay collapsed onto turn 1"
        );
        assert_eq!(finalizes, 1, "replay finalizes exactly once");
    }

    /// INV-4 (Finding 13): replaying many chunks (>64, the bin's per-tick
    /// drain budget) across bursts must finalize EXACTLY once — after the
    /// last replayed chunk, never mid-replay on a transiently-empty queue.
    /// The explicit `ReplayComplete` marker (sent strictly after the last
    /// chunk) is the gate; an empty queue between bursts no longer infers
    /// turn-end. Multiple agent chunks under one user boundary all stay on
    /// the same `Llm(k)`.
    #[test]
    fn replay_finalizes_exactly_once_after_last_chunk() {
        let mut events = Vec::new();
        events.push(Ev::User);
        for _ in 0..200 {
            events.push(Ev::Chunk);
        }
        events.push(Ev::ReplayComplete);

        let (tags, finalizes) = drive_replay(0, &events);

        assert_eq!(
            finalizes, 1,
            "finalize must run exactly once, gated on ReplayComplete (INV-4)"
        );
        // First line is the user boundary; the rest are agent chunks, all on
        // turn 1 (one user boundary → one Llm turn). No premature boundary
        // bump from a transient empty queue.
        assert_eq!(tags.first(), Some(&Tag::User(1)));
        assert_eq!(tags.len(), 201, "200 chunks + 1 user line");
        assert!(
            tags[1..].iter().all(|t| *t == Tag::Llm(1)),
            "all replayed agent chunks under one user boundary share Llm(1)"
        );
    }

    /// `finish_replay` folds the replay cursor into the live counter so the
    /// NEXT live turn resumes from the right `k` (no off-by-one after a
    /// resumed multi-turn session).
    #[test]
    fn finish_replay_resumes_live_counter() {
        let mut rt = ReplayTurns::new(0);
        rt.advance_user_boundary(); // replay turn 1
        rt.advance_user_boundary(); // replay turn 2
        assert_eq!(rt.current_turn(), 2);
        rt.finish_replay();
        assert_eq!(rt.replay_turn, 0, "replay cursor cleared");
        assert_eq!(
            rt.last_seen, 2,
            "live counter caught up to last replayed turn"
        );
        // The next live turn in flight is turn 3.
        assert_eq!(rt.current_turn(), 3);
    }

    /// `current_turn()` is the single source of `k` shared by live and
    /// replay (INV-3): outside replay it is `last_seen + 1`; inside replay it
    /// is the boundary-advanced cursor.
    #[test]
    fn current_turn_single_source() {
        let mut rt = ReplayTurns::new(3);
        assert_eq!(rt.current_turn(), 4, "live: one past last settled turn");
        rt.advance_user_boundary();
        assert_eq!(rt.current_turn(), 4, "replay seeds from the live counter");
    }
}
