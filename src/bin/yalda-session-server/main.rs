//! `yalda-session-server` — thin daemon that owns ACP agent subprocesses.
//!
//! The GUI (`yalda-gpui`) connects over a Unix domain socket and
//! creates/attaches/prompts sessions. When the GUI is rebuilt and
//! relaunched, it reconnects to the same running server — agent sessions
//! survive the transition.
//!
//! Run:
//!     cargo run --bin yalda-session-server
//!
//! The GUI auto-launches this binary if not already running.

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};

use yalda::acp_channel::{
    AgentProvider, AgentSpawner, AgentTransport, ImageAttachment, PermissionMode, PromptPayload,
    RealAgentSpawner, TransportHandle, YaldaFrontend, configured_agent_command,
};
use yalda::session_proto::*;

mod bridge;
mod launchd;

// ── Forwarder progress + published log snapshot (Bug 1) ─────────────
//
// The forwarder runs as a detached task and tails `event_log` on each wake.
// Two pieces of plumbing make it log_base-aware (so a Stage-B trim can't corrupt
// the LIVE stream) and let the trim site treat the owner / live forwarders as a
// hard compaction ceiling (spec §6 "never compact past owner.acked_seq"):
//
// * `LogSnapshot` is what rides the `log_tx` watch instead of a bare
//   `Arc<Vec<Notification>>`. It carries the whole `EventLog` (a cheap clone —
//   an `Arc` pointer + a `u64` base) plus the session's current `generation`, so
//   the forwarder can translate its LOGICAL `sent_seq` into a `Vec` offset
//   against the CURRENT `log_base` on every wake (Bug 1a) via
//   `EventLog::resolve_sent` — the same translation the attach-time resolver
//   uses, never a duplicated-and-drifting copy.
//
// * `ForwarderProgress` is a shared handle holding a forwarder's last forwarded
//   `sent_seq` plus an `evicted` kill flag. One clone lives in the forwarder
//   task; one clone is registered on `ManagedSession::forwarders`. The actor
//   reads the MINIMUM `sent_seq` over all still-live forwarders (a dead forwarder
//   drops its `Arc`, so `Arc::strong_count == 1` prunes it) to compute the trim
//   floor — so the trim never drops below the slowest live forwarder, the OWNER
//   (which is always a live forwarder) included. That is the spec §6 owner
//   hard-ceiling in its shippable minimal form (Bug 1b).
//
//   HIGH-WATER DISCONNECT (spec §6, MAJOR): the owner hard-ceiling means a
//   slow/paused forwarder pins `min(sent_seq)` and the trim can't fire, so the
//   in-memory `Vec` grows. When the backlog (`tip_seq - floor`) crosses
//   `event_log_high_water()`, the actor sets `evicted` on the SLOWEST forwarder
//   (the one holding the floor) and prunes it from `forwarders`, dropping it
//   from the `min` so the trim resumes — bounding growth. The forwarder task
//   observes `evicted` on its next wake (every `push_event` publishes a snapshot
//   that wakes it) and returns, closing its write half → the client sees EOF and
//   does a clean from-base reconnect (NOT a silent gap). The owner is NOT exempt:
//   a wedged owner that crosses the mark is cleanly bounced and reclaims its
//   lease deterministically via same-`client_id` reclaim on reconnect (phase 4).
#[derive(Clone)]
struct LogSnapshot {
    log: yalda::event_log::EventLog,
    /// The session's `channel_generation` at publish time. A live forwarder
    /// shares this epoch (a generation bump forces a fresh attach), so it is the
    /// `current_gen` passed to `resolve_sent`.
    generation: u64,
}

/// A live forwarder's shared state, held by both the forwarder task and the
/// actor. `sent_seq` is the last forwarded logical seq (the actor reads the
/// `min` over live handles for the trim floor); `evicted` is the high-water
/// kill flag (the actor sets it to force-disconnect the slowest forwarder when
/// the backlog crosses the high-water bound, spec §6 — the forwarder observes it
/// on its next wake and returns).
struct ForwarderHandle {
    sent_seq: std::sync::atomic::AtomicU64,
    /// Set by the actor's high-water disconnect (spec §6). When `true`, the
    /// forwarder task exits at its next wake, closing its write half so the
    /// client gets a clean EOF + from-base reconnect (NOT a silent gap).
    ///
    /// **This flag kills the whole CONNECTION, not one session.** The write half
    /// is per-connection (`stream.into_split()`), shared by every session
    /// forwarder on it — shutting it down disconnects all of them. Only a real
    /// high-water wedge may set this (bug-0028).
    evicted: std::sync::atomic::AtomicBool,
    /// Set when ONE session's forwarder should stop while the connection stays
    /// up — today, archiving that session (bug-0028). The forwarder exits at its
    /// next wake without touching the shared write half, so the client's other
    /// sessions keep streaming and nothing reconnects.
    released: std::sync::atomic::AtomicBool,
}

impl ForwarderHandle {
    fn new(initial_sent_seq: u64) -> Self {
        Self {
            sent_seq: std::sync::atomic::AtomicU64::new(initial_sent_seq),
            evicted: std::sync::atomic::AtomicBool::new(false),
            released: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// How a forwarder task must stop when the actor has flagged it. Split out as a
/// pure mapping so "archive must not kill the connection" is guardable without
/// a live socket (bug-0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwarderStop {
    /// Exit AND shut the shared connection write half (client sees EOF and
    /// reconnects from base). High-water wedge only.
    ShutdownConnection,
    /// Exit this session's forwarder only; leave the connection alone.
    ThisSessionOnly,
}

/// The flag → stop-action mapping. `released` is checked first: it is the
/// narrower, non-destructive action, so a handle carrying both must not escalate
/// to a connection teardown.
fn forwarder_stop_action(progress: &ForwarderHandle) -> Option<ForwarderStop> {
    use std::sync::atomic::Ordering;
    if progress.released.load(Ordering::Acquire) {
        return Some(ForwarderStop::ThisSessionOnly);
    }
    if progress.evicted.load(Ordering::Acquire) {
        return Some(ForwarderStop::ShutdownConnection);
    }
    None
}

/// A shared [`ForwarderHandle`] (the actor's clone + the forwarder task's clone).
type ForwarderProgress = Arc<ForwarderHandle>;

// ── Actor command inlet ────────────────────────────────────────────
//
// All session-state mutation flows through this single inlet, drained by the
// single-writer `run_manager` actor task that OWNS the HashMap (no Mutex).
// `sid` = ServerSessionId. Oneshot replies are used where the caller needs a
// consistent read/ack; pump-sourced commands carry no reply.
//
// `generation` on the pump-sourced commands (Record/TurnCount/AgentDisconnected)
// is the fence (Blocker B): the actor ignores any whose generation !=
// session.channel_generation.
// wire/event enum — boxing the large variant would ripple through serialization + every match site
#[allow(clippy::large_enum_variant)]
enum Command {
    // ── External (connection-handler sourced; each carries a oneshot) ──
    Create {
        cwd: PathBuf,
        label: String,
        provider: AgentProvider,
        resume_session_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<SessionInfo>,
    },
    Attach {
        sid: ServerSessionId,
        /// Optional reconnect cursor `(generation, index)`. Resolved by
        /// `do_attach` against the session's `channel_generation` +
        /// `event_log.len()` into the forwarder's initial `sent` value (the
        /// `u64` in the reply): the tail starts there. `None` / stale /
        /// out-of-range ⇒ `0` ⇒ full replay (unchanged behavior).
        cursor: Option<(u64, u64)>,
        // On success: (log watch, initial forwarder cursor, forwarder progress
        // handle). The forwarder cursor is a LOGICAL `sent_seq` (Bug 1a), NOT a
        // `Vec` index, so a later trim can't re-alias it; the progress handle is
        // the shared `AtomicU64` the actor reads for the trim floor (Bug 1b).
        // type alias would hurt readability here more than help
        #[allow(clippy::type_complexity)]
        reply: tokio::sync::oneshot::Sender<
            Result<(watch::Receiver<LogSnapshot>, u64, ForwarderProgress), String>,
        >,
    },
    Detach {
        sid: ServerSessionId,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Prompt {
        sid: ServerSessionId,
        text: String,
        images: Vec<ImageAttachment>,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Steer {
        sid: ServerSessionId,
        text: String,
        images: Vec<ImageAttachment>,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Headless "start-work" enqueue (ADR-0015): same as `Prompt` but with NO
    /// owner gate. The handler calls `enqueue_prompt` directly, so a non-GUI
    /// caller can drive a turn on a session it does not own.
    AdminPrompt {
        session_id: ServerSessionId,
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Cancel {
        sid: ServerSessionId,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Close {
        sid: ServerSessionId,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Restart {
        sid: ServerSessionId,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Rename {
        sid: ServerSessionId,
        label: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetArchived {
        sid: ServerSessionId,
        archived: bool,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetPermissionMode {
        sid: ServerSessionId,
        mode: PermissionMode,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetModel {
        sid: ServerSessionId,
        model_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    ListSessions {
        reply: tokio::sync::oneshot::Sender<Vec<SessionInfo>>,
    },
    AdminQuery {
        reply: tokio::sync::oneshot::Sender<AdminSnapshot>,
    },
    SessionCount {
        reply: tokio::sync::oneshot::Sender<usize>,
    },

    // ── Spawn-worker sourced (channel (re)spawn completed) ──
    // The freshly-spawned client's `handle` (Send surface) is installed in the
    // map; the OWNING pump thread is spawned by the worker AFTER the actor
    // replies the committed generation. The actor never receives or drops the
    // client. `is_respawn` bumps generation (and gen_watch) so the old pump
    // self-terminates and drops its client off-actor (Blocker A).
    PublishChannel {
        sid: ServerSessionId,
        handle: TransportHandle,
        /// Generation observed when this spawn was requested. Archive/restart
        /// may fence a blocking handshake before it returns.
        expected_generation: u64,
        is_respawn: bool,
        /// The spawn attempted `session/load` (it had a resume id) — arms the
        /// replay fence and is recorded as `ChannelOpened { resumed }`.
        resumed: bool,
        // On success: (committed generation, gen_watch subscription, replay
        // fence, turn base) — everything the OWNING pump needs to drive +
        // self-terminate. `turn base` is the session's settled turn count at
        // publish: the channel's own counter restarts at 0 every spawn, so the
        // pump reports TurnCounts as `base + channel count` to keep the
        // durable numbering monotonic. `None` if the session was closed while
        // spawning.
        // type alias would hurt readability here more than help
        #[allow(clippy::type_complexity)]
        reply: tokio::sync::oneshot::Sender<Option<(u64, watch::Receiver<u64>, usize, usize)>>,
    },
    SpawnFailed {
        sid: ServerSessionId,
        expected_generation: u64,
        reason: String,
    },

    // ── Pump-thread sourced (fire-and-forget; generation-fenced) ──
    Record {
        sid: ServerSessionId,
        generation: u64,
        event: yalda::acp_channel::ReplyEvent,
    },
    TurnCount {
        sid: ServerSessionId,
        generation: u64,
        turns: usize,
    },
    /// The pump observed the worker's end-of-replay marker on a resumed
    /// channel and dropped its fence; clear the actor's copy so a later
    /// `PublishChannel` doesn't seed a new pump with a stale fence.
    ReplayDone {
        sid: ServerSessionId,
        generation: u64,
    },
    AgentDisconnected {
        sid: ServerSessionId,
        generation: u64,
    },
}

/// CLI: with no subcommand the binary runs the server (the default the GUI
/// auto-launches); subcommands manage launchd supervision.
#[derive(clap::Parser)]
#[command(
    name = "yalda-session-server",
    about = "Yalda ACP session-server daemon"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Subcmd>,
}

#[derive(clap::Subcommand)]
enum Subcmd {
    /// Install + load the launchd LaunchAgent: the server starts at login and
    /// is restarted automatically if it crashes (so agent sessions run with no
    /// GUI present). Hands off any running server losslessly via its WAL.
    Install,
    /// Unload + remove the launchd LaunchAgent.
    Uninstall,
    /// Show whether the LaunchAgent is installed/loaded and the socket is live.
    Status,
    /// Enqueue a prompt to an existing session with no GUI attached (headless
    /// start-work). Connects to the already-running server and drives a turn on
    /// a session this CLI does not own (ADR-0015); the agent runs it to
    /// completion with no GUI ever attaching.
    Prompt {
        /// The id of the existing session to enqueue the prompt to.
        session_id: String,
        /// The prompt text to send to the agent.
        text: String,
    },
}

// ── Managed session ────────────────────────────────────────────────

/// Runtime lifecycle is intentionally separate from `channel: Option<_>`.
/// `None` alone cannot distinguish a handshake that will publish shortly from
/// a terminally disconnected process, and treating both alike is what allowed
/// prompts to queue forever after a failed spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionLifecycle {
    Spawning,
    Live,
    Restarting,
    Disconnected,
    Archived,
}

#[derive(Debug, Clone)]
struct PendingPrompt {
    id: u64,
    payload: PromptPayload,
}

struct ManagedSession {
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    provider: AgentProvider,
    /// Last agent-owned session id. Unlike `channel.session_id()`, this
    /// survives dropping the live transport for cold archive.
    acp_session_id: Option<String>,
    /// Durable lifecycle state. Archived sessions retain transcript metadata
    /// but own neither a transport nor an open WAL handle.
    archived: bool,
    /// Explicit runtime state. The transport option remains the data-bearing
    /// handle; this field owns the policy for prompt/cancel/retry behavior.
    lifecycle: SessionLifecycle,
    /// The live ACP transport surface — the Send sub-handles of the
    /// `AcpChannelClient` whose `reply_rx` is owned by the pump thread. `None`
    /// while the subprocess is being spawned. The actor never holds the client
    /// itself (so its blocking `Drop` never runs on the actor task).
    channel: Option<TransportHandle>,
    /// Bumped every time `channel` is replaced (force-restart). The apply
    /// handlers fence stale pump messages on this (Blocker B, CP5), and the
    /// `gen_watch` mirror lets the old pump self-terminate (Blocker A).
    channel_generation: u64,
    /// Mirrors `channel_generation` so each pump thread can observe a restart
    /// (generation bump) and self-terminate + drop its owned client off the
    /// actor task (Blocker A).
    gen_watch: watch::Sender<u64>,
    turns: usize,
    permission_mode: PermissionMode,
    /// Desired model selection, persisted and re-applied after every spawn.
    model_id: Option<String>,
    /// A turn is in flight (bug-0022). Set when a prompt is accepted by the
    /// channel, cleared when the turn completes (`TurnCount`) or a channel is
    /// (re)spawned — a respawn kills whatever was running, so a stale `true`
    /// would strand a session showing "working" forever. Surfaced on
    /// [`SessionInfo::busy`] and broadcast as `SessionBusy` so every GUI can show
    /// live status for sessions it is not attached to.
    busy: bool,
    /// Per-session transcript log channel. Holds the latest snapshot of
    /// `event_log` (as a cloned `Arc`); every `record`/`log_only` sends the
    /// updated snapshot via `send_replace`. The forwarder tails `[sent..]` of
    /// the latest snapshot lock-free — watch coalescing self-heals exactly like
    /// the old broadcast `Lagged` path.
    log_tx: watch::Sender<LogSnapshot>,
    /// The single attached client's forwarder progress handle (Bug 1b), or
    /// `None` when no client is attached (strict 1:1). A shared `AtomicU64`
    /// holding that forwarder's last forwarded logical `sent_seq`; `push_event`
    /// uses it as the trim floor so a trim never gaps the live forwarder. The
    /// handle is pruned when the forwarder task drops its clone
    /// (`Arc::strong_count == 1`).
    forwarder: Option<ForwarderProgress>,
    /// Prompts that arrived before the ACP subprocess finished spawning.
    /// Drained in submission order once `channel` becomes `Some`.
    pending_prompts: Vec<PendingPrompt>,
    /// Monotonic identity for prompt intent/terminal WAL records.
    next_prompt_id: u64,
    /// Every notification ever broadcast for this session, so a
    /// re-attaching GUI can replay the full transcript.
    ///
    /// Wrapped in `Arc` so `attach` clones a *pointer* under the
    /// global lock, not the whole (unbounded) `Vec`. Pushes go through
    /// `Arc::make_mut`, which is a cheap in-place mutation whenever the only
    /// reference is this field (the common case — snapshots are short-lived and
    /// released before the next push).
    ///
    /// Phase-8 Stage B (spec §6): now an [`EventLog`] ringbuffer — the IN-MEMORY
    /// `Vec` is bounded to [`event_log_cap`], with a logical `log_base` seq
    /// offset so a trim never re-aliases a client's acked `seq`. The on-disk WAL
    /// stays append-only / unbounded.
    event_log: yalda::event_log::EventLog,
    /// Replay-fence arm state, re-derived at every channel publish
    /// (`apply_channel_state`): nonzero (the settled turn count) when the
    /// channel was spawned with a resume id AND `event_log` already holds the
    /// history `session/load` will re-emit. The pump suppresses Records while
    /// its (marker-based, see `yalda::replay_fence`) fence is up and sends
    /// `ReplayDone` when the worker's end-of-replay marker clears it; this
    /// field is the actor's mirror so a later publish never seeds a pump with
    /// a stale fence. Zero for fresh sessions and non-resumed channels.
    replay_fence: usize,
    /// Durable write-ahead log for this session (ADR-0009). Every logged event
    /// is appended here so a crash (not just a clean shutdown) preserves the
    /// transcript. `None` only if the WAL couldn't be opened (we degrade to
    /// in-memory-only rather than refusing to run).
    wal: Option<yalda::session_wal::SessionWal>,
    /// Stable path retained while `wal` is closed so unarchive can reopen it
    /// and explicit close can still remove it.
    wal_path: Option<PathBuf>,
    /// Phase-8 Stage A (spec §2/§3): the authoritative durable `seq` for the
    /// canonical `AgentEvent` envelope — monotonic per `(session, generation)`,
    /// assigned at the server's `record()` chokepoint. During the additive
    /// rollout (spec §9) the `Agent` records interleave with the legacy
    /// `ReplyEvent`/`TurnEnded` records in the SAME `event_log`, so this seq is a
    /// dedicated logical counter for the agent stream, NOT the `Vec` index (the
    /// `seq == Vec position` identity the spec resolves to is the post-deletion
    /// steady state, once the legacy variants are gone). Reset to 0 on every
    /// channel (re)spawn alongside `channel_generation`.
    agent_seq: u64,
    /// Outbound tap for the external-chat bridge (T-004, spec §5). Every logged
    /// notification is forwarded as `(session_id, note)` so the bridge can fold
    /// it into this session's topic. `None` when no bridge sender is wired
    /// (e.g. tests); a send whose receiver was dropped (bridge disabled) errors
    /// and is ignored, so this never buffers unboundedly.
    bridge_tx: Option<BridgeTx>,
}

/// Sender for the outbound bridge tap (T-004): the canonical `push_event`
/// chokepoint forwards each logged `(session_id, note)` to the external-chat
/// bridge. The `Manager` owns the authoritative sender and hands a clone to
/// every session (live + recovered) so all of a session's transcript streams.
type BridgeTx = tokio::sync::mpsc::UnboundedSender<(ServerSessionId, Notification)>;

/// Apply the Stage-B in-memory bound to one event log, including the honest
/// `CompactedSummary` marker required by spec §6.  This is shared by the live
/// append path and WAL recovery: recovery must compact *before* publishing the
/// first watch snapshot, otherwise a client can attach to the unbounded WAL
/// image and pin it before the first live event gets a chance to trim it.
fn compact_event_log(
    event_log: &mut yalda::event_log::EventLog,
    session_id: &str,
    generation: u64,
    cap: usize,
    floor: u64,
) -> Option<usize> {
    // Low-water mark: ¾ of the cap, leaving a slot for the prepended marker
    // and headroom so the next several pushes don't re-trim.
    let target = (cap * 3 / 4).max(1).min(cap.saturating_sub(1));
    let trim = event_log.trim(cap, target, floor)?;

    // The marker reuses the LAST-DROPPED slot: `prepend` decrements `log_base`
    // by one so survivor seqs remain stable.
    let through_turn = trim.through_turn.unwrap_or(0);
    let marker_seq = trim.new_base.saturating_sub(1);
    let marker = yalda::agent_event::AgentEvent::new(
        session_id.to_string(),
        generation,
        through_turn,
        marker_seq,
        yalda::agent_event::AgentEventKind::CompactedSummary {
            through_turn,
            summary: format!(
                "history compacted: {} earlier event(s) trimmed (through turn {through_turn})",
                trim.dropped
            ),
        },
    );
    event_log.prepend(Notification::Agent { event: marker });
    Some(trim.dropped)
}

/// Rebuild and immediately bound one WAL transcript.  Keeping construction and
/// compaction behind one function makes it impossible for recovery callers to
/// accidentally publish the raw, unbounded `Vec` again.
fn event_log_from_recovery(
    entries: Vec<Notification>,
    session_id: &str,
    generation: u64,
    cap: usize,
) -> (yalda::event_log::EventLog, usize) {
    let mut event_log = yalda::event_log::EventLog::from_recovered(entries, 0);
    let dropped =
        compact_event_log(&mut event_log, session_id, generation, cap, u64::MAX).unwrap_or(0);
    (event_log, dropped)
}

impl ManagedSession {
    fn info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.id.clone(),
            acp_session_id: self.acp_session_id.clone(),
            label: self.label.clone(),
            cwd: self.cwd.clone(),
            provider: self.provider,
            turns: self.turns,
            connected: self.channel.as_ref().is_some_and(|c| c.is_connected()),
            permission_mode: self.permission_mode,
            busy: self.busy,
            archived: self.archived,
        }
    }

    /// Record that an event happened: append it to the durable `event_log`
    /// (source of truth) **and** fire the broadcast wake in one step. This is
    /// the single mutator for "a logged event happened" — every log+broadcast
    /// site routes through here so the two writes can never skew (one appended
    /// without waking subscribers, or one broadcast without being logged).
    fn record(&mut self, note: Notification) {
        self.push_event(note);
    }

    /// The single in-memory + WAL push, with Stage B ringbuffer trim (spec §6).
    /// Appends to the durable WAL (unbounded), pushes onto the bounded in-memory
    /// [`EventLog`], trims the front (with hysteresis) when it exceeds
    /// [`event_log_cap`] — splicing a `CompactedSummary` marker so a trim
    /// surfaces as a deterministic placeholder (NOT a silent drop) — then
    /// publishes the new snapshot on `log_tx`.
    ///
    /// HYSTERESIS (spec §11 / risk #2): a `Vec` front-drain + the marker
    /// `prepend(0)` are each O(resident), so trimming one entry per push at the
    /// cap would be O(cap) per push. We trim only when over `cap` and drop down
    /// to a low-water `target` (≈ ¾ cap), amortising the cost across many pushes.
    ///
    /// COMPACTION FLOOR (spec §6, N3 / Bug 1b): the trim treats every LIVE
    /// forwarder as a hard ceiling — it never drops below the slowest live
    /// forwarder's last-forwarded `sent_seq`, and since the owner (lease holder)
    /// is always one of the live forwarders, the owner is never gapped mid-stream
    /// ("never compact past owner.acked_seq"). The floor is `min(sent_seq)` over
    /// [`compaction_floor`]; with no live forwarders it is `u64::MAX` (nothing to
    /// protect → cap-only).
    ///
    /// HIGH-WATER DISCONNECT (spec §6, MAJOR): the forwarder hard-ceiling means a
    /// slow/paused subscriber (e.g. a backgrounded GUI under App Nap that stops
    /// draining its socket) pins the floor — the trim can't fire and the `Vec`
    /// grows. The 60s slow-sub write timeout is the only other reaper, and a
    /// forwarder that drains just enough to keep resetting that timer could pin
    /// growth EFFECTIVELY unbounded. So BEFORE computing the floor we
    /// [`enforce_high_water`](Self::enforce_high_water): when the backlog
    /// (`tip_seq - floor`) crosses [`event_log_high_water`], the forwarder is
    /// force-DISCONNECTED (a clean from-base reconnect, NOT a silent gap) and
    /// dropped from the floor, so the trim resumes and growth is bounded.
    fn push_event(&mut self, note: Notification) {
        self.wal_append(&note);
        // Outbound bridge tap (T-004): forward every logged notification to the
        // external-chat bridge, keyed by session, at this single chokepoint —
        // clone BEFORE `note` is moved into the log. A dropped receiver (bridge
        // disabled) errors and is ignored, so this never buffers.
        if let Some(tx) = &self.bridge_tx {
            let _ = tx.send((self.id.clone(), note.clone()));
        }
        self.event_log.push(note);
        let cap = yalda::event_log::event_log_cap();
        // Disconnect-before-gap (spec §6): evict any forwarder whose backlog has
        // crossed the high-water bound, so the floor below is not pinned by a
        // wedged consumer and the trim can bound growth.
        self.enforce_high_water();
        let floor = self.compaction_floor();
        compact_event_log(
            &mut self.event_log,
            &self.id,
            self.channel_generation,
            cap,
            floor,
        );
        self.publish_snapshot();
    }

    /// Publish the current `event_log` (plus the live `channel_generation`) on the
    /// `log_tx` watch, waking the forwarder. The forwarder re-resolves its logical
    /// `sent_seq` against the published `log_base` (Bug 1a), so a trim that
    /// shortened the `Vec` can never make it slice a stale offset.
    fn publish_snapshot(&self) {
        let _ = self.log_tx.send_replace(LogSnapshot {
            log: self.event_log.clone(),
            generation: self.channel_generation,
        });
    }

    /// The trim floor (Bug 1b): the single attached forwarder's last-forwarded
    /// logical `sent_seq`, or `u64::MAX` (no floor — cap-only) when no client is
    /// attached. A dead forwarder dropped its progress `Arc`, so it is the SOLE
    /// remaining ref (`strong_count == 1`) and is pruned here. The trim never
    /// drops below the live forwarder's forwarded position, so the subscriber is
    /// never gapped mid-stream.
    fn compaction_floor(&mut self) -> u64 {
        use std::sync::atomic::Ordering;
        self.prune_dead_forwarder();
        self.forwarder
            .as_ref()
            .map(|p| p.sent_seq.load(Ordering::Acquire))
            .unwrap_or(u64::MAX)
    }

    /// Drop the forwarder handle if the forwarder task has exited (it was the
    /// sole remaining ref to the `Arc`).
    fn prune_dead_forwarder(&mut self) {
        if matches!(&self.forwarder, Some(p) if Arc::strong_count(p) == 1) {
            self.forwarder = None;
        }
    }

    /// High-water backlog bound (spec §6, MAJOR — disconnect-before-gap).
    ///
    /// The floor ([`compaction_floor`]) is a HARD ceiling, so a slow/paused
    /// forwarder (e.g. a backgrounded GUI under macOS App Nap that stops draining
    /// its socket) pins `sent_seq` and prevents the trim from firing, letting the
    /// in-memory `Vec` grow without bound. When the backlog `tip_seq - sent_seq`
    /// crosses [`event_log_high_water`], force-DISCONNECT the forwarder: set its
    /// `evicted` flag (the forwarder task observes it on its next wake — the
    /// `publish_snapshot` at the end of `push_event` provides that wake — and
    /// returns, closing its write half) and drop its handle HERE so it
    /// immediately drops out of the floor. The trim then proceeds and growth is
    /// bounded.
    ///
    /// This is NOT a silent in-place gap (which §6 forbids): the disconnected
    /// client gets a clean EOF, reconnects, and rebuilds from base via
    /// `resolve_cursor` → `FromBase` (surfacing the `CompactedSummary` marker).
    fn enforce_high_water(&mut self) {
        use std::sync::atomic::Ordering;
        self.prune_dead_forwarder();
        let high_water = yalda::event_log::event_log_high_water() as u64;
        let tip = self.event_log.tip_seq();
        let Some(handle) = self.forwarder.as_ref() else {
            return; // no forwarder → cap-only mode, nothing to evict
        };
        let floor = handle.sent_seq.load(Ordering::Acquire);
        // Saturating so a forwarder somehow ahead of tip can't underflow.
        let backlog = tip.saturating_sub(floor);
        if backlog <= high_water {
            return; // forwarder is within the bound — done
        }
        // Force-disconnect: flag it (the forwarder task exits at its next wake)
        // and drop the actor's handle now so the floor no longer includes it and
        // the trim can proceed.
        handle.evicted.store(true, Ordering::Release);
        tracing::warn!(
            session_id = %&self.id[..8.min(self.id.len())],
            backlog,
            high_water,
            "high-water disconnect: evicting forwarder (sent_seq {floor}) — \
             in-memory backlog past threshold (wedged/paused consumer)"
        );
        self.forwarder = None;
    }

    /// Phase-8 Stage A (spec §1/§2/§3, ADDITIVE): record the canonical
    /// `AgentEvent` for `kind`, ALONGSIDE the legacy notification the caller
    /// already recorded. This is the server's piece of the §3 emit chokepoint:
    /// it assigns the authoritative envelope identity under the actor's single-
    /// writer discipline — `session_id` = this session, `generation` =
    /// `channel_generation`, `turn` = `self.turns`, `seq` = a monotonic
    /// `agent_seq` (per `(session, generation)`). seq/turn ride the durable WAL
    /// for free (the envelope is inside the persisted `Notification::Agent`), so
    /// on resume the server forwards them verbatim (spec §5).
    ///
    /// Emitting alongside the inference (not instead of it) is the spec §9
    /// reversible rollout: deleting the legacy path is a follow-up once the
    /// forwarded stream is confirmed to agree with the inference on real
    /// sessions. `turn` is the CURRENT (post-increment for TurnEnded) settled
    /// count — callers that record a TurnEnded must bump `self.turns` first so
    /// the boundary's envelope `turn` matches the completed turn number.
    fn record_agent(&mut self, kind: yalda::agent_event::AgentEventKind) {
        let event = yalda::agent_event::AgentEvent::new(
            self.id.clone(),
            self.channel_generation,
            self.turns as u64,
            self.agent_seq,
            kind,
        );
        self.agent_seq += 1;
        self.record(Notification::Agent { event });
    }

    /// Append a transcript event to `event_log` + WAL and fire the watch wake.
    /// Used for the user's own prompt: the live GUI already rendered it locally,
    /// and its transcript reconciler dedups the prompt it then sees replayed via
    /// the log tail (the watch delivers every event_log entry, same as the old
    /// broadcast tail did). Distinguished from [`record`] only in intent — both
    /// now publish through the per-session `log_tx` watch.
    fn log_only(&mut self, note: Notification) {
        self.push_event(note);
    }

    /// Append `note` to the durable WAL. `fsync`s at turn boundaries
    /// (`UserPrompt` / `TurnEnded`) so a completed turn or a sent prompt is
    /// never lost on power loss, but not per streamed chunk (ADR-0009). A WAL
    /// error is logged, never fatal — the in-memory `event_log` still holds the
    /// event for live subscribers.
    fn wal_append(&mut self, note: &Notification) {
        if let Some(wal) = self.wal.as_mut() {
            // Turn boundaries fsync (UserPrompt / TurnEnded), streamed chunks do
            // not (ADR-0009). Phase-8 Stage A: an `Agent` record carrying a
            // boundary kind (UserMessage or a real TurnEnded) is the same
            // guarantee in the new vocabulary — fsync those too. ReplayEnd is a
            // replay-prefix marker, not a completed turn, so it need not fsync,
            // but it's cheap and harmless to treat all TurnEnded alike here.
            let boundary = match note {
                Notification::UserPrompt { .. } | Notification::TurnEnded { .. } => true,
                Notification::Agent { event } => {
                    use yalda::agent_event::AgentEventKind;
                    matches!(
                        event.kind,
                        AgentEventKind::UserMessage { .. } | AgentEventKind::TurnEnded { .. }
                    )
                }
                _ => false,
            };
            if let Err(e) = wal.append(note, boundary) {
                tracing::error!(
                    session_id = %&self.id[..8.min(self.id.len())],
                    error = %e,
                    "WAL append failed"
                );
            }
        }
    }

    fn begin_prompt_intent(&mut self, payload: PromptPayload) -> Result<PendingPrompt, String> {
        let id = self.next_prompt_id;
        if let Some(wal) = self.wal.as_mut() {
            wal.append_prompt_intent(id, &payload)
                .map_err(|error| format!("could not persist prompt before delivery: {error}"))?;
        }
        self.next_prompt_id = self.next_prompt_id.saturating_add(1);
        Ok(PendingPrompt { id, payload })
    }

    fn finish_prompt_intent(&mut self, id: u64, outcome: yalda::session_wal::PromptOutcome) {
        if let Some(wal) = self.wal.as_mut()
            && let Err(error) = wal.append_prompt_terminal(id, outcome)
        {
            // Delivery has already happened (or terminal rejection/cancel was
            // already decided). Keep serving live traffic but make the
            // at-least-once recovery risk explicit in the log.
            tracing::error!(
                session_id = %&self.id[..8.min(self.id.len())],
                prompt_id = id,
                %error,
                "WAL prompt terminal append failed; recovery may retry this intent"
            );
        }
    }

    /// Publish a freshly-spawned channel's `TransportHandle` as this session's
    /// live transport, running the full attach choreography atomically under the
    /// caller's lock. The single chokepoint for create / restore / restart (9′)
    /// so the three can't drift:
    /// 1. Re-apply the session's `permission_mode` (a fresh channel starts at
    ///    its default — without this the configured mode silently reverts).
    /// 2. Drain `pending_prompts` in arrival order onto the new transport BEFORE
    ///    publishing it, so they're enqueued at the ACP driver before any
    ///    future prompt races in. Doing this under the lock also closes the
    ///    take-then-publish window where a concurrent `prompt()` could re-queue
    ///    onto a `pending_prompts` we'd already drained.
    /// 3. On a respawn (force-restart), bump `channel_generation` AND the
    ///    `gen_watch` mirror so (a) the apply handlers fence the old pump's
    ///    in-flight messages (Blocker B, CP5) and (b) the OLD pump thread
    ///    observes the bump and self-terminates + drops its owned client off the
    ///    actor task (Blocker A).
    /// 4. Swap the handle in and `record(SessionAttached)`.
    ///
    /// Unlike the old client-owning version this returns nothing: the actor only
    /// ever holds the cheap Send `TransportHandle`; the owning `AcpChannelClient`
    /// (and its blocking `Drop`) lives on the pump's OS thread.
    /// `resumed` = the spawn ATTEMPTED `session/load` (it had a resume id), so
    /// the worker will emit an end-of-replay marker after its handshake. That
    /// is the only condition under which the replay fence may be armed: an
    /// unarmed channel never emits the marker, and a fence with no marker
    /// coming never clears — discarding every live event forever (the
    /// resume-hang bug).
    /// Install the (re)spawned channel. Returns the queued prompts that FAILED
    /// to flush onto the new channel as `(text, reason)` so the caller can
    /// surface a `PromptRejected` for each — these were optimistically echoed in
    /// the GUI when the user typed them while the channel was down, so a silent
    /// drop here is invisible data loss (spec-session-recall-integrity B1/B2: the
    /// "I see the message, the agent doesn't" bug). Empty on the happy path.
    #[must_use]
    fn apply_channel_state(
        &mut self,
        mut handle: TransportHandle,
        is_respawn: bool,
        resumed: bool,
    ) -> Vec<(String, String)> {
        handle.set_permission_mode(self.permission_mode);
        if let Some(model_id) = &self.model_id {
            handle.set_model(model_id);
        }
        let mut undelivered: Vec<(String, String)> = Vec::new();
        // bug-0022: a (re)spawn kills whatever turn was running, so the in-flight
        // flag starts false and is re-raised only by a queued prompt that
        // actually flushes onto the new channel. Without the reset a session
        // whose agent was restarted mid-turn would show "working" forever.
        self.busy = false;
        for pending in std::mem::take(&mut self.pending_prompts) {
            let text = pending.payload.text.clone();
            match handle.send_payload(pending.payload) {
                Ok(()) => {
                    self.finish_prompt_intent(
                        pending.id,
                        yalda::session_wal::PromptOutcome::Delivered,
                    );
                    self.busy = true;
                }
                Err(e) => {
                    self.finish_prompt_intent(
                        pending.id,
                        yalda::session_wal::PromptOutcome::Rejected,
                    );
                    tracing::error!(error = %e, "queued prompt failed to flush — notifying submitter");
                    undelivered.push((
                        text,
                        format!("queued message was not delivered on reconnect: {e}"),
                    ));
                }
            }
        }
        let acp_session_id = handle.session_id();
        if acp_session_id.is_some() {
            self.acp_session_id = acp_session_id.clone();
        }
        if is_respawn {
            self.channel_generation = self.channel_generation.wrapping_add(1);
            let _ = self.gen_watch.send_replace(self.channel_generation);
            // Phase-8 Stage A (spec §2/§4): a respawn is a NEW channel, so the
            // per-(session, generation) agent seq restarts at 0. The
            // `ChannelOpened` first-event below then rides this new generation
            // with seq 0 — the uniform rebaseline signal a consumer keys on
            // (spec §4 rebaseline rule).
            //
            // NOTE (minor, re-review): this resets `agent_seq` to 0 but does NOT
            // clear `event_log` — the respawn's events APPEND to the existing
            // in-memory log. That is correct because `event.seq` is the
            // PER-GENERATION envelope seq, which is NOT the forwarding/reconnect
            // cursor. The cursor is ALWAYS the logical `log_base + vec_index`
            // computed by `EventLog::seq_of` / resolved by `resolve_cursor`
            // (which carry the generation alongside the seq, so a generation
            // mismatch forces a from-base rebuild — see `CursorResolution`). A
            // future client-wiring phase (phase-5 cursor reconnect) MUST source
            // its acked position from `seq_of(vec_index)` of the entry it last
            // consumed, NEVER from `event.seq` — the two diverge after a respawn
            // (agent_seq restarts at 0 while the log_base seq keeps climbing) and
            // after any trim.
            self.agent_seq = 0;
        }
        handle.generation = self.channel_generation;
        self.channel = Some(handle);
        self.lifecycle = SessionLifecycle::Live;
        // Arm (or disarm) the replay fence for THIS channel. A resumed channel
        // re-emits `self.turns` turns of history that are already in
        // `event_log` — the pump suppresses them until the worker's
        // end-of-replay marker (`ReplayComplete`). `self.turns == 0` (a fresh
        // session adopting an existing ACP session via create-with-resume)
        // leaves the fence down: the log is empty, so the replay is exactly
        // the content we WANT recorded. A non-resumed channel emits no
        // marker, so any stale fence from a prior channel MUST be cleared
        // here or it would discard the new channel's live events forever.
        self.replay_fence = if resumed { self.turns } else { 0 };
        // ADDITIVE (spec §4/§9): emit `ChannelOpened` as the FIRST event of this
        // (re)spawned channel, BEFORE `SessionAttached`. `resumed` is true when
        // we are reconnecting to an existing ACP session id (the respawn /
        // restore case). This rides the new generation + seq 0 so a consumer
        // that adopts the uniform rebaseline rule (Stage C) resets before
        // applying. It is recorded alongside the kept `SessionAttached` control
        // variant, which still carries the resume id for WAL recovery.
        //
        // MINOR (4) — INTENTIONAL SPEC DEVIATION: spec §2/§4 says envelope facts
        // (incl. `ChannelOpened`) are sourced "at the worker". In Stage A the
        // server sources `ChannelOpened` here instead (it owns the authoritative
        // `channel_generation` and the spawn lifecycle). A future reader must NOT
        // assume these envelopes are worker-sourced: the worker-sourced emission
        // (§4 "emit before the load RPC") is a later-phase move; today the SERVER
        // stamps the generation + seq 0 at this site.
        // `resumed` here is the real resume-attempted flag — NOT
        // `acp_session_id.is_some()`, which is true for every successful
        // handshake (session/new also yields an id) and so mislabeled fresh
        // channels as resumed.
        self.record_agent(yalda::agent_event::channel_opened_kind(resumed));
        self.record(Notification::SessionAttached {
            session_id: self.id.clone(),
            acp_session_id,
        });
        undelivered
    }
}

// ── Session manager ────────────────────────────────────────────────

/// Build a fresh `ManagedSession` for a brand-new session.
fn new_managed_session(
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    provider: AgentProvider,
    permission_mode: PermissionMode,
    wal: Option<yalda::session_wal::SessionWal>,
    bridge_tx: Option<BridgeTx>,
) -> ManagedSession {
    let wal_path = wal.as_ref().map(|wal| wal.path().to_path_buf());
    let event_log = yalda::event_log::EventLog::new();
    let (log_tx, _) = watch::channel(LogSnapshot {
        log: event_log.clone(),
        generation: 0,
    });
    let (gen_watch, _) = watch::channel(0u64);
    ManagedSession {
        id,
        label,
        cwd,
        provider,
        acp_session_id: None,
        archived: false,
        lifecycle: SessionLifecycle::Spawning,
        channel: None,
        channel_generation: 0,
        gen_watch,
        turns: 0,
        permission_mode,
        model_id: None,
        busy: false,
        log_tx,
        forwarder: None,
        pending_prompts: Vec::new(),
        next_prompt_id: 1,
        event_log,
        replay_fence: 0,
        wal,
        wal_path,
        agent_seq: 0,
        bridge_tx,
    }
}

/// A pending ACP resume job produced by WAL recovery — the seed map plus the
/// data each resume worker needs to re-spawn its subprocess.
struct ResumeJob {
    session_id: ServerSessionId,
    cwd: PathBuf,
    provider: AgentProvider,
    acp_session_id: Option<String>,
    expected_generation: u64,
}

/// Resolve the canonical stream position for the first channel published after
/// durable recovery. This is kept at the recovery boundary so the seed state,
/// watch channels, and resume worker cannot choose different generations.
fn recovered_stream_position(event_log: &[Notification]) -> (u64, u64) {
    let generation = event_log
        .iter()
        .filter_map(|note| match note {
            Notification::Agent { event } => Some(event.generation),
            _ => None,
        })
        .max()
        .map(|generation| {
            generation
                .checked_add(1)
                .expect("durable channel generation exhausted")
        })
        .unwrap_or(0);
    // `AgentEvent.seq` is per generation. A recovered server channel is a new
    // generation, so its first `ChannelOpened` must start at seq 0 rather than
    // continuing the maximum seq from an older generation.
    (generation, 0)
}

/// The single-writer actor state: it OWNS the sessions map (no Mutex). Mutated
/// only on the `run_manager` task, one command at a time.
struct Manager {
    sessions: HashMap<ServerSessionId, ManagedSession>,
    /// Manager-level broadcast for session-list changes (create/close/rename).
    events: broadcast::Sender<Notification>,
    default_permission_mode: PermissionMode,
    /// The inlet sender — cloned into spawn workers so they can post back
    /// `PublishChannel` / `SpawnFailed` without touching the map directly.
    cmd_tx: mpsc::UnboundedSender<Command>,
    /// The agent-transport factory (Phase 6 seam). Cloned into every (off-actor)
    /// spawn thread; the shipping binary installs [`RealAgentSpawner`], a test
    /// substitutes a `FakeAgentSpawner`. The actor itself never spawns — it only
    /// hands this `Arc` to the spawn workers.
    spawner: Arc<dyn AgentSpawner>,
    /// The canonical outbound bridge sender (T-004). The Manager is the single
    /// owner; `do_create` clones it into each new session so `push_event` can
    /// tap the transcript. `None` when the server runs without a bridge sender.
    bridge_tx: Option<BridgeTx>,
}

/// The public handle the connection handlers hold. All mutation goes through
/// `cmd_tx`; the single-writer `run_manager` actor owns the sessions map. The
/// handle keeps only the inlet sender and the manager-level (session-list)
/// broadcast — there is no shared map and no lock.
struct SessionManager {
    /// Manager-level broadcast for session-list changes (create/close/rename).
    events: broadcast::Sender<Notification>,
    /// The actor command inlet — every request becomes a Command sent here.
    cmd_tx: mpsc::UnboundedSender<Command>,
}

/// Open a fresh durable WAL for a session, or `None` (degrade to in-memory) if
/// no WAL dir is resolvable or the file can't be created — logged, never fatal.
fn open_session_wal(
    id: &str,
    label: &str,
    cwd: &std::path::Path,
    permission_mode: PermissionMode,
    provider: AgentProvider,
) -> Option<yalda::session_wal::SessionWal> {
    let dir = session_wal_dir()?;
    match yalda::session_wal::SessionWal::create_for_provider(
        &dir,
        id,
        label,
        cwd,
        permission_mode,
        provider,
    ) {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::error!(
                session_id = %&id[..8.min(id.len())],
                error = %e,
                "WAL create failed (in-memory only)"
            );
            None
        }
    }
}

impl SessionManager {
    fn new_with_inlet(
        default_permission_mode: PermissionMode,
    ) -> (Self, mpsc::UnboundedReceiver<Command>, PermissionMode) {
        let (events, _) = broadcast::channel(1024);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        (Self { events, cmd_tx }, cmd_rx, default_permission_mode)
    }

    /// Subscribe to manager-level session-list notifications.
    fn subscribe_events(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    // ── Async request wrappers (oneshot round-trip to the actor) ──

    async fn send_create(
        &self,
        cwd: PathBuf,
        label: String,
        provider: AgentProvider,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Create {
            cwd,
            label,
            provider,
            resume_session_id,
            reply,
        });
        rx.await.expect("actor dropped a Create reply")
    }

    async fn send_attach(
        &self,
        sid: &str,
        cursor: Option<(u64, u64)>,
    ) -> Result<(watch::Receiver<LogSnapshot>, u64, ForwarderProgress), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Attach {
            sid: sid.to_string(),
            cursor,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_detach(&self, sid: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Detach {
            sid: sid.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_prompt(
        &self,
        sid: &str,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Prompt {
            sid: sid.to_string(),
            text: text.to_string(),
            images,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_steer(
        &self,
        sid: &str,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Steer {
            sid: sid.to_string(),
            text: text.to_string(),
            images,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    /// Headless ungated enqueue (ADR-0015). No `conn_id` / owner check.
    async fn send_admin_prompt(&self, sid: &str, text: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::AdminPrompt {
            session_id: sid.to_string(),
            text: text.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_cancel(&self, sid: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Cancel {
            sid: sid.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_close(&self, sid: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Close {
            sid: sid.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_restart(&self, sid: &str) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Restart {
            sid: sid.to_string(),
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_rename(&self, sid: &str, label: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Rename {
            sid: sid.to_string(),
            label,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_set_archived(&self, sid: &str, archived: bool) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SetArchived {
            sid: sid.to_string(),
            archived,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_set_permission_mode(
        &self,
        sid: &str,
        mode: PermissionMode,
    ) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SetPermissionMode {
            sid: sid.to_string(),
            mode,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_set_model(&self, sid: &str, model_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SetModel {
            sid: sid.to_string(),
            model_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_list_sessions(&self) -> Vec<SessionInfo> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::ListSessions { reply });
        rx.await.unwrap_or_default()
    }

    async fn send_admin_status(&self) -> AdminSnapshot {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::AdminQuery { reply });
        rx.await.unwrap_or(AdminSnapshot {
            session_count: 0,
            sessions: Vec::new(),
        })
    }

    async fn send_session_count(&self) -> usize {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SessionCount { reply });
        rx.await.unwrap_or(0)
    }
}

/// Recover sessions from their durable WALs (ADR-0009). Returns the SEED map
/// (moved into `run_manager` before the actor starts) plus the resume jobs whose
/// workers re-spawn the ACP subprocesses (each posting `PublishChannel` back
/// into the actor). Runs once at startup before accepting connections.
fn restore_seed_from_disk(
    bridge_tx: Option<BridgeTx>,
) -> (HashMap<ServerSessionId, ManagedSession>, Vec<ResumeJob>) {
    let mut sessions = HashMap::new();
    let mut jobs = Vec::new();
    let Some(dir) = session_wal_dir() else {
        return (sessions, jobs);
    };
    yalda::session_wal::recover_each(&dir, |rs| {
        let sid = rs.server_session_id.clone();
        let acp_session_id = rs.acp_session_id.clone();
        let wal = if rs.archived {
            None
        } else {
            match yalda::session_wal::SessionWal::reopen(rs.path.clone()) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::error!(
                        session_id = %&sid[..8.min(sid.len())],
                        error = %e,
                        "WAL reopen failed"
                    );
                    None
                }
            }
        };

        let (channel_generation, agent_seq) = recovered_stream_position(&rs.event_log);
        // Stage B: recovery always starts from `log_base == 0` — the on-disk WAL
        // is never trimmed, so the restored transcript is a faithful append-
        // ordered prefix from seq 0 (spec §6 / ringbuffer note: on restart
        // log_base resets to the seq of the first recovered event, which is 0).
        // Recovery previously exposed the entire WAL image until the first live
        // append.  A fast GUI attach could install a floor at seq 0 first,
        // preventing the trim and replaying hundreds of thousands of events.
        // Compact while there are no subscribers, before the watch/actor/accept
        // loop can publish this session.  The durable WAL remains untouched.
        let (event_log, recovered_dropped) =
            event_log_from_recovery(rs.event_log, &sid, 0, yalda::event_log::event_log_cap());
        // Seed the watch with the recovered log so the first tail sees history.
        let (log_tx, _) = watch::channel(LogSnapshot {
            log: event_log.clone(),
            generation: channel_generation,
        });
        let (gen_watch, _) = watch::channel(channel_generation);
        let session = ManagedSession {
            id: sid.clone(),
            label: rs.label.clone(),
            cwd: rs.cwd.clone(),
            provider: rs.provider,
            acp_session_id: acp_session_id.clone(),
            archived: rs.archived,
            lifecycle: if rs.archived {
                SessionLifecycle::Archived
            } else {
                SessionLifecycle::Spawning
            },
            channel: None,
            channel_generation,
            gen_watch,
            turns: rs.turns,
            permission_mode: rs.permission_mode,
            model_id: rs.model_id.clone(),
            // A recovered session has no live turn — whatever was running died
            // with the previous process (bug-0022).
            busy: false,
            log_tx,
            forwarder: None,
            pending_prompts: rs
                .pending_prompts
                .iter()
                .map(|pending| PendingPrompt {
                    id: pending.id,
                    payload: pending.payload.clone(),
                })
                .collect(),
            next_prompt_id: rs.next_prompt_id,
            event_log,
            replay_fence: rs.turns,
            wal,
            wal_path: Some(rs.path.clone()),
            agent_seq,
            // Recovered sessions must stream too (spec §5): hand each the same
            // canonical bridge sender so a resumed session's transcript folds
            // into its topic just like a live one.
            bridge_tx: bridge_tx.clone(),
        };

        tracing::info!(
            session_id = %&sid[..8.min(sid.len())],
            events = session.event_log.len(),
            recovered_dropped,
            turns = rs.turns,
            acp_session_id = %acp_session_id.as_deref().unwrap_or("<none>"),
            archived = rs.archived,
            "recovering session"
        );

        sessions.insert(sid.clone(), session);
        if !rs.archived {
            jobs.push(ResumeJob {
                session_id: sid,
                cwd: rs.cwd,
                provider: rs.provider,
                acp_session_id,
                expected_generation: channel_generation,
            });
        }
    });
    (sessions, jobs)
}

/// Spawn the OS thread that re-spawns a recovered session's ACP subprocess with
/// `--resume`, then publishes the transport via the actor inlet.
fn spawn_resume_worker(
    cmd_tx: mpsc::UnboundedSender<Command>,
    job: ResumeJob,
    spawner: Arc<dyn AgentSpawner>,
) {
    let failure_tx = cmd_tx.clone();
    let ResumeJob {
        session_id,
        cwd,
        provider,
        acp_session_id,
        expected_generation,
    } = job;
    let failure_sid = session_id.clone();
    let spawn_result = std::thread::Builder::new()
        .name(format!(
            "acp-resume-{}",
            &session_id[..8.min(session_id.len())]
        ))
        .spawn(move || {
            // SAFETY: dedicated spawn thread; see create worker.
            unsafe {
                std::env::set_var("YALDA_SESSION_MANAGED", "1");
            }
            let cmd = configured_agent_command(provider);
            match spawner.spawn(
                provider,
                &cmd,
                Some(cwd),
                acp_session_id.clone(),
                YaldaFrontend::Gpui,
            ) {
                Ok(client) => {
                    // Resume/fresh recovery → is_respawn=false. Recovery already
                    // selected the committed generation, so publish against that
                    // exact seed without incrementing it a second time.
                    //
                    // The recovered generation is strictly newer than every
                    // durable Agent event. The new ChannelOpened is therefore an
                    // unambiguous rebaseline signal even when an older hard
                    // restart already left generation-1 history in the WAL.
                    publish_channel(
                        &cmd_tx,
                        &session_id,
                        client,
                        expected_generation,
                        false,
                        acp_session_id.is_some(),
                    );
                }
                Err(e) => {
                    tracing::error!(
                        session_id = %&session_id[..8.min(session_id.len())],
                        error = %e,
                        "failed to resume session"
                    );
                    let _ = cmd_tx.send(Command::SpawnFailed {
                        sid: session_id,
                        expected_generation,
                        reason: format!("resume failed: {e}"),
                    });
                }
            }
        });
    if let Err(error) = spawn_result {
        let _ = failure_tx.send(Command::SpawnFailed {
            sid: failure_sid,
            expected_generation,
            reason: format!("could not start resume worker: {error}"),
        });
    }
}

// ── Manager actor task ─────────────────────────────────────────────

/// The single-writer actor: owns the sessions map and drains the inlet, one
/// command at a time. Replaces the old mutex-guarded map + per-method locking.
async fn run_manager(
    mut rx: mpsc::UnboundedReceiver<Command>,
    sessions: HashMap<ServerSessionId, ManagedSession>,
    events: broadcast::Sender<Notification>,
    default_permission_mode: PermissionMode,
    cmd_tx: mpsc::UnboundedSender<Command>,
    spawner: Arc<dyn AgentSpawner>,
    bridge_tx: Option<BridgeTx>,
) {
    let mut mgr = Manager {
        sessions,
        events,
        default_permission_mode,
        cmd_tx,
        spawner,
        bridge_tx,
    };
    // Single-writer actor (ADR-0012): drain the inlet one command at a time;
    // `apply` is the only mutator of the session map.
    while let Some(cmd) = rx.recv().await {
        mgr.apply(cmd);
    }
}

impl Manager {
    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Create {
                cwd,
                label,
                provider,
                resume_session_id,
                reply,
            } => {
                let info = self.do_create(cwd, label, provider, resume_session_id);
                let _ = reply.send(info);
            }
            Command::Attach { sid, cursor, reply } => {
                let _ = reply.send(self.do_attach(&sid, cursor));
            }
            Command::Detach { sid, reply } => {
                let _ = reply.send(self.do_detach(&sid));
            }
            Command::Prompt {
                sid,
                text,
                images,
                reply,
            } => {
                let _ = reply.send(self.do_prompt(&sid, &text, images));
            }
            Command::Steer {
                sid,
                text,
                images,
                reply,
            } => {
                let _ = reply.send(self.do_steer(&sid, &text, images));
            }
            Command::AdminPrompt {
                session_id,
                text,
                reply,
            } => {
                // Ungated: enqueue directly, no owner check (ADR-0015).
                let _ = reply.send(self.enqueue_prompt(&session_id, &text, Vec::new()));
            }
            Command::Cancel { sid, reply } => {
                let _ = reply.send(self.do_cancel(&sid));
            }
            Command::Close { sid, reply } => {
                let _ = reply.send(self.do_close(&sid));
            }
            Command::Restart { sid, reply } => {
                let _ = reply.send(self.do_restart(&sid));
            }
            Command::Rename { sid, label, reply } => {
                let _ = reply.send(self.do_rename(&sid, label));
            }
            Command::SetArchived {
                sid,
                archived,
                reply,
            } => {
                let _ = reply.send(self.do_set_archived(&sid, archived));
            }
            Command::SetPermissionMode { sid, mode, reply } => {
                let _ = reply.send(self.do_set_permission_mode(&sid, mode));
            }
            Command::SetModel {
                sid,
                model_id,
                reply,
            } => {
                let _ = reply.send(self.do_set_model(&sid, &model_id));
            }
            Command::ListSessions { reply } => {
                let _ = reply.send(self.sessions.values().map(|s| s.info()).collect());
            }
            Command::AdminQuery { reply } => {
                let _ = reply.send(self.do_admin_status());
            }
            Command::SessionCount { reply } => {
                let _ = reply.send(self.sessions.len());
            }
            Command::PublishChannel {
                sid,
                handle,
                expected_generation,
                is_respawn,
                resumed,
                reply,
            } => {
                let (published, undelivered, busy_now) = match self.sessions.get_mut(&sid) {
                    Some(s) if !s.archived && s.channel_generation == expected_generation => {
                        let undelivered = s.apply_channel_state(handle, is_respawn, resumed);
                        (
                            Some((
                                s.channel_generation,
                                s.gen_watch.subscribe(),
                                s.replay_fence,
                                s.turns,
                            )),
                            undelivered,
                            Some(s.busy),
                        )
                    }
                    Some(_) | None => (None, Vec::new(), None),
                };
                // bug-0022: publish the post-spawn in-flight state to every GUI —
                // a respawn clears it (unless a queued prompt flushed), and a GUI
                // that isn't attached has no other way to learn that.
                if let Some(busy) = busy_now {
                    self.broadcast_busy(&sid, busy);
                }
                // bug-0027: this is the moment the agent subprocess becomes
                // live. `SessionCreated` was necessarily sent with
                // `connected: false` (it precedes the blocking handshake), so
                // this is the ONLY thing that can move a GUI's roster off
                // "Unavailable" without a full reseed.
                if published.is_some() {
                    self.broadcast_connected(&sid, true);
                }
                // Queued prompts that failed to flush onto the (re)spawned channel
                // were optimistically echoed in the GUI — surface each as a
                // transient `PromptRejected` (the manager broadcast reaches the
                // session's subscribers) so the user learns it didn't land and
                // gets the text back, instead of silent data loss
                // (spec-session-recall-integrity B1/B2).
                for (text, reason) in undelivered {
                    let _ = self.events.send(Notification::PromptRejected {
                        session_id: sid.clone(),
                        reason,
                        text,
                    });
                }
                let _ = reply.send(published);
            }
            Command::SpawnFailed {
                sid,
                expected_generation,
                reason,
            } => {
                self.handle_spawn_failed(&sid, expected_generation, reason);
            }
            Command::Record {
                sid,
                generation,
                event,
            } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                // ADDITIVE (spec §9): record the canonical AgentEvent ALONGSIDE
                // the legacy ReplyEvent. The legacy variant still drives the GUI
                // transcript this pass; the Agent stream is forwarded for the §9
                // agreement check. ReplayComplete / TurnEnded carry their
                // identity in the envelope, not the payload, so they map to the
                // ReplayEnd / (handled-below) boundary kinds rather than a Chunk.
                if let Some(kind) = yalda::agent_event::agent_kind_from_reply(&event) {
                    s.record_agent(kind);
                } else if matches!(event, yalda::acp_channel::ReplyEvent::ReplayComplete) {
                    s.record_agent(yalda::agent_event::replay_end_kind());
                }
                // NOTE: a worker `ReplyEvent::TurnEnded { count }` (only emitted
                // under YALDA_EMIT_TURN_ENDED=1) is intentionally NOT mapped
                // here — the authoritative live boundary is recorded by the
                // TurnCount handler below, where `self.turns` is already updated
                // so the envelope `turn` matches the settled count.
                s.record(Notification::ReplyEvent {
                    session_id: sid.clone(),
                    event,
                });
            }
            Command::TurnCount {
                sid,
                generation,
                turns,
            } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                // The pump's `turns` is already session-absolute (its
                // `turn_base` + the channel-local live count), and replay
                // never produces a TurnCount (the fence clears on the
                // end-of-replay marker via `ReplayDone`, not on a turn
                // number) — so every TurnCount that lands here is a real,
                // completed live turn.
                s.turns = turns;
                let channel_generation = s.channel_generation;
                // ADDITIVE (spec §9): record the canonical AgentEvent TurnEnded
                // ALONGSIDE the legacy TurnEnded. The envelope `turn` is the
                // COMPLETED turn index (0-based): a 1-based settled count of
                // `turns` means turn `turns - 1` just ended — this is exactly the
                // value the WAL recovery derives via `max(turn) + 1`. The outcome
                // is `Completed` here; richer outcomes (Cancelled / MaxTokens /
                // Failed) become available once the worker forwards the verbatim
                // ACP stopReason (a follow-up; the inference has no stopReason).
                let completed_turn = turns.saturating_sub(1) as u64;
                let agent_seq = s.agent_seq;
                s.agent_seq += 1;
                s.record(Notification::Agent {
                    event: yalda::agent_event::AgentEvent::new(
                        sid.clone(),
                        channel_generation,
                        completed_turn,
                        agent_seq,
                        yalda::agent_event::turn_ended_kind(
                            yalda::agent_event::TurnOutcome::Completed,
                        ),
                    ),
                });
                s.record(Notification::TurnEnded {
                    session_id: sid.clone(),
                    turn_count: turns,
                    generation: channel_generation,
                });
                // bug-0022: the turn is settled — the session is idle again.
                // Broadcast so every GUI's status mark drops out of "working",
                // including the ones not attached to this session.
                self.set_busy(&sid, false);
            }
            Command::ReplayDone { sid, generation } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                s.replay_fence = 0;
            }
            Command::AgentDisconnected { sid, generation } => {
                self.handle_spawn_failed(&sid, generation, "agent disconnected".into());
            }
        }
    }

    /// Terminalize a failed handshake or a dead live pump. Every deferred
    /// prompt receives an explicit rejection and the busy flag is cleared, so
    /// neither the server roster nor an attached GUI can remain "thinking"
    /// forever after there is no process capable of doing the work.
    fn handle_spawn_failed(&mut self, session_id: &str, expected_generation: u64, reason: String) {
        let was_busy = {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            if session.archived || session.channel_generation != expected_generation {
                return; // archived or stale worker/pump
            }

            let was_busy = session.busy;
            session.lifecycle = SessionLifecycle::Disconnected;
            session.channel = None;
            session.busy = false;
            session.replay_fence = 0;
            for pending in std::mem::take(&mut session.pending_prompts) {
                session
                    .finish_prompt_intent(pending.id, yalda::session_wal::PromptOutcome::Rejected);
                session.record(Notification::PromptRejected {
                    session_id: session_id.to_string(),
                    reason: reason.clone(),
                    text: pending.payload.text,
                });
            }
            session.record(Notification::SessionDetached {
                session_id: session_id.to_string(),
                reason,
            });
            was_busy
        };

        if was_busy {
            self.broadcast_busy(session_id, false);
        }
        // `SessionDetached` is per-session; connectivity is roster-wide.
        self.broadcast_connected(session_id, false);
    }

    fn do_create(
        &mut self,
        cwd: PathBuf,
        label: String,
        provider: AgentProvider,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let permission_mode = self.default_permission_mode;
        // Open the durable WAL up front so even a crash immediately after create
        // can recover the session's identity.
        let wal = open_session_wal(&id, &label, &cwd, permission_mode, provider);
        let session = new_managed_session(
            id.clone(),
            label,
            cwd.clone(),
            provider,
            permission_mode,
            wal,
            self.bridge_tx.clone(),
        );

        let info = session.info();
        self.sessions.insert(id.clone(), session);
        let _ = self.events.send(Notification::SessionCreated {
            session: info.clone(),
        });

        // Spawn the ACP agent on a background thread (blocking handshake), which
        // posts `PublishChannel` back into the actor when ready.
        let cmd_tx = self.cmd_tx.clone();
        let spawner = Arc::clone(&self.spawner);
        let session_id = id.clone();
        let spawn_result = std::thread::Builder::new()
            .name(format!("acp-spawn-{}", &id[..8]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; single-purpose server.
                unsafe {
                    std::env::set_var("YALDA_SESSION_MANAGED", "1");
                }
                let cmd = configured_agent_command(provider);
                let resumed = resume_session_id.is_some();
                match spawner.spawn(
                    provider,
                    &cmd,
                    Some(cwd),
                    resume_session_id,
                    YaldaFrontend::Gpui,
                ) {
                    Ok(client) => {
                        // Fresh spawn → is_respawn = false, generation stays 0.
                        // `resumed` (create-with-resume) arms nothing here —
                        // the session's turns are 0, so the fence stays down
                        // and the adopted history records into the empty log.
                        publish_channel(&cmd_tx, &session_id, client, 0, false, resumed);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid: session_id,
                            expected_generation: 0,
                            reason: format!("spawn failed: {e}"),
                        });
                    }
                }
            });
        if let Err(error) = spawn_result {
            self.handle_spawn_failed(
                &id,
                0,
                format!("could not start agent spawn worker: {error}"),
            );
        }

        info
    }

    fn do_close(&mut self, session_id: &str) -> Result<(), String> {
        // Delete durable identity BEFORE removing live state. If deletion
        // fails, closing anyway guarantees the session resurrects from its WAL
        // on the next daemon start — a much worse result than a visible close
        // error with the current session left intact.
        let delete_result = {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if let Some(wal) = session.wal.take() {
                let path = wal.path().to_path_buf();
                match wal.remove() {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        session.wal = yalda::session_wal::SessionWal::reopen(path).ok();
                        Err(error)
                    }
                }
            } else if let Some(path) = &session.wal_path {
                match std::fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            }
        };
        delete_result.map_err(|error| format!("could not delete session WAL: {error}"))?;

        // Only after durable deletion succeeds may the actor drop the live
        // transport and announce closure.
        let session = self.sessions.remove(session_id).expect("checked above");
        let _ = session
            .gen_watch
            .send_replace(session.channel_generation.wrapping_add(1));
        let _ = self.events.send(Notification::SessionClosed {
            session_id: session_id.to_string(),
        });
        Ok(())
    }

    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    fn do_attach(
        &mut self,
        session_id: &str,
        cursor: Option<(u64, u64)>,
    ) -> Result<(watch::Receiver<LogSnapshot>, u64, ForwarderProgress), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;

        let log_rx = session.log_tx.subscribe();

        // Resolve the reconnect cursor into the forwarder's initial `sent` (a
        // `Vec` index) under the §6 epoch predicate. The cursor's second field is
        // a LOGICAL `acked_seq` (not a raw `Vec` index) — `EventLog::resolve_cursor`
        // owns the single `seq ↔ Vec-offset` translation via `log_base` (spec §6,
        // risk #3). Incremental tail ONLY when the cursor's generation matches the
        // session's current `channel_generation` AND `log_base <= acked_seq <= tip`.
        // Falls back to a from-base rebuild (`Vec` index 0) for any of:
        //   - no cursor (every client today);
        //   - generation mismatch — a *force-restart* bumped the epoch, so the
        //     client's pre-restart cursor is stale;
        //   - `acked_seq < log_base` — the slow subscriber fell off the trimmed
        //     tail (Stage B compaction-past-cursor); a clean from-base rebuild
        //     (which begins with the `CompactedSummary` marker), NEVER a gap;
        //   - `acked_seq > tip` — bogus client / lost un-fsynced mid-turn tail.
        // Evaluated under the actor lock so `log_base` can't advance mid-decision.
        //
        // BACK-COMPAT: before any trim `log_base == 0`, so `acked_seq == Vec
        // index` and this is byte-identical to the phase-5 steady state — a
        // never-force-restarted (gen 0, idx) cursor tails exactly `[idx..]`.
        let initial_vec_index = session
            .event_log
            .resolve_cursor(cursor, session.channel_generation)
            .initial_vec_index();
        // Hand the forwarder a LOGICAL `sent_seq` (Bug 1a), not the raw `Vec`
        // index, so a later trim re-resolves it correctly: the entries up to
        // `initial_vec_index` are considered already-sent, so `sent_seq` is the
        // seq of the FIRST not-yet-sent entry == `log_base + initial_vec_index`.
        let initial_sent_seq = session.event_log.seq_of(initial_vec_index);

        // Register the latest forwarder's progress handle (Bug 1b): one clone
        // goes to the forwarder task (returned), one is retained on the session
        // so the trim floor sees it. Seed it at the initial `sent_seq`. Do not
        // release the prior handle here: a separate connection may legitimately
        // observe the same session, and killing it would also kill the healthy
        // owner when an observer attaches. Duplicate Attach on ONE connection
        // is cleaned up by that connection's `subscribed.insert` task swap.
        let progress: ForwarderProgress = Arc::new(ForwarderHandle::new(initial_sent_seq));
        session.forwarder = Some(Arc::clone(&progress));

        Ok((log_rx, initial_sent_seq, progress))
    }

    fn do_detach(&mut self, session_id: &str) -> Result<(), String> {
        // Detaching the single client tears down its forwarder (the connection
        // handler aborts the task); the session and agent keep running. Drop the
        // actor's forwarder handle so the trim is no longer floored by a
        // departing subscriber.
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.forwarder = None;
        }
        Ok(())
    }

    fn do_prompt(
        &mut self,
        session_id: &str,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<(), String> {
        self.enqueue_prompt(session_id, text, images)
    }

    fn do_steer(
        &mut self,
        session_id: &str,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.lifecycle == SessionLifecycle::Archived {
            return Err(format!(
                "session {session_id} is archived; unarchive it before steering"
            ));
        }
        if session.lifecycle == SessionLifecycle::Disconnected {
            return Err(format!(
                "session {session_id} is disconnected; restart it before steering"
            ));
        }

        let pending = session.begin_prompt_intent(PromptPayload {
            text: text.to_string(),
            images,
        })?;
        let prompt_id = pending.id;
        let was_busy = session.busy;
        session.busy = true;
        let result = match (session.lifecycle, session.channel.as_ref()) {
            (SessionLifecycle::Live, Some(channel)) => {
                match channel.steer_or_replace_payload(pending.payload) {
                    Ok(()) => {
                        session.finish_prompt_intent(
                            prompt_id,
                            yalda::session_wal::PromptOutcome::Delivered,
                        );
                        Ok(())
                    }
                    Err(error) => {
                        session.finish_prompt_intent(
                            prompt_id,
                            yalda::session_wal::PromptOutcome::Rejected,
                        );
                        Err(format!("steer failed: {error}"))
                    }
                }
            }
            // A channel that is still spawning/restarting has no active turn
            // to steer. Queue an ordinary prompt; publish drains it in order.
            (SessionLifecycle::Spawning | SessionLifecycle::Restarting, None) => {
                session.pending_prompts.push(pending);
                Ok(())
            }
            _ => {
                session
                    .finish_prompt_intent(prompt_id, yalda::session_wal::PromptOutcome::Rejected);
                Err(format!(
                    "session {session_id} has inconsistent lifecycle state; restart it"
                ))
            }
        };
        if result.is_ok() {
            session.log_only(Notification::UserPrompt {
                session_id: session_id.to_string(),
                text: text.to_string(),
            });
            session.record_agent(yalda::agent_event::AgentEventKind::UserMessage {
                text: text.to_string(),
            });
        }
        let failure = result
            .as_ref()
            .err()
            .cloned()
            .map(|reason| (session.channel_generation, reason));
        if let Some((generation, reason)) = failure {
            self.handle_spawn_failed(session_id, generation, reason);
        } else if !was_busy {
            self.broadcast_busy(session_id, true);
        }
        result
    }

    /// Log the user's prompt durably, then hand it to the live channel (or queue
    /// it if the agent is still spawning). Shared by the GUI [`do_prompt`] path
    /// and the headless [`Command::AdminPrompt`] path (ADR-0015) — under strict
    /// 1:1 there is no owner gate, so the two are identical.
    fn enqueue_prompt(
        &mut self,
        session_id: &str,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.lifecycle == SessionLifecycle::Archived {
            return Err(format!(
                "session {session_id} is archived; unarchive it before sending a prompt"
            ));
        }
        if session.lifecycle == SessionLifecycle::Disconnected {
            return Err(format!(
                "session {session_id} is disconnected; restart it before sending"
            ));
        }
        let pending = session.begin_prompt_intent(PromptPayload {
            text: text.to_string(),
            images,
        })?;
        let prompt_id = pending.id;
        // bug-0022: a prompt (sent OR queued for a still-spawning agent) means a
        // turn is now owed — the session is working from the user's point of
        // view, which is what the status marks report.
        let was_busy = session.busy;
        session.busy = true;
        let result = match (session.lifecycle, session.channel.as_ref()) {
            (SessionLifecycle::Live, Some(channel)) => {
                match channel.send_payload(pending.payload) {
                    Ok(()) => {
                        session.finish_prompt_intent(
                            prompt_id,
                            yalda::session_wal::PromptOutcome::Delivered,
                        );
                        Ok(())
                    }
                    Err(error) => {
                        session.finish_prompt_intent(
                            prompt_id,
                            yalda::session_wal::PromptOutcome::Rejected,
                        );
                        Err(format!("send failed: {error}"))
                    }
                }
            }
            (SessionLifecycle::Spawning | SessionLifecycle::Restarting, None) => {
                session.pending_prompts.push(pending);
                Ok(())
            }
            _ => {
                session
                    .finish_prompt_intent(prompt_id, yalda::session_wal::PromptOutcome::Rejected);
                Err(format!(
                    "session {session_id} has inconsistent lifecycle state; restart it"
                ))
            }
        };
        if result.is_ok() {
            // Publish the optimistic transcript fact only after the prompt was
            // either handed to a live worker or durably admitted to the spawn
            // queue. A rejected live send therefore cannot recover as a phantom
            // user turn.
            session.log_only(Notification::UserPrompt {
                session_id: session_id.to_string(),
                text: text.to_string(),
            });
            session.record_agent(yalda::agent_event::AgentEventKind::UserMessage {
                text: text.to_string(),
            });
        }
        let failure = result
            .as_ref()
            .err()
            .cloned()
            .map(|reason| (session.channel_generation, reason));
        if let Some((generation, reason)) = failure {
            self.handle_spawn_failed(session_id, generation, reason);
        } else if !was_busy {
            self.broadcast_busy(session_id, true);
        }
        result
    }

    /// Set a session's in-flight flag and broadcast the change to EVERY
    /// connection (bug-0022). No-op when the flag is already at `busy`, so the
    /// broadcast fires on transitions only.
    fn set_busy(&mut self, session_id: &str, busy: bool) {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        if session.busy == busy {
            return;
        }
        session.busy = busy;
        self.broadcast_busy(session_id, busy);
    }

    fn broadcast_busy(&self, session_id: &str, busy: bool) {
        let _ = self.events.send(Notification::SessionBusy {
            session_id: session_id.to_string(),
            busy,
        });
    }

    /// bug-0027: publish agent-subprocess liveness to every GUI. `SessionInfo`
    /// carries `connected`, but it is only ever delivered by `list_sessions`,
    /// which clients treat as a seed — so without this broadcast every
    /// connectivity transition (spawn completing, agent exiting, respawn) was
    /// invisible until an unrelated reseed. Same shape as `broadcast_busy`.
    fn broadcast_connected(&self, session_id: &str, connected: bool) {
        let _ = self.events.send(Notification::SessionConnected {
            session_id: session_id.to_string(),
            connected,
        });
    }

    fn do_cancel(&mut self, session_id: &str) -> Result<(), String> {
        let cleared_queued = {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if let Some(channel) = session.channel.as_ref() {
                channel.cancel();
            }
            if matches!(
                session.lifecycle,
                SessionLifecycle::Spawning
                    | SessionLifecycle::Restarting
                    | SessionLifecycle::Disconnected
            ) {
                let queued = std::mem::take(&mut session.pending_prompts);
                let had_work = session.busy || !queued.is_empty();
                session.busy = false;
                for pending in queued {
                    session.finish_prompt_intent(
                        pending.id,
                        yalda::session_wal::PromptOutcome::Cancelled,
                    );
                    session.record(Notification::PromptRejected {
                        session_id: session_id.to_string(),
                        reason: "cancelled before agent delivery".into(),
                        text: pending.payload.text,
                    });
                }
                had_work
            } else {
                false
            }
        };
        if cleared_queued {
            self.broadcast_busy(session_id, false);
        }
        Ok(())
    }

    fn do_restart(&mut self, session_id: &str) -> Result<(), String> {
        let (cwd, provider, resume_id, expected_generation, was_busy) = {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if session.archived {
                return Err(format!(
                    "session {session_id} is archived; unarchive it before restarting"
                ));
            }
            // A dead transport has no session id, but the durable identity from
            // its last successful attach is still the correct resume target.
            let resume = session
                .channel
                .as_ref()
                .and_then(|c| c.session_id())
                .or_else(|| session.acp_session_id.clone());
            let was_busy = session.busy;
            // Fence and drop the old transport BEFORE the blocking replacement
            // handshake begins. Keeping it live until PublishChannel allowed a
            // prompt to race onto the doomed generation during restart.
            session.channel_generation = session.channel_generation.wrapping_add(1);
            session.agent_seq = 0;
            session.replay_fence = 0;
            session.busy = false;
            session.lifecycle = SessionLifecycle::Restarting;
            let _ = session.gen_watch.send_replace(session.channel_generation);
            session.channel = None;
            (
                session.cwd.clone(),
                session.provider,
                resume,
                session.channel_generation,
                was_busy,
            )
        };

        if was_busy {
            self.broadcast_busy(session_id, false);
        }
        self.broadcast_connected(session_id, false);

        let cmd_tx = self.cmd_tx.clone();
        let spawner = Arc::clone(&self.spawner);
        let sid = session_id.to_string();
        let failure_sid = sid.clone();
        let spawn_result = std::thread::Builder::new()
            .name(format!("acp-restart-{}", &sid[..8.min(sid.len())]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; see do_create.
                unsafe {
                    std::env::set_var("YALDA_SESSION_MANAGED", "1");
                }
                let cmd = configured_agent_command(provider);
                let resumed = resume_id.is_some();
                match spawner.spawn(provider, &cmd, Some(cwd), resume_id, YaldaFrontend::Gpui) {
                    Ok(client) => {
                        // The generation was bumped synchronously before this
                        // worker began, so publish it without a second bump.
                        publish_channel(&cmd_tx, &sid, client, expected_generation, false, resumed);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid,
                            expected_generation,
                            reason: format!("restart failed: {e}"),
                        });
                    }
                }
            });
        if let Err(error) = spawn_result {
            let reason = format!("could not start restart worker: {error}");
            self.handle_spawn_failed(&failure_sid, expected_generation, reason.clone());
            return Err(reason);
        }
        Ok(())
    }

    fn do_rename(&mut self, session_id: &str, label: String) -> Result<(), String> {
        {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            session.label = label.clone();
            // Persist the rename to the WAL so it survives a server restart;
            // without this the session recovers under its creation-time name
            // (the header label), which is the "names keep getting forgotten"
            // bug. A WAL error is logged, never fatal — the live broadcast below
            // still updates connected GUIs.
            let result = if let Some(wal) = session.wal.as_mut() {
                wal.append_rename(&label)
            } else if let Some(path) = session.wal_path.clone() {
                yalda::session_wal::SessionWal::reopen(path)
                    .and_then(|mut wal| wal.append_rename(&label))
            } else {
                Ok(())
            };
            if let Err(e) = result {
                tracing::error!(
                    session_id = %&session_id[..8.min(session_id.len())],
                    error = %e,
                    "WAL rename append failed"
                );
            }
        }
        let _ = self.events.send(Notification::SessionRenamed {
            session_id: session_id.to_string(),
            label,
        });
        Ok(())
    }

    /// Move a session into or out of cold storage. Archive is a real resource
    /// boundary: persist the marker first, then fence/drop the pump, transport,
    /// forwarder, and WAL handle. Unarchive reopens the same WAL and lazily
    /// resumes the last ACP session id.
    fn do_set_archived(&mut self, session_id: &str, archived: bool) -> Result<(), String> {
        let mut resume: Option<(PathBuf, AgentProvider, Option<String>, u64)> = None;
        let busy_was_true;
        let archive_changed;
        {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if session.archived == archived {
                // An earlier unarchive may have persisted `false` but then
                // failed its worker/handshake. Repeating unarchive is a retry,
                // not a no-op, while the explicit lifecycle is Disconnected.
                if !archived && session.lifecycle == SessionLifecycle::Disconnected {
                    session.lifecycle = SessionLifecycle::Spawning;
                    busy_was_true = session.busy;
                    archive_changed = false;
                    resume = Some((
                        session.cwd.clone(),
                        session.provider,
                        session.acp_session_id.clone(),
                        session.channel_generation,
                    ));
                } else {
                    return Ok(());
                }
            } else {
                busy_was_true = session.busy;
                archive_changed = true;

                if archived {
                    let wal = session
                        .wal
                        .as_mut()
                        .ok_or_else(|| format!("session {session_id} has no open durable WAL"))?;
                    wal.append_archived(true)
                        .map_err(|e| format!("could not persist archive state: {e}"))?;

                    if let Some(channel) = session.channel.as_ref() {
                        channel.cancel();
                    }
                    session.archived = true;
                    session.lifecycle = SessionLifecycle::Archived;
                    session.busy = false;
                    for pending in std::mem::take(&mut session.pending_prompts) {
                        session.finish_prompt_intent(
                            pending.id,
                            yalda::session_wal::PromptOutcome::Cancelled,
                        );
                    }
                    session.replay_fence = 0;
                    session.channel_generation = session.channel_generation.wrapping_add(1);
                    session.agent_seq = 0;
                    let _ = session.gen_watch.send_replace(session.channel_generation);
                    session.channel = None;
                    if let Some(forwarder) = session.forwarder.take() {
                        // bug-0028: `released`, NOT `evicted`. `evicted` is the
                        // high-water kill flag, and its handler shuts down the
                        // per-CONNECTION write half — so archiving one session used
                        // to tear down the GUI's whole socket and force every other
                        // session to reconnect from base. Archiving one session must
                        // stop exactly one forwarder.
                        forwarder.released.store(true, Ordering::Release);
                    }
                    session.publish_snapshot();
                    drop(session.wal.take());
                } else {
                    let path = session
                        .wal_path
                        .clone()
                        .ok_or_else(|| format!("session {session_id} has no durable WAL path"))?;
                    let mut wal = yalda::session_wal::SessionWal::reopen(path)
                        .map_err(|e| format!("could not reopen archived WAL: {e}"))?;
                    wal.append_archived(false)
                        .map_err(|e| format!("could not persist unarchive state: {e}"))?;
                    session.wal = Some(wal);
                    session.archived = false;
                    session.lifecycle = SessionLifecycle::Spawning;
                    resume = Some((
                        session.cwd.clone(),
                        session.provider,
                        session.acp_session_id.clone(),
                        session.channel_generation,
                    ));
                }
            }
        }

        if busy_was_true && archived {
            self.broadcast_busy(session_id, false);
        }
        if archived {
            self.broadcast_connected(session_id, false);
        }
        if archive_changed {
            let _ = self.events.send(Notification::SessionArchived {
                session_id: session_id.to_string(),
                archived,
            });
        }

        if let Some((cwd, provider, resume_id, expected_generation)) = resume {
            if let Err(error) = self.spawn_channel(
                session_id.to_string(),
                cwd,
                provider,
                resume_id,
                expected_generation,
                false,
            ) {
                self.handle_spawn_failed(session_id, expected_generation, error.clone());
                return Err(error);
            }
        }
        Ok(())
    }

    fn spawn_channel(
        &self,
        sid: String,
        cwd: PathBuf,
        provider: AgentProvider,
        resume_id: Option<String>,
        expected_generation: u64,
        is_respawn: bool,
    ) -> Result<(), String> {
        let cmd_tx = self.cmd_tx.clone();
        let spawner = Arc::clone(&self.spawner);
        std::thread::Builder::new()
            .name(format!("acp-lifecycle-{}", &sid[..8.min(sid.len())]))
            .spawn(move || {
                unsafe {
                    std::env::set_var("YALDA_SESSION_MANAGED", "1");
                }
                let cmd = configured_agent_command(provider);
                let resumed = resume_id.is_some();
                match spawner.spawn(provider, &cmd, Some(cwd), resume_id, YaldaFrontend::Gpui) {
                    Ok(client) => {
                        publish_channel(
                            &cmd_tx,
                            &sid,
                            client,
                            expected_generation,
                            is_respawn,
                            resumed,
                        );
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid,
                            expected_generation,
                            reason: format!("lifecycle spawn failed: {e}"),
                        });
                    }
                }
            })
            .map(|_| ())
            .map_err(|e| format!("could not start lifecycle worker: {e}"))
    }

    fn do_set_permission_mode(
        &mut self,
        session_id: &str,
        mode: PermissionMode,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        let persist = if let Some(wal) = session.wal.as_mut() {
            wal.append_permission_mode(mode)
        } else if let Some(path) = session.wal_path.clone() {
            yalda::session_wal::SessionWal::reopen(path)
                .and_then(|mut wal| wal.append_permission_mode(mode))
        } else {
            Ok(())
        };
        persist.map_err(|error| format!("could not persist permission mode: {error}"))?;
        session.permission_mode = mode;
        if let Some(channel) = &session.channel {
            channel.set_permission_mode(mode);
        }
        Ok(())
    }

    fn do_set_model(&mut self, session_id: &str, model_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if session.archived {
            return Err(format!(
                "session {session_id} is archived; unarchive it before switching models"
            ));
        }
        let persist = if let Some(wal) = session.wal.as_mut() {
            wal.append_model(model_id)
        } else if let Some(path) = session.wal_path.clone() {
            yalda::session_wal::SessionWal::reopen(path)
                .and_then(|mut wal| wal.append_model(model_id))
        } else {
            Ok(())
        };
        persist.map_err(|error| format!("could not persist model selection: {error}"))?;
        session.model_id = Some(model_id.to_string());
        if let Some(channel) = &session.channel {
            channel.set_model(model_id);
        }
        // If the channel is spawning/restarting/disconnected, the desired
        // model is still accepted and apply_channel_state replays it when the
        // replacement transport publishes.
        Ok(())
    }

    fn do_admin_status(&self) -> AdminSnapshot {
        let infos = self
            .sessions
            .values()
            .map(|s| AdminSessionInfo {
                session_id: s.id.clone(),
                label: s.label.clone(),
                provider: s.provider,
                connected: s.channel.is_some(),
                turns: s.turns,
                event_log_len: s.event_log.len(),
                log_base: s.event_log.log_base(),
                subscriber_count: s.log_tx.receiver_count(),
                channel_generation: s.channel_generation,
                permission_mode: s.permission_mode,
                archived: s.archived,
                wal_open: s.wal.is_some(),
            })
            .collect();
        AdminSnapshot {
            session_count: self.sessions.len(),
            sessions: infos,
        }
    }
}

// ── Session pump task ──────────────────────────────────────────────

/// Publish a freshly-spawned `AcpChannelClient` as a session's live transport,
/// from a (blocking) spawn worker thread:
///
/// 1. Derive its [`TransportHandle`] (the Send surface the actor stores).
/// 2. Send `PublishChannel` into the actor inlet and BLOCK on the oneshot reply.
///    The actor installs the handle via `apply_channel_state` (drains queued
///    prompts, re-applies permission mode, bumps generation + `gen_watch` on
///    respawn) and replies with (committed generation, gen_watch subscription,
///    replay fence). The actor never holds the client.
/// 3. Spawn the OWNING pump thread with the client moved into it, stamped with
///    that generation and wired to the gen_watch + fence.
///
/// On `is_respawn`, the generation bump wakes any OLD pump (via `gen_watch`) so
/// it self-terminates and drops its own client off the actor task (Blocker A).
/// If the session was closed mid-spawn, the client is dropped here on the
/// worker's OWN thread (its blocking Drop never runs on the actor).
fn publish_channel(
    cmd_tx: &mpsc::UnboundedSender<Command>,
    session_id: &ServerSessionId,
    client: Box<dyn AgentTransport>,
    expected_generation: u64,
    is_respawn: bool,
    resumed: bool,
) {
    let handle = client.handle();
    let (reply, rx) = tokio::sync::oneshot::channel();
    if cmd_tx
        .send(Command::PublishChannel {
            sid: session_id.clone(),
            handle,
            expected_generation,
            is_respawn,
            resumed,
            reply,
        })
        .is_err()
    {
        drop(client); // actor gone — drop the client on this worker thread.
        return;
    }
    // Blocking recv on this OS worker thread — never on the actor task.
    match rx.blocking_recv() {
        Ok(Some((generation, gen_rx, replay_fence, turn_base))) => {
            spawn_pump_thread(
                cmd_tx.clone(),
                session_id.clone(),
                client,
                generation,
                gen_rx,
                replay_fence,
                turn_base,
            );
        }
        Ok(None) | Err(_) => {
            // Session closed while spawning (or actor gone) — drop the client
            // here, on this worker thread (its Drop joins the worker / kills the
            // child; must never run on the actor task).
            drop(client);
        }
    }
}

/// Background thread that OWNS an `AcpChannelClient`, drains its `ReplyEvent`s,
/// and forwards them to the actor inlet as generation-stamped `Command`s.
///
/// Runs on a dedicated OS thread (not a tokio task) because `AcpChannelClient`
/// contains a `std::sync::mpsc::Receiver` which isn't `Sync`. The pump is the
/// SOLE owner of the client: it drops it (running the blocking `Drop`) on its
/// OWN thread when it observes a generation bump (restart) or a closed inlet
/// (close) — never on the actor (Blocker A).
fn spawn_pump_thread(
    cmd_tx: mpsc::UnboundedSender<Command>,
    session_id: ServerSessionId,
    client: Box<dyn AgentTransport>,
    my_generation: u64,
    gen_rx: watch::Receiver<u64>,
    initial_replay_fence: usize,
    turn_base: usize,
) {
    std::thread::Builder::new()
        .name(format!("pump-{}", &session_id[..8.min(session_id.len())]))
        .spawn(move || {
            // Per-session generation watch: a restart (generation bump) wakes us
            // to self-terminate + drop the client off the actor task.
            let gen_rx = gen_rx;

            let mut last_turns: usize = 0;
            // Marker-based replay fence (see `yalda::replay_fence`). The
            // suppression decision stays pump-side (cycle granularity); the
            // actor only sees Records that should be logged, plus one
            // `ReplayDone` when the fence drops.
            let mut fence = yalda::replay_fence::ReplayFence::new(initial_replay_fence > 0);

            const PUMP_IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(16);

            loop {
                // A newer generation means a restart (or close) superseded us —
                // break and drop the client off the actor task.
                if *gen_rx.borrow() > my_generation {
                    break;
                }
                // Inlet closed (manager gone) — terminate.
                if cmd_tx.is_closed() {
                    break;
                }

                // Liveness.
                if !client.is_connected() {
                    let _ = cmd_tx.send(Command::AgentDisconnected {
                        sid: session_id.clone(),
                        generation: my_generation,
                    });
                    break;
                }

                // Drain events up to a budget. If we hit the budget and more
                // events are pending, defer turn-end detection to a later cycle.
                const PUMP_EVENT_BUDGET: usize = 64;
                let mut events = Vec::new();
                while events.len() < PUMP_EVENT_BUDGET {
                    match client.try_recv() {
                        Some(ev) => events.push(ev),
                        None => break,
                    }
                }
                let more_pending = events.len() == PUMP_EVENT_BUDGET
                    && match client.try_recv() {
                        Some(ev) => {
                            events.push(ev);
                            true
                        }
                        None => false,
                    };

                let current_turns = client.turn_count();
                let turn_ended = !more_pending && current_turns > last_turns;

                let mut tail_events: Vec<yalda::acp_channel::ReplyEvent> = if turn_ended {
                    std::iter::from_fn(|| client.try_recv()).collect()
                } else {
                    Vec::new()
                };

                // ── Replay fence: suppress the resume's duplicate history ──
                // A restored/resumed session replays prior turns that are
                // already in `event_log`. Drain them (so the channel doesn't
                // back up) but emit no Records until the worker's
                // end-of-replay marker (`ReplayComplete`), which orders
                // strictly after the replay burst and strictly before any
                // live event.
                //
                // The fence MUST key on the marker, not on `turn_count()`:
                // the channel's counter restarts at 0 every spawn and never
                // moves during replay (092c218 replaced the post-load bump
                // with the marker), so a turn-count fence never cleared —
                // every post-resume event, replayed AND live, was silently
                // discarded while the agent kept working invisibly (the
                // "resume hangs" bug).
                if fence.is_up() {
                    // Fold any turn-end tail into the batch first so a marker
                    // landing in the tail can't be dropped with it.
                    events.append(&mut tail_events);
                    use yalda::replay_fence::FenceAction;
                    match fence.on_batch(&events, turn_ended) {
                        None => unreachable!("fence.is_up() checked above"),
                        Some(FenceAction::ClearAtMarker { marker_index }) => {
                            // Replayed duplicates precede the marker; the
                            // marker itself is recorded (the actor maps it to
                            // the durable ReplayEnd the GUI finalizes on) and
                            // everything after it is live.
                            let _ = cmd_tx.send(Command::ReplayDone {
                                sid: session_id.clone(),
                                generation: my_generation,
                            });
                            tracing::info!(
                                session_id = %&session_id[..8.min(session_id.len())],
                                discarded = marker_index,
                                "replay fence cleared (end-of-replay marker)"
                            );
                            events.drain(..marker_index);
                        }
                        Some(FenceAction::ForceClear) => {
                            // A live turn completed with the fence still up:
                            // the marker was lost (it is emitted on every
                            // resume attempt, so this is a defensive valve,
                            // not an expected path). Unwedge and record what
                            // remains — the turn's earlier chunks were
                            // already discarded.
                            let _ = cmd_tx.send(Command::ReplayDone {
                                sid: session_id.clone(),
                                generation: my_generation,
                            });
                            tracing::warn!(
                                session_id = %&session_id[..8.min(session_id.len())],
                                "replay fence force-cleared by live turn end \
                                 (missing end-of-replay marker)"
                            );
                        }
                        Some(FenceAction::Discard) => {
                            let drained = !events.is_empty();
                            if !drained && !more_pending {
                                std::thread::sleep(PUMP_IDLE_SLEEP);
                            }
                            continue;
                        }
                    }
                }

                let drained_events = !events.is_empty();

                // Forward events first (in order).
                for ev in events {
                    if std::env::var("YALDA_CHUNKLOG").is_ok()
                        && let yalda::acp_channel::ReplyEvent::Chunk(t) = &ev
                    {
                        tracing::info!("[chunklog srv] {t:?}");
                    }
                    let _ = cmd_tx.send(Command::Record {
                        sid: session_id.clone(),
                        generation: my_generation,
                        event: ev,
                    });
                }

                if turn_ended {
                    // Tail events recorded after budget events, before TurnEnded.
                    for ev in tail_events {
                        let _ = cmd_tx.send(Command::Record {
                            sid: session_id.clone(),
                            generation: my_generation,
                            event: ev,
                        });
                    }
                    last_turns = current_turns;
                    // Report session-absolute turns: the channel's counter
                    // restarts at 0 on every spawn, so a resumed session's
                    // first live turn is `turn_base + 1` (continuing the
                    // durable numbering), never 1 — the actor's `s.turns`
                    // must not regress (envelope `turn` stamps and the WAL's
                    // `max(turn) + 1` recovery both depend on it).
                    let _ = cmd_tx.send(Command::TurnCount {
                        sid: session_id.clone(),
                        generation: my_generation,
                        turns: turn_base + current_turns,
                    });
                }

                if !drained_events && !more_pending && !turn_ended {
                    std::thread::sleep(PUMP_IDLE_SLEEP);
                }
            }

            // Drop the client on THIS thread (blocking Drop: kills child +
            // joins worker). Never runs on the actor task (Blocker A).
            drop(client);
        })
        .ok();
}

// ── Connection handler ─────────────────────────────────────────────

/// Handle a single GUI connection on the Unix socket. `conn_id` uniquely
/// identifies this connection so the session manager can track which
/// connection owns each session and gate driving operations accordingly.
async fn handle_connection(stream: UnixStream, manager: Arc<SessionManager>, conn_id: u64) {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    // Track which sessions this connection is subscribed to, so we can
    // clean up on disconnect.
    let mut subscribed: HashMap<ServerSessionId, tokio::task::JoinHandle<()>> = HashMap::new();

    // Manager-level forwarder: pushes session-list changes (create/close/
    // rename) to this GUI so its session list stays consistent with every
    // other connection. Independent of per-session attach state.
    let manager_events = {
        let mut rx = manager.subscribe_events();
        let w = Arc::clone(&writer);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(note) => {
                        let frame = Frame::Notification { note };
                        if let Ok(mut line) = serde_json::to_string(&frame) {
                            line.push('\n');
                            let mut w = w.lock().await;
                            // Same slow-subscriber reaping as the per-session
                            // forwarder: a peer that never drains this fd would
                            // otherwise park this task forever once its kernel
                            // send buffer fills under session-list churn.
                            match tokio::time::timeout(
                                slow_sub_write_timeout(),
                                w.write_all(line.as_bytes()),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => return, // client gone
                                Err(_) => {
                                    tracing::warn!(
                                        "slow subscriber: session-list write stalled \
                                         >{}ms — disconnecting",
                                        slow_sub_write_timeout().as_millis()
                                    );
                                    return;
                                }
                            }
                        }
                    }
                    // Lagged: a few list events were dropped under load. The
                    // GUI reconciles on next open/reconnect, so skip and
                    // continue rather than tearing down the forwarder.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    };

    let started = std::time::Instant::now();
    let mut lines = reader.lines();

    // Read loop. Captures WHY it exits so the teardown log can name the cause
    // of a disconnect — the reconnect-storm diagnostic. Distinguishes client
    // EOF vs socket read error vs a failed response write (client already gone).
    let exit_reason: String = loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break "client closed connection (EOF)".to_string(),
            Err(e) => break format!("socket read error: {e}"),
        };
        let frame: Frame = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(error = %e, "bad frame");
                continue;
            }
        };

        let Frame::Request { id, req } = frame else {
            continue;
        };

        let response = match req {
            Request::Ping => Response::Ok {
                data: ResponseData::Pong,
            },

            Request::ListSessions => {
                let sessions = manager.send_list_sessions().await;
                Response::Ok {
                    data: ResponseData::Sessions { sessions },
                }
            }

            Request::CreateSession {
                cwd,
                label,
                provider,
                resume_session_id,
            } => {
                let info = manager
                    .send_create(cwd, label, provider, resume_session_id)
                    .await;
                Response::Ok {
                    data: ResponseData::Session { session: info },
                }
            }

            Request::Attach { session_id, cursor } => {
                match manager.send_attach(&session_id, cursor).await {
                    Ok((log_rx, initial_sent_seq, progress)) => {
                        // `initial_sent_seq` is the actor-resolved tail start as a
                        // LOGICAL seq (Bug 1a): the seq of the first not-yet-sent
                        // entry. `log_base` for a from-replay attach (so the first
                        // tail streams from `Vec` index 0), or the cursor's resolved
                        // seq for an incremental reconnect. The forwarder translates
                        // it to a `Vec` offset against the CURRENT `log_base` on
                        // every wake, so a Stage-B trim can't make it slice a stale
                        // offset. `progress` is its shared trim-floor handle (Bug 1b).
                        tracing::info!(
                            session_id = %&session_id[..8],
                            initial_sent_seq,
                            cursor = ?cursor,
                            "attach: forwarder tail start resolved"
                        );
                        let w = Arc::clone(&writer);
                        let handle = tokio::spawn(forward_notifications(
                            session_id.clone(),
                            w,
                            log_rx,
                            initial_sent_seq,
                            progress,
                        ));
                        if let Some(previous) = subscribed.insert(session_id, handle) {
                            previous.abort();
                        }
                        Response::Ok {
                            data: ResponseData::Attached,
                        }
                    }
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Detach { session_id } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_detach(&session_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Prompt {
                session_id,
                text,
                images,
            } => {
                match manager.send_prompt(&session_id, &text, images).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => {
                        // The client's `prompt()` is fire-and-forget, so this
                        // `Response::Error` has no waiter — without more, a
                        // refused prompt is INVISIBLE while the GUI renders its
                        // optimistic echo. Surface the rejection on this
                        // connection's notification stream (transient, never
                        // recorded) so the GUI can tell the user and offer the
                        // text back. (Under strict 1:1 a prompt is no longer
                        // owner-gated, so this path now only fires on a genuine
                        // send failure / missing session, not on contention.)
                        tracing::warn!(
                            session_id = %&session_id[..8.min(session_id.len())],
                            reason = %e,
                            "prompt rejected — notifying submitter"
                        );
                        let frame = Frame::Notification {
                            note: Notification::PromptRejected {
                                session_id: session_id.clone(),
                                reason: e.clone(),
                                text,
                            },
                        };
                        if let Ok(mut line) = serde_json::to_string(&frame) {
                            line.push('\n');
                            let _ = writer.lock().await.write_all(line.as_bytes()).await;
                        }
                        Response::Error { message: e }
                    }
                }
            }

            Request::Steer {
                session_id,
                text,
                images,
            } => match manager.send_steer(&session_id, &text, images).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => {
                    tracing::warn!(
                        session_id = %&session_id[..8.min(session_id.len())],
                        reason = %e,
                        "steer rejected — notifying submitter"
                    );
                    let frame = Frame::Notification {
                        note: Notification::PromptRejected {
                            session_id: session_id.clone(),
                            reason: e.clone(),
                            text,
                        },
                    };
                    if let Ok(mut line) = serde_json::to_string(&frame) {
                        line.push('\n');
                        let _ = writer.lock().await.write_all(line.as_bytes()).await;
                    }
                    Response::Error { message: e }
                }
            },

            Request::AdminPrompt { session_id, text } => {
                match manager.send_admin_prompt(&session_id, &text).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Cancel { session_id } => match manager.send_cancel(&session_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::RestartSession { session_id } => {
                match manager.send_restart(&session_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetPermissionMode { session_id, mode } => {
                match manager.send_set_permission_mode(&session_id, mode).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetModel {
                session_id,
                model_id,
            } => match manager.send_set_model(&session_id, model_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::CloseSession { session_id } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_close(&session_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::RenameSession { session_id, label } => {
                match manager.send_rename(&session_id, label).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::SetArchived {
                session_id,
                archived,
            } => {
                if archived && let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_set_archived(&session_id, archived).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::AdminStatus => Response::Ok {
                data: ResponseData::AdminStatus {
                    snapshot: manager.send_admin_status().await,
                },
            },
        };

        let resp_frame = Frame::Response {
            id,
            result: response,
        };
        let mut line = serde_json::to_string(&resp_frame).unwrap();
        line.push('\n');
        let mut w = writer.lock().await;
        // Bound the reply write too: a client that issued a request but stopped
        // draining its socket must not park this read loop forever.
        match tokio::time::timeout(slow_sub_write_timeout(), w.write_all(line.as_bytes())).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break "response write failed (client gone)".to_string(),
            Err(_) => break "response write stalled (slow client)".to_string(),
        }
    };

    tracing::info!(
        conn_id,
        attached = subscribed.len(),
        "conn {conn_id} closed after {:.1}s — {exit_reason}; was attached to {} session(s)",
        started.elapsed().as_secs_f64(),
        subscribed.len(),
    );

    // Connection closed (socket EOF) — tear down this client's forwarder. The
    // session and its agent keep running with no client attached (strict 1:1);
    // a later Attach resumes from the durable `event_log`. `send_detach` drops
    // the actor's forwarder handle so the trim is no longer floored by a gone
    // subscriber.
    for (sid, handle) in &subscribed {
        handle.abort();
        let _ = manager.send_detach(sid).await;
    }
    manager_events.abort();
}

/// Forward a session's notifications to the single attached GUI connection's
/// writer (strict 1:1).
///
/// **Source of truth is `event_log`, not the broadcast.** The watch channel is
/// used only as a wake signal: on any wake we re-read `event_log[sent..]` and
/// forward whatever the client hasn't seen. This makes a slow/lagging
/// subscriber *self-healing* — it can never permanently lose transcript content.
/// The first tail pass (`sent == 0`) also subsumes the attach-time replay, so
/// history and live stream share one ordered path with no replay/live seam.
async fn forward_notifications(
    session_id: ServerSessionId,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    mut log_rx: watch::Receiver<LogSnapshot>,
    initial_sent_seq: u64,
    progress: ForwarderProgress,
) {
    use std::sync::atomic::Ordering;

    // The forwarder's position is a LOGICAL `sent_seq` (Bug 1a), NOT a raw `Vec`
    // index: the seq of the FIRST entry this client has NOT yet been sent.
    // Starts at the actor-resolved `initial_sent_seq` (`log_base` for a full
    // replay, or the reconnect cursor's resolved seq for an incremental reconnect).
    //
    // On every wake we translate `sent_seq` → a `Vec` offset against the CURRENT
    // published `log_base` (via `EventLog::resolve_sent`, the SAME translation the
    // attach resolver uses — not a duplicated copy). A Stage-B trim front-drains
    // the `Vec` and advances `log_base`, so the same `sent_seq` maps to a smaller
    // offset; if the trim passed this forwarder entirely (`sent_seq < log_base`),
    // `resolve_sent` returns `FromBase` and we re-slice from index 0 (which now
    // begins with the `CompactedSummary` marker) — never a stale-offset gap/dup.
    let mut sent_seq = initial_sent_seq;

    // Tail the given published snapshot from `sent_seq`: resolve the offset,
    // flush `[offset..]`, advance `sent_seq` to the snapshot tip, and publish the
    // new position to the shared progress handle so the trim floor sees it.
    // Returns `false` if the write failed/stalled (caller exits, dropping the
    // progress handle → it falls out of the trim-floor `min`).
    async fn tail_snapshot(
        snap: &LogSnapshot,
        writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
        session_id: &str,
        sent_seq: &mut u64,
        progress: &ForwarderProgress,
    ) -> bool {
        // High-water disconnect (spec §6): the actor set `evicted` because this
        // forwarder's backlog crossed the high-water bound. Shut down the write
        // half so the CLIENT sees a clean EOF and does a from-base reconnect
        // (NOT a silent gap) — merely returning would only stop this forwarder
        // task while the connection's read loop kept the socket open (a wedged
        // GUI under App Nap would never notice). The progress handle drops on
        // return (the actor already cleared `forwarder`, so the trim resumed).
        match forwarder_stop_action(progress) {
            Some(ForwarderStop::ShutdownConnection) => {
                tracing::warn!(
                    session_id = %&session_id[..8.min(session_id.len())],
                    "high-water disconnect: backlog past threshold — closing wedged forwarder's socket"
                );
                use tokio::io::AsyncWriteExt as _;
                let _ = writer.lock().await.shutdown().await;
                return false;
            }
            // bug-0028: this session was archived. Stop tailing it, but leave
            // the shared per-connection write half open — every OTHER session on
            // this connection is still streaming through it.
            Some(ForwarderStop::ThisSessionOnly) => {
                tracing::info!(
                    session_id = %&session_id[..8.min(session_id.len())],
                    "forwarder released (session archived); connection stays up"
                );
                return false;
            }
            None => {}
        }
        let offset = match snap.log.resolve_sent(*sent_seq, snap.generation) {
            yalda::event_log::CursorResolution::FromBase => 0,
            yalda::event_log::CursorResolution::Tail { vec_index } => vec_index,
        };
        let tail = snap.log.tail_from(offset);
        if !tail.is_empty() {
            if !flush_tail(writer, session_id, &tail).await {
                return false;
            }
            // Advance to the tip seq: everything resident is now sent.
            *sent_seq = snap.log.tip_seq();
            progress.sent_seq.store(*sent_seq, Ordering::Release);
        }
        true
    }

    // First pass: `watch::Sender::subscribe()` marks the current value as
    // already-seen, so the initial transcript replay IS the first tail. Mark
    // the current snapshot seen with `borrow_and_update()` and tail it from
    // `sent_seq` (the resolved cursor seq, or `log_base` for full replay) to
    // subsume attach replay (no separate replay path).
    {
        let snap = log_rx.borrow_and_update().clone();
        if !tail_snapshot(&snap, &writer, &session_id, &mut sent_seq, &progress).await {
            return;
        }
    }

    loop {
        // Transcript log channel: a new snapshot was published. Tail the latest
        // snapshot lock-free from the cloned snapshot — no manager lock in the
        // hot path. Coalesced wakes self-heal: we always re-resolve `sent_seq`
        // against the latest published `log_base`.
        match log_rx.changed().await {
            Ok(()) => {
                let snap = log_rx.borrow_and_update().clone();
                if !tail_snapshot(&snap, &writer, &session_id, &mut sent_seq, &progress).await {
                    return;
                }
            }
            Err(_) => {
                // Sender dropped (session closing). One final tail of the last
                // snapshot, then exit.
                let snap = log_rx.borrow().clone();
                let _ = tail_snapshot(&snap, &writer, &session_id, &mut sent_seq, &progress).await;
                return;
            }
        }
    }
}

/// Per-write timeout for forwarder socket writes. A subscriber whose socket
/// stops draining (dead/stuck peer) would otherwise make `write_all` block
/// indefinitely, parking the forwarder task + its fd forever. We bound every
/// forwarder write by this duration; on elapse we drop the subscriber (its
/// write half closes → the client sees EOF and cleanly reconnects, replaying
/// from the watch snapshot, so no events are lost).
///
/// Default is GENEROUS (60s) so a healthy slow-but-progressing client is never
/// falsely reaped. Override via `YALDA_SLOW_SUB_TIMEOUT_MS` (u64 ms); `0` or
/// unset → the 60s default.
fn slow_sub_write_timeout() -> std::time::Duration {
    // Resolved once per process (env can't change mid-run) so the hot
    // streaming write path doesn't lock + parse the env on every write.
    static TIMEOUT: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        const DEFAULT_MS: u64 = 60_000;
        let ms = std::env::var("YALDA_SLOW_SUB_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .unwrap_or(DEFAULT_MS);
        std::time::Duration::from_millis(ms)
    })
}

/// Serialize and write a tail slice of notifications in one buffered write.
/// Returns `false` if the write failed (client gone) or stalled past the
/// slow-subscriber timeout (non-draining peer), in which case the caller drops
/// the forwarder.
async fn flush_tail(
    writer: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    session_id: &str,
    tail: &[Notification],
) -> bool {
    let mut buf = String::new();
    for note in tail {
        let frame = Frame::Notification { note: note.clone() };
        if let Ok(line) = serde_json::to_string(&frame) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    let dur = slow_sub_write_timeout();
    let mut w = writer.lock().await;
    match tokio::time::timeout(dur, w.write_all(buf.as_bytes())).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            // Socket error — client gone.
            tracing::warn!(
                session_id = %&session_id[..8.min(session_id.len())],
                this_pass = tail.len(),
                "forwarder write failed — client gone"
            );
            false
        }
        Err(_) => {
            // Elapsed — the peer's socket buffer is full and not draining.
            tracing::warn!(
                session_id = %&session_id[..8.min(session_id.len())],
                this_pass = tail.len(),
                "slow subscriber: write stalled >{}ms — disconnecting",
                dur.as_millis()
            );
            false
        }
    }
}

// ── Main ─────────────────────────────────────────���─────────────────

/// launchd commonly starts GUI-adjacent processes with a 256-descriptor soft
/// limit. A durable session consumes a WAL fd plus several pipes/kqueues for its
/// ACP transport, so a perfectly healthy roster of a few dozen sessions can
/// otherwise make the next `Command::spawn` fail with EMFILE. Raise only the
/// soft limit, never beyond the inherited hard limit.
#[cfg(unix)]
fn raise_open_file_limit() -> io::Result<(libc::rlim_t, libc::rlim_t)> {
    const DEFAULT_TARGET: libc::rlim_t = 4096;
    let requested = std::env::var("YALDA_MAX_OPEN_FILES")
        .ok()
        .and_then(|value| value.parse::<libc::rlim_t>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TARGET);
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let old = limit.rlim_cur;
    let target = if limit.rlim_max == libc::RLIM_INFINITY {
        requested
    } else {
        requested.min(limit.rlim_max)
    };
    if target > limit.rlim_cur {
        limit.rlim_cur = target;
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((old, limit.rlim_cur))
}

#[tokio::main]
async fn main() -> io::Result<()> {
    // Structured logging FIRST, before any other work. Route to STDERR (the
    // launchd/test harness captures the server's stderr to a log file and greps
    // it), with ANSI colors off so the log file stays clean grep-able text.
    // Defaults to "info" when RUST_LOG is unset.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(unix)]
    match raise_open_file_limit() {
        Ok((old, new)) if new > old => {
            tracing::info!(old, new, "raised open-file soft limit");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "could not raise open-file soft limit");
        }
    }

    // Reap ACP adapters orphaned by a previously crashed/killed yalda (parent
    // reparented to PID 1) before doing anything else — graceful exits already
    // reap via kill_on_drop; this catches the SIGKILL/panic path.
    let reaped = yalda::acp_channel::reap_orphaned_adapters();
    if reaped > 0 {
        tracing::info!("reaped {reaped} orphaned ACP adapter process(es) at startup");
    }

    // Relocate any state written by older builds under <cache_dir>/yalda into
    // the durable `~/.yalda` home (ADR-0018), BEFORE the WAL dir is read below.
    // One-time, idempotent, best-effort.
    yalda::paths::migrate_legacy_cache_dir();

    use clap::Parser;
    // Subcommands manage launchd supervision and exit; no subcommand = run the
    // server (the default path the GUI auto-launches).
    if let Some(command) = Cli::parse().command {
        return match command {
            Subcmd::Install => launchd::install(),
            Subcmd::Uninstall => launchd::uninstall(),
            Subcmd::Status => launchd::status(),
            Subcmd::Prompt { session_id, text } => {
                // Headless start-work (ADR-0015). Connect to an ALREADY-RUNNING
                // server (never auto-launch a throwaway daemon — a CLI prompt
                // targets a session in a live server), then enqueue via the
                // ungated admin path. Print ok/error and exit.
                let client = match yalda::session_client::SessionServerClient::connect_existing() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "error: could not connect to a running session server ({e}). \
                             Start one with `yalda-session-server` (or `yalda-session-server install`)."
                        );
                        std::process::exit(1);
                    }
                };
                match client.admin_prompt(&session_id, &text) {
                    Ok(()) => {
                        println!("ok");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        };
    }

    let socket_path = socket_path();
    let pid_path = pid_file_path();

    // Single-instance guard. If a server is ALREADY listening on this socket,
    // exit cleanly instead of removing the socket and re-binding — which would
    // silently steal it from the live server and orphan every session that
    // server is running. The client auto-launches a server on any failed
    // connect, so spurious concurrent launches genuinely happen; this makes
    // them harmless (the loser exits, the client's connect-retry finds the
    // winner). A socket file that exists but is NOT connectable is stale (a
    // prior crash left it behind), so we clear it and take over.
    if socket_path.exists() {
        if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
            tracing::warn!(
                "another server already listening on {} — exiting",
                socket_path.display()
            );
            return Ok(());
        }
        let _ = std::fs::remove_file(&socket_path);
    }

    // Owner-only socket: nobody else on the box can connect to (and drive)
    // our agent sessions. The mode must be tight from the instant the inode
    // exists — `bind()` starts queueing `connect()`s immediately, so a
    // chmod-after-bind leaves a TOCTOU window where a same-host attacker can
    // slip into the backlog. Clamp the umask around the bind so the socket is
    // created 0600 atomically; the explicit set_permissions is a belt-and-
    // suspenders assertion (and covers any platform that ignores umask on
    // socket inodes). We are single-threaded here (pre-accept-loop), so the
    // process-global umask flip is safe to restore right after.
    let prev_umask = unsafe { libc::umask(0o177) };
    let bind_result = UnixListener::bind(&socket_path);
    unsafe { libc::umask(prev_umask) };
    let listener = bind_result?;
    let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));

    // Write PID file.
    let _ = std::fs::write(&pid_path, std::process::id().to_string());

    tracing::info!("listening on {}", socket_path.display());

    // Load the config once at startup to pick up the user's default permission
    // mode. Config::load() is a plain lib fn (no GUI deps) and returns the
    // Default config when no file is present, so this is safe in the headless
    // server. Any parse error degrades to the hard-coded default rather than
    // refusing to start.
    let config = yalda::config::Config::load().unwrap_or_default();
    let default_permission_mode = config.default_permission_mode;
    tracing::info!(
        default_permission_mode = config.default_permission_mode.short_label(),
        "loaded config"
    );

    let (mgr, cmd_rx, default_permission_mode) =
        SessionManager::new_with_inlet(default_permission_mode);
    let manager = Arc::new(mgr);

    // Outbound bridge tap (T-004): one channel carries every session's logged
    // notifications to the bridge. The tx is threaded into the Manager (and each
    // recovered session) so `push_event` can forward; the rx is handed to
    // `maybe_spawn_bridge`.
    //
    // Gate the per-session sender on whether the bridge is actually enabled: when
    // it's disabled (the common case) sessions get `None`, so `push_event` skips
    // the clone+send entirely rather than cloning every streamed notification into
    // a dead channel on the hot path.
    let bridge_enabled = matches!(bridge::BridgeConfig::load(), Ok(Some(_)));
    let (bridge_evt_tx, bridge_evt_rx) =
        tokio::sync::mpsc::unbounded_channel::<(ServerSessionId, Notification)>();
    let session_bridge_tx = bridge_enabled.then(|| bridge_evt_tx.clone());

    // Recover sessions from a prior run BEFORE the actor starts (recovery must
    // precede the accept loop). The seed map is moved into the actor; the resume
    // jobs spawn workers that re-spawn ACP subprocesses and post `PublishChannel`
    // back into the actor once it's running.
    let (seed_sessions, resume_jobs) = restore_seed_from_disk(session_bridge_tx.clone());

    // Spawn the single-writer manager actor: it OWNS the sessions map and drains
    // the inlet (external requests, spawn-worker publishes, pump-sourced records)
    // one command at a time.
    // The shipping binary always uses the real subprocess-spawning transport.
    // The seam (Arc<dyn AgentSpawner>) exists so headless tests can substitute an
    // in-process fake; production behaviour is unchanged.
    let spawner: Arc<dyn AgentSpawner> = Arc::new(RealAgentSpawner);

    tokio::spawn(run_manager(
        cmd_rx,
        seed_sessions,
        manager.events.clone(),
        default_permission_mode,
        manager.cmd_tx.clone(),
        Arc::clone(&spawner),
        session_bridge_tx,
    ));

    // Now the actor is running, kick off the resume workers.
    for job in resume_jobs {
        spawn_resume_worker(manager.cmd_tx.clone(), job, Arc::clone(&spawner));
    }

    // Start the external chat bridge (Telegram) iff configured. No-op when
    // unconfigured (the common case); spec-external-chat-bridge.md.
    bridge::maybe_spawn_bridge(Arc::clone(&manager), bridge_evt_rx);

    // Handle graceful shutdown — persist sessions before exiting.
    // Listen for both SIGINT (Ctrl-C) and SIGTERM (kill / process manager).
    let mgr_shutdown = Arc::clone(&manager);
    let socket_path_cleanup = socket_path.clone();
    let pid_path_cleanup = pid_path.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to register SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            ctrl_c.await.ok();
        }
        // No explicit persist needed: each session's durable WAL is written
        // continuously (ADR-0009), so sessions already survive this shutdown
        // (and a crash). Just clean up the socket + pid so the next start is
        // tidy; the WAL dir is intentionally left for recovery.
        let _ = &mgr_shutdown;
        tracing::info!("shutting down (WALs are durable)");
        let _ = std::fs::remove_file(&socket_path_cleanup);
        let _ = std::fs::remove_file(&pid_path_cleanup);
        std::process::exit(0);
    });

    // Monotonic connection id — identifies which connection owns a session.
    let next_conn_id = std::sync::atomic::AtomicU64::new(1);

    loop {
        let (stream, _) = listener.accept().await?;
        let mgr = Arc::clone(&manager);
        let conn_id = next_conn_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Every GUI relaunch is a fresh accept (no persistent client identity),
        // so a "reconnect" surfaces here as conn_id > 1 and/or pre-existing
        // sessions — the session count is what tells you the client rejoined
        // live state rather than starting cold.
        let stored_sessions = manager.send_session_count().await;
        if conn_id == 1 {
            tracing::info!(
                conn_id,
                stored_sessions,
                "client connected (conn {conn_id}); {stored_sessions} stored session(s)"
            );
        } else {
            tracing::info!(
                conn_id,
                stored_sessions,
                "client reconnected (conn {conn_id}); {stored_sessions} stored session(s)"
            );
        }
        tokio::spawn(handle_connection(stream, mgr, conn_id));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    /// Regression (bug-0053): a server restart is a new channel generation.
    /// The durable WAL may already contain generation 1 from an earlier hard
    /// session restart; restarting the whole server must therefore seed the
    /// resumed channel at generation 2 with a fresh per-generation sequence.
    /// Otherwise the GUI replays generation 1, then rejects every live
    /// generation-0 response as stale while the backend completes it normally.
    #[test]
    fn recovered_wal_resumes_strictly_after_its_highest_agent_generation() {
        use yalda::agent_event::{AgentEvent, AgentEventKind};

        let dir = tempfile::tempdir().expect("WAL tempdir");
        let mut wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "recovered-generation",
            "recovered generation",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let path = wal.path().to_path_buf();
        wal.append(
            &Notification::SessionAttached {
                session_id: "recovered-generation".into(),
                acp_session_id: Some("acp-recovered-generation".into()),
            },
            false,
        )
        .expect("append resume identity");
        wal.append(
            &Notification::Agent {
                event: AgentEvent::new(
                    "recovered-generation".into(),
                    1,
                    7,
                    41,
                    AgentEventKind::ChannelOpened { resumed: true },
                ),
            },
            false,
        )
        .expect("append prior generation");
        drop(wal);

        let recovered = yalda::session_wal::recover_one(&path)
            .expect("read WAL")
            .expect("recover session");
        assert_eq!(
            recovered_stream_position(&recovered.event_log),
            (2, 0),
            "the first post-restart channel must be newer than durable history, \
             and its per-generation sequence must restart at zero"
        );
        assert_eq!(
            recovered_stream_position(&[]),
            (0, 0),
            "a legacy/empty WAL keeps brand-new generation-zero behavior"
        );
        let fresh = new_managed_session(
            "fresh-generation".into(),
            "fresh generation".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        assert_eq!(
            (fresh.channel_generation, fresh.agent_seq),
            (0, 0),
            "brand-new sessions still start at generation zero, sequence zero"
        );
    }

    /// Recovery must apply the same cap to active and archived WAL images
    /// before either can be attached.  Archived sessions never produce the
    /// later live append that used to trigger trimming, so covering both states
    /// pins the production failure rather than only its active-session face.
    #[test]
    fn recovery_compacts_oversized_active_and_archived_logs_before_attach() {
        use yalda::agent_event::{AgentEvent, AgentEventKind, ChunkRole};

        for archived in [false, true] {
            let sid = if archived { "archived" } else { "active" };
            let mut entries = Vec::new();
            for seq in 0..20 {
                entries.push(Notification::Agent {
                    event: AgentEvent::new(
                        sid.into(),
                        0,
                        seq / 2,
                        seq,
                        AgentEventKind::Chunk {
                            text: format!("chunk-{seq}"),
                            role: ChunkRole::Message,
                        },
                    ),
                });
            }
            let (log, dropped) = event_log_from_recovery(entries, sid, 0, 8);

            assert_eq!(dropped, 14, "20 entries compact to the ¾-cap target");
            assert_eq!(log.len(), 7, "six survivors plus one honest marker");
            assert!(log.len() <= 8, "recovered log is bounded before attach");
            assert_eq!(log.tip_seq(), 20, "compaction keeps the logical tip stable");
            match &log.tail_from(0)[0] {
                Notification::Agent { event } => match &event.kind {
                    AgentEventKind::CompactedSummary { summary, .. } => {
                        assert!(summary.contains("14 earlier event(s) trimmed"));
                        assert_eq!(event.seq, log.log_base());
                    }
                    other => panic!("first recovered entry must be summary, got {other:?}"),
                },
                other => panic!("first recovered entry must be Agent marker, got {other:?}"),
            }
        }
    }

    /// Regression (bug-0036 recurrence): two Codex questions submitted while a
    /// turn is active must remain FIFO native-steering requests. A later,
    /// explicit Stop is one independent cancel; it must not turn either
    /// question into an ordinary prompt or consume an extra cancel signal.
    #[cfg(feature = "test-support")]
    #[test]
    fn successive_codex_questions_then_stop_stay_fifo_through_server_actor() {
        let (transport, mut controls) =
            yalda::acp_channel::FakeTransport::with_session_id_and_steering(
                "fake-codex-steer",
                true,
            );
        let mut session = new_managed_session(
            "codex-steer".into(),
            "codex steering".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.channel = Some(transport.handle());
        session.lifecycle = SessionLifecycle::Live;
        session.busy = true;

        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("codex-steer".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(RealAgentSpawner),
            bridge_tx: None,
        };

        manager
            .do_steer("codex-steer", "first question", Vec::new())
            .expect("first steer");
        manager
            .do_steer("codex-steer", "second question", Vec::new())
            .expect("second steer");
        manager.do_cancel("codex-steer").expect("explicit stop");

        assert_eq!(
            controls
                .try_recv_native_steer()
                .expect("first native steer")
                .text,
            "first question"
        );
        assert_eq!(
            controls
                .try_recv_native_steer()
                .expect("second native steer")
                .text,
            "second question"
        );
        assert!(
            controls.prompt_rx.try_recv().is_err(),
            "capable Codex path must not fall back to ordinary prompts"
        );
        assert!(
            controls.try_recv_native_cancel(),
            "explicit Stop must follow both questions on the ordered control stream"
        );
        assert!(
            !controls.try_recv_native_cancel(),
            "ordered control stream must contain exactly one explicit Stop"
        );
        assert!(
            controls.cancel_rx.try_recv().is_err(),
            "capable path must not emit legacy compatibility cancels"
        );
    }

    /// Older Codex adapters advertise no native steering. The server actor must
    /// preserve the compatibility contract: one graceful cancel followed by
    /// the replacement prompt, with nothing on the native-control stream.
    #[cfg(feature = "test-support")]
    #[test]
    fn legacy_codex_steer_uses_cancel_then_prompt_through_server_actor() {
        let (transport, mut controls) = yalda::acp_channel::FakeTransport::new();
        let mut session = new_managed_session(
            "legacy-codex".into(),
            "legacy codex".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.channel = Some(transport.handle());
        session.lifecycle = SessionLifecycle::Live;
        session.busy = true;

        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("legacy-codex".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(RealAgentSpawner),
            bridge_tx: None,
        };

        manager
            .do_steer("legacy-codex", "replace the turn", Vec::new())
            .expect("legacy steer fallback");

        controls
            .cancel_rx
            .try_recv()
            .expect("legacy fallback emits one graceful cancel");
        assert!(
            controls.cancel_rx.try_recv().is_err(),
            "legacy fallback emits exactly one cancel"
        );
        assert_eq!(
            controls
                .prompt_rx
                .try_recv()
                .expect("legacy replacement prompt")
                .text,
            "replace the turn"
        );
        assert!(controls.prompt_rx.try_recv().is_err());
        assert!(
            controls.try_recv_native_steer().is_none(),
            "legacy adapter must receive no native control"
        );
    }

    /// REGRESSION (bug-0028): archiving ONE session must not disconnect the
    /// GUI. The per-connection write half is shared by every session forwarder
    /// on that socket, and `evicted`'s handler shuts it down — so reusing
    /// `evicted` to release an archived session's forwarder tore down the whole
    /// connection and forced every other session to reconnect from base. The
    /// user saw this as an unarchived session "having trouble starting up".
    ///
    /// Negative control: set `evicted` instead of `released` in
    /// `do_set_archived`. The stop action becomes `ShutdownConnection` and this
    /// fails on the "must not kill the shared connection" assertion.
    #[test]
    fn archiving_one_session_stops_its_forwarder_without_killing_the_connection() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "cold-2",
            "cold session",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let mut session = new_managed_session(
            "cold-2".into(),
            "cold session".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        session.acp_session_id = Some("acp-cold-2".into());
        // An attached GUI: this session has a live forwarder sharing the
        // connection's write half with every other attached session.
        let forwarder: ForwarderProgress = Arc::new(ForwarderHandle::new(0));
        session.forwarder = Some(Arc::clone(&forwarder));

        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("cold-2".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(RealAgentSpawner),
            bridge_tx: None,
        };

        manager
            .do_set_archived("cold-2", true)
            .expect("archive transition");

        // The forwarder must stop — an archived session streams nothing.
        assert_eq!(
            forwarder_stop_action(&forwarder),
            Some(ForwarderStop::ThisSessionOnly),
            "archiving must stop this session's forwarder"
        );
        // ...but it must NOT take the shared connection with it.
        assert!(
            !forwarder.evicted.load(std::sync::atomic::Ordering::Acquire),
            "archive must not set the high-water kill flag — its handler shuts \
             down the per-connection write half, disconnecting every OTHER \
             session on this socket"
        );
        assert_ne!(
            forwarder_stop_action(&forwarder),
            Some(ForwarderStop::ShutdownConnection),
            "archiving one session must never resolve to a connection teardown"
        );

        // The high-water wedge still escalates — this fix must not disarm it.
        let wedged: ForwarderProgress = Arc::new(ForwarderHandle::new(0));
        wedged
            .evicted
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            forwarder_stop_action(&wedged),
            Some(ForwarderStop::ShutdownConnection),
            "a real high-water eviction must still close the socket"
        );
        // A handle with no flag keeps streaming.
        let live: ForwarderProgress = Arc::new(ForwarderHandle::new(0));
        assert_eq!(forwarder_stop_action(&live), None);
    }

    #[test]
    fn archive_releases_runtime_state_and_wal_but_keeps_durable_session() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "cold-1",
            "cold session",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let wal_path = wal.path().to_path_buf();
        let mut session = new_managed_session(
            "cold-1".into(),
            "cold session".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        session.acp_session_id = Some("acp-cold-1".into());
        session.record(Notification::SessionAttached {
            session_id: "cold-1".into(),
            acp_session_id: session.acp_session_id.clone(),
        });
        session.busy = true;
        let pending = session
            .begin_prompt_intent(PromptPayload::text("queued"))
            .unwrap();
        session.pending_prompts.push(pending);

        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("cold-1".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(RealAgentSpawner),
            bridge_tx: None,
        };

        manager
            .do_set_archived("cold-1", true)
            .expect("archive transition");

        let session = manager.sessions.get("cold-1").unwrap();
        assert!(session.archived);
        assert!(
            session.channel.is_none(),
            "transport handle must be released"
        );
        assert!(session.forwarder.is_none(), "forwarder must be released");
        assert!(
            session.wal.is_none(),
            "archive must close the WAL descriptor"
        );
        assert!(!session.busy);
        assert!(session.pending_prompts.is_empty());
        assert_eq!(session.acp_session_id.as_deref(), Some("acp-cold-1"));
        assert!(wal_path.exists(), "archive retains the durable transcript");

        let admin = manager.do_admin_status();
        assert!(admin.sessions[0].archived);
        assert!(!admin.sessions[0].wal_open);
        assert!(
            manager
                .enqueue_prompt("cold-1", "must fail", Vec::new())
                .unwrap_err()
                .contains("archived")
        );

        let recovered = yalda::session_wal::recover_one(&wal_path)
            .expect("recover WAL")
            .expect("recovered session");
        assert!(recovered.archived);
        assert_eq!(recovered.acp_session_id.as_deref(), Some("acp-cold-1"));
    }

    fn manager_with_session(session: ManagedSession) -> Manager {
        let sid = session.id.clone();
        let (events, _) = broadcast::channel(32);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        Manager {
            sessions: HashMap::from([(sid, session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(RealAgentSpawner),
            bridge_tx: None,
        }
    }

    #[test]
    fn disconnected_session_rejects_prompt_instead_of_queueing_forever() {
        let mut session = new_managed_session(
            "dead-prompt".into(),
            "dead prompt".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.lifecycle = SessionLifecycle::Disconnected;
        let mut manager = manager_with_session(session);

        let error = manager
            .enqueue_prompt("dead-prompt", "never queue me", Vec::new())
            .expect_err("terminal disconnect must reject prompts");
        assert!(error.contains("disconnected"), "unexpected error: {error}");
        let session = manager.sessions.get("dead-prompt").unwrap();
        assert!(session.pending_prompts.is_empty());
        assert!(!session.busy);
    }

    #[test]
    fn spawn_failure_terminalizes_busy_and_rejects_every_queued_prompt() {
        let mut session = new_managed_session(
            "spawn-failure".into(),
            "spawn failure".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.busy = true;
        let pending = session
            .begin_prompt_intent(PromptPayload::text("queued during spawn"))
            .unwrap();
        session.pending_prompts.push(pending);
        let mut manager = manager_with_session(session);

        manager.apply(Command::SpawnFailed {
            sid: "spawn-failure".into(),
            expected_generation: 0,
            reason: "handshake failed".into(),
        });

        let session = manager.sessions.get("spawn-failure").unwrap();
        assert_eq!(session.lifecycle, SessionLifecycle::Disconnected);
        assert!(!session.busy);
        assert!(session.pending_prompts.is_empty());
        assert!(session.event_log.tail_from(0).iter().any(|note| matches!(
            note,
            Notification::PromptRejected { text, .. } if text == "queued during spawn"
        )));
    }

    #[test]
    fn cancel_while_spawning_removes_work_that_could_run_later() {
        let mut session = new_managed_session(
            "cancel-spawn".into(),
            "cancel spawn".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.busy = true;
        let pending = session
            .begin_prompt_intent(PromptPayload::text("cancel this"))
            .unwrap();
        session.pending_prompts.push(pending);
        let mut manager = manager_with_session(session);

        manager.do_cancel("cancel-spawn").expect("cancel");

        let session = manager.sessions.get("cancel-spawn").unwrap();
        assert!(session.pending_prompts.is_empty());
        assert!(!session.busy);
    }

    #[test]
    fn observer_attach_does_not_release_prior_connection_forwarder() {
        let session = new_managed_session(
            "attach-replace".into(),
            "attach replace".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        let mut manager = manager_with_session(session);

        let (_, _, first) = manager.do_attach("attach-replace", None).unwrap();
        let (_, _, second) = manager.do_attach("attach-replace", None).unwrap();

        assert!(
            !first.released.load(Ordering::Acquire),
            "the actor cannot assume an attach came from the same connection"
        );
        assert!(!second.released.load(Ordering::Acquire));
    }

    struct CaptureFailSpawner {
        resume_tx: std::sync::mpsc::Sender<Option<String>>,
    }

    impl AgentSpawner for CaptureFailSpawner {
        fn spawn(
            &self,
            _provider: AgentProvider,
            _command: &str,
            _cwd: Option<PathBuf>,
            resume: Option<String>,
            _frontend: YaldaFrontend,
        ) -> io::Result<Box<dyn AgentTransport>> {
            let _ = self.resume_tx.send(resume);
            Err(io::Error::other("injected handshake failure"))
        }
    }

    #[test]
    fn restart_fences_old_generation_and_uses_saved_resume_identity() {
        let mut session = new_managed_session(
            "restart-fence".into(),
            "restart fence".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.lifecycle = SessionLifecycle::Disconnected;
        session.channel_generation = 7;
        session.busy = true;
        session.acp_session_id = Some("durable-acp-id".into());

        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("restart-fence".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(CaptureFailSpawner { resume_tx }),
            bridge_tx: None,
        };

        manager.do_restart("restart-fence").expect("start restart");

        let session = manager.sessions.get("restart-fence").unwrap();
        assert_eq!(session.lifecycle, SessionLifecycle::Restarting);
        assert_eq!(session.channel_generation, 8);
        assert!(session.channel.is_none());
        assert!(!session.busy);
        assert_eq!(
            resume_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("restart worker called spawner"),
            Some("durable-acp-id".into()),
            "restart must fall back to the saved ACP id after a dead channel"
        );
    }

    #[test]
    fn repeated_unarchive_retries_a_disconnected_handshake() {
        let mut session = new_managed_session(
            "retry-unarchive".into(),
            "retry unarchive".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        // This is the state left after the durable archived=false transition
        // succeeded but the asynchronous handshake failed.
        session.archived = false;
        session.lifecycle = SessionLifecycle::Disconnected;
        session.acp_session_id = Some("retry-acp-id".into());

        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (events, _) = broadcast::channel(16);
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut manager = Manager {
            sessions: HashMap::from([("retry-unarchive".into(), session)]),
            events,
            default_permission_mode: PermissionMode::ReadOnly,
            cmd_tx,
            spawner: Arc::new(CaptureFailSpawner { resume_tx }),
            bridge_tx: None,
        };

        manager
            .do_set_archived("retry-unarchive", false)
            .expect("retry worker starts");

        assert_eq!(
            manager.sessions["retry-unarchive"].lifecycle,
            SessionLifecycle::Spawning
        );
        assert_eq!(
            resume_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("retry called spawner"),
            Some("retry-acp-id".into())
        );
    }

    #[test]
    fn permission_and_model_selection_survive_server_recovery() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "durable-settings",
            "durable settings",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let path = wal.path().to_path_buf();
        let session = new_managed_session(
            "durable-settings".into(),
            "durable settings".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        let mut manager = manager_with_session(session);

        manager
            .do_set_permission_mode("durable-settings", PermissionMode::Yolo)
            .expect("persist permission mode");
        manager
            .do_set_model("durable-settings", "gpt-durable")
            .expect("persist model");
        drop(manager);

        let recovered = yalda::session_wal::recover_one(&path)
            .expect("read WAL")
            .expect("recover session");
        assert_eq!(recovered.permission_mode, PermissionMode::Yolo);
        assert_eq!(recovered.model_id.as_deref(), Some("gpt-durable"));
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn replacement_channel_reapplies_durable_permission_and_model() {
        let (transport, controls) = yalda::acp_channel::FakeTransport::new();
        let mut session = new_managed_session(
            "reapply-settings".into(),
            "reapply settings".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::Yolo,
            None,
            None,
        );
        session.model_id = Some("gpt-durable".into());

        let undelivered = session.apply_channel_state(transport.handle(), true, true);

        assert!(undelivered.is_empty());
        assert_eq!(controls.permission_mode(), PermissionMode::Yolo);
        assert_eq!(
            controls
                .set_model_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("model replayed to replacement channel"),
            "gpt-durable"
        );
    }

    #[test]
    fn prompt_admitted_while_spawning_survives_recovery_with_attachments() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "durable-prompt",
            "durable prompt",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let path = wal.path().to_path_buf();
        let session = new_managed_session(
            "durable-prompt".into(),
            "durable prompt".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        let mut manager = manager_with_session(session);
        let images = vec![ImageAttachment {
            data: "cGl4ZWxz".into(),
            mime_type: "image/png".into(),
        }];

        manager
            .enqueue_prompt("durable-prompt", "remember me", images.clone())
            .expect("durably admit prompt");
        drop(manager);

        let recovered = yalda::session_wal::recover_one(&path)
            .expect("read WAL")
            .expect("recover session");
        assert_eq!(recovered.pending_prompts.len(), 1);
        assert_eq!(recovered.pending_prompts[0].payload.text, "remember me");
        assert_eq!(recovered.pending_prompts[0].payload.images, images);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn failed_live_send_is_terminal_without_a_phantom_user_turn() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "failed-send",
            "failed send",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let path = wal.path().to_path_buf();
        let (transport, controls) = yalda::acp_channel::FakeTransport::new();
        let mut session = new_managed_session(
            "failed-send".into(),
            "failed send".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        session.channel = Some(transport.handle());
        session.lifecycle = SessionLifecycle::Live;
        controls.disconnect();
        let mut manager = manager_with_session(session);

        manager
            .enqueue_prompt("failed-send", "must not reappear", Vec::new())
            .expect_err("dead channel must reject send");
        let session = &manager.sessions["failed-send"];
        assert_eq!(session.lifecycle, SessionLifecycle::Disconnected);
        assert!(!session.busy);
        assert!(!session.event_log.tail_from(0).iter().any(|note| matches!(
            note,
            Notification::UserPrompt { text, .. } if text == "must not reappear"
        )));
        drop(manager);

        let recovered = yalda::session_wal::recover_one(&path)
            .expect("read WAL")
            .expect("recover session");
        assert!(
            recovered.pending_prompts.is_empty(),
            "a rejected intent must not be retried after restart"
        );
    }

    #[test]
    fn close_keeps_live_session_when_durable_delete_fails() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let mut session = new_managed_session(
            "close-failure".into(),
            "close failure".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            None,
            None,
        );
        session.wal_path = Some(dir.path().to_path_buf());
        let mut manager = manager_with_session(session);

        manager
            .do_close("close-failure")
            .expect_err("a directory cannot be removed as a WAL file");
        assert!(
            manager.sessions.contains_key("close-failure"),
            "failed durable deletion must leave the live session recoverable"
        );
    }

    #[test]
    fn successful_close_deletes_wal_before_dropping_live_session() {
        let dir = tempfile::tempdir().expect("WAL tempdir");
        let wal = yalda::session_wal::SessionWal::create_for_provider(
            dir.path(),
            "close-success",
            "close success",
            std::path::Path::new("/tmp/project"),
            PermissionMode::ReadOnly,
            AgentProvider::Codex,
        )
        .expect("create WAL");
        let path = wal.path().to_path_buf();
        let session = new_managed_session(
            "close-success".into(),
            "close success".into(),
            PathBuf::from("/tmp/project"),
            AgentProvider::Codex,
            PermissionMode::ReadOnly,
            Some(wal),
            None,
        );
        let mut manager = manager_with_session(session);

        manager.do_close("close-success").expect("close session");

        assert!(!path.exists());
        assert!(!manager.sessions.contains_key("close-success"));
    }
}
