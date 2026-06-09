//! `sketch-session-server` — thin daemon that owns ACP agent subprocesses.
//!
//! The GUI (`sketch-gpui`) connects over a Unix domain socket and
//! creates/attaches/prompts sessions. When the GUI is rebuilt and
//! relaunched, it reconnects to the same running server — agent sessions
//! survive the transition.
//!
//! Run:
//!     cargo run --bin sketch-session-server
//!
//! The GUI auto-launches this binary if not already running.

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;

use sketch::acp_channel::{
    AgentSpawner, AgentTransport, PermissionMode, RealAgentSpawner, SketchFrontend, TransportHandle,
};
use sketch::session_proto::*;

mod launchd;

// ── Lease (write-ownership) constants ──────────────────────────────
//
// A lease grants drive rights to a stable `client_id`. Expiry is driven by a
// monotonic `tokio::time::Instant` (immune to wall-clock steps); the wire
// `Lease.expires_at_unix_ms` is a display-only SystemTime stamp computed at
// emit time. The client beats `Heartbeat` every `HEARTBEAT_INTERVAL`; three
// missed beats (~`LEASE_TTL`) free a crashed owner so a candidate can promote,
// while two dropped beats / a GC pause tolerate a live owner.

/// How long a lease stays valid without a renewing heartbeat. Overridable for
/// tests via `SKETCH_LEASE_TTL_MS`.
fn lease_ttl() -> Duration {
    static TTL: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *TTL.get_or_init(|| {
        std::env::var("SKETCH_LEASE_TTL_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(15))
    })
}

/// Actor idle-sweep cadence: how often the run_manager loop proactively clears
/// expired leases and emits `LeaseChanged{None}` so an idle observing candidate
/// learns a crashed owner's lease freed (lazy eval already gates who-may-act).
const LEASE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Actor-local lease state. Holds a MONOTONIC `Instant` for expiry math (never
/// the wire millis). The wire [`Lease`] is built from this via SystemTime only
/// at broadcast time, for display.
struct LeaseState {
    client_id: String,
    expires_at: Instant,
}

impl LeaseState {
    /// Whether this lease is held by `client_id` and not yet expired at `now`.
    fn is_live_for(&self, client_id: &str, now: Instant) -> bool {
        self.client_id == client_id && self.expires_at > now
    }
}

/// Build the wire [`Lease`] (display-only millis) from an actor-local
/// [`LeaseState`] (monotonic Instant). The expiry millis is derived by adding
/// the Instant's remaining duration to the current wall clock.
fn lease_to_wire(state: &LeaseState, now: Instant) -> Lease {
    let remaining = state.expires_at.saturating_duration_since(now);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    Lease {
        client_id: state.client_id.clone(),
        expires_at_unix_ms: now_ms.saturating_add(remaining.as_millis() as u64),
    }
}

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
    log: sketch::event_log::EventLog,
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
    evicted: std::sync::atomic::AtomicBool,
}

impl ForwarderHandle {
    fn new(initial_sent_seq: u64) -> Self {
        Self {
            sent_seq: std::sync::atomic::AtomicU64::new(initial_sent_seq),
            evicted: std::sync::atomic::AtomicBool::new(false),
        }
    }
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
        resume_session_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<SessionInfo>,
    },
    Attach {
        sid: ServerSessionId,
        mode: AttachMode,
        /// Stable client identity (phase 4). Used to acquire/resume the lease;
        /// replaces the old per-connection `conn_id` ownership key.
        client_id: String,
        /// Optional reconnect cursor `(generation, index)`. Resolved by
        /// `do_attach` against the session's `channel_generation` +
        /// `event_log.len()` into the forwarder's initial `sent` value (the
        /// `usize` in the reply): the tail starts there. `None` / stale /
        /// out-of-range ⇒ `0` ⇒ full replay (unchanged behavior).
        cursor: Option<(u64, u64)>,
        // On success: (lease watch, log watch, initial forwarder cursor,
        // forwarder progress handle, granted_drive). The forwarder cursor is a
        // LOGICAL `sent_seq` (Bug 1a), NOT a `Vec` index, so a later trim can't
        // re-alias it; the progress handle is the shared `AtomicU64` the actor
        // reads for the trim floor (Bug 1b).
        // type alias would hurt readability here more than help
        #[allow(clippy::type_complexity)]
        reply: tokio::sync::oneshot::Sender<
            Result<
                (
                    watch::Receiver<Option<Lease>>,
                    watch::Receiver<LogSnapshot>,
                    u64,
                    ForwarderProgress,
                    bool,
                ),
                String,
            >,
        >,
    },
    Detach {
        sid: ServerSessionId,
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Renew a held lease (phase 4). No `event_log` side effect; renews the
    /// expiry in place. Errors if the caller no longer holds the lease.
    Heartbeat {
        sid: ServerSessionId,
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Promote {
        sid: ServerSessionId,
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Prompt {
        sid: ServerSessionId,
        text: String,
        client_id: String,
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
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Close {
        sid: ServerSessionId,
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Restart {
        sid: ServerSessionId,
        client_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Rename {
        sid: ServerSessionId,
        label: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    SetPermissionMode {
        sid: ServerSessionId,
        mode: PermissionMode,
        client_id: String,
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
        is_respawn: bool,
        // On success: (committed generation, gen_watch subscription, replay
        // fence) — everything the OWNING pump needs to drive + self-terminate.
        // `None` if the session was closed while spawning.
        reply: tokio::sync::oneshot::Sender<Option<(u64, watch::Receiver<u64>, usize)>>,
    },
    SpawnFailed {
        sid: ServerSessionId,
        reason: String,
    },

    // ── Pump-thread sourced (fire-and-forget; generation-fenced) ──
    Record {
        sid: ServerSessionId,
        generation: u64,
        event: sketch::acp_channel::ReplyEvent,
    },
    TurnCount {
        sid: ServerSessionId,
        generation: u64,
        turns: usize,
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
    name = "sketch-session-server",
    about = "Sketch ACP session-server daemon"
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

struct ManagedSession {
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
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
    /// Per-session transcript log channel. Holds the latest snapshot of
    /// `event_log` (as a cloned `Arc`); every `record`/`log_only` sends the
    /// updated snapshot via `send_replace`. The forwarder tails `[sent..]` of
    /// the latest snapshot lock-free — watch coalescing self-heals exactly like
    /// the old broadcast `Lagged` path.
    log_tx: watch::Sender<LogSnapshot>,
    /// Live forwarders' progress handles (Bug 1b). Each entry is a shared
    /// `AtomicU64` holding that forwarder's last forwarded logical `sent_seq`.
    /// `push_event` reads the MINIMUM over the still-live entries (pruning any
    /// whose `Arc::strong_count == 1` — the forwarder task dropped its clone) to
    /// compute the trim floor, so a trim never gaps the slowest live forwarder,
    /// the owner included (spec §6 owner hard-ceiling).
    forwarders: Vec<ForwarderProgress>,
    /// Per-session lease control channel (phase 4). Holds the current wire
    /// [`Lease`] (or `None`). The forwarder selects on this and emits a single
    /// `LeaseChanged` control note on holder change — replaces the old
    /// `owner_tx: watch<bool>` ownership path. Carries the wire form so the
    /// forwarder does zero conversion.
    lease_tx: watch::Sender<Option<Lease>>,
    /// The current write-ownership lease (phase 4) — the stable `client_id`
    /// allowed to drive the session (prompt / cancel / restart / set permission
    /// / close) plus its monotonic expiry. `None` when unleased, in which case
    /// an observer may `Promote` to claim it. Replaces the old `owner: conn_id`.
    /// In-memory only: never persisted (a crash stops all heartbeats, so every
    /// lease is dead by construction on restart).
    lease: Option<LeaseState>,
    /// Prompts that arrived before the ACP subprocess finished spawning.
    /// Drained in submission order once `channel` becomes `Some`.
    pending_prompts: Vec<String>,
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
    event_log: sketch::event_log::EventLog,
    /// Persisted turn count at restore time. The pump thread suppresses
    /// logging while the ACP agent's turn counter is ≤ this value, since
    /// those events are replays of turns already in `event_log`. Once the
    /// agent moves past the fence (a genuinely new turn), normal logging
    /// resumes. Zero for fresh (non-restored) sessions.
    replay_fence: usize,
    /// Durable write-ahead log for this session (ADR-0009). Every logged event
    /// is appended here so a crash (not just a clean shutdown) preserves the
    /// transcript. `None` only if the WAL couldn't be opened (we degrade to
    /// in-memory-only rather than refusing to run).
    wal: Option<sketch::session_wal::SessionWal>,
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
}

impl ManagedSession {
    fn info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.id.clone(),
            acp_session_id: self.channel.as_ref().and_then(|c| c.session_id()),
            label: self.label.clone(),
            cwd: self.cwd.clone(),
            turns: self.turns,
            connected: self.channel.as_ref().is_some_and(|c| c.is_connected()),
            permission_mode: self.permission_mode,
            // Lazy expiry: a held-but-expired lease reports as no owner.
            has_owner: self.is_leased(Instant::now()),
        }
    }

    /// Whether a non-expired lease is currently held (lazy expiry: an expired
    /// lease counts as unleased even before a sweep clears it).
    fn is_leased(&self, now: Instant) -> bool {
        matches!(&self.lease, Some(l) if l.expires_at > now)
    }

    /// Record that an event happened: append it to the durable `event_log`
    /// (source of truth) **and** fire the broadcast wake in one step. This is
    /// the single mutator for "a logged event happened" — every log+broadcast
    /// site routes through here so the two writes can never skew (one appended
    /// without waking subscribers, or one broadcast without being logged).
    ///
    /// The `LeaseChanged` broadcast-only path (`broadcast_lease_changed`) is
    /// deliberately NOT routed through here: it is transient lease state,
    /// not transcript, and must never land in `event_log`.
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
    /// HIGH-WATER DISCONNECT (spec §6, MAJOR): the owner hard-ceiling means a
    /// slow/paused forwarder (e.g. a backgrounded GUI owner under App Nap that
    /// stops draining its socket) pins the floor — the trim can't fire and the
    /// `Vec` grows. The 60s slow-sub write timeout is the only other reaper, and
    /// a forwarder that drains just enough to keep resetting that timer could pin
    /// growth EFFECTIVELY unbounded. So BEFORE computing the floor we
    /// [`enforce_high_water`](Self::enforce_high_water): when the backlog
    /// (`tip_seq - floor`) crosses [`event_log_high_water`], the slowest
    /// forwarder is force-DISCONNECTED (a clean from-base reconnect, NOT a silent
    /// gap) and dropped from the floor `min`, so the trim resumes and growth is
    /// bounded. The owner is NOT exempt (the App Nap case) — a wedged owner is
    /// cleanly bounced and reclaims its lease via same-`client_id` reclaim.
    fn push_event(&mut self, note: Notification) {
        self.wal_append(&note);
        self.event_log.push(note);
        let cap = sketch::event_log::event_log_cap();
        // Low-water mark: ¾ of the cap, leaving a slot for the prepended marker
        // and headroom so the next several pushes don't re-trim.
        let target = (cap * 3 / 4).max(1).min(cap.saturating_sub(1));
        // Disconnect-before-gap (spec §6): evict any forwarder whose backlog has
        // crossed the high-water bound, so the floor below is not pinned by a
        // wedged consumer and the trim can bound growth.
        self.enforce_high_water();
        let floor = self.compaction_floor();
        if let Some(trim) = self.event_log.trim(cap, target, floor) {
            // Splice a CompactedSummary marker at the NEW front carrying the new
            // base seq, so a from-base rebuild begins with "history compacted
            // through turn N" rather than an unexplained jump (spec §6/§7).
            //
            // Bug 2: the marker reuses the LAST-DROPPED slot — `prepend` decrements
            // `log_base` by one so survivor seqs stay stable. The marker's own seq
            // must therefore be `new_base - 1` (the decremented base), so that after
            // the prepend `marker.seq == log_base` and the seq space is contiguous.
            let through_turn = trim.through_turn.unwrap_or(0);
            let marker_seq = trim.new_base.saturating_sub(1);
            let marker = sketch::agent_event::AgentEvent::new(
                self.id.clone(),
                self.channel_generation,
                through_turn,
                marker_seq,
                sketch::agent_event::AgentEventKind::CompactedSummary {
                    through_turn,
                    summary: format!(
                        "history compacted: {} earlier event(s) trimmed (through turn {through_turn})",
                        trim.dropped
                    ),
                },
            );
            self.event_log
                .prepend(Notification::Agent { event: marker });
        }
        self.publish_snapshot();
    }

    /// Publish the current `event_log` (plus the live `channel_generation`) on the
    /// `log_tx` watch, waking every forwarder. The forwarder re-resolves its
    /// logical `sent_seq` against the published `log_base` (Bug 1a), so a trim
    /// that shortened the `Vec` can never make it slice a stale offset.
    fn publish_snapshot(&self) {
        let _ = self.log_tx.send_replace(LogSnapshot {
            log: self.event_log.clone(),
            generation: self.channel_generation,
        });
    }

    /// The trim floor (spec §6 owner hard-ceiling, Bug 1b): the minimum logical
    /// `sent_seq` over all still-live forwarders. A dead forwarder dropped its
    /// progress `Arc`, so it is the SOLE remaining ref (`strong_count == 1`) and
    /// is pruned here; `u64::MAX` (no floor) when no live forwarder remains.
    ///
    /// `&mut self` because it prunes dead handles as a side effect. The owner is
    /// always a live forwarder, so it is implicitly included in the `min` — the
    /// trim can never drop below the owner's forwarded position.
    fn compaction_floor(&mut self) -> u64 {
        use std::sync::atomic::Ordering;
        self.forwarders.retain(|p| Arc::strong_count(p) > 1);
        self.forwarders
            .iter()
            .map(|p| p.sent_seq.load(Ordering::Acquire))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// High-water backlog bound (spec §6, MAJOR — disconnect-before-gap).
    ///
    /// The floor ([`compaction_floor`]) is a HARD ceiling, so a slow/paused
    /// forwarder (e.g. a backgrounded GUI owner under macOS App Nap that stops
    /// draining its socket) pins `min(sent_seq)` and prevents the trim from
    /// firing, letting the in-memory `Vec` grow without bound. When the backlog
    /// `tip_seq - floor` crosses [`event_log_high_water`], force-DISCONNECT the
    /// SLOWEST live forwarder (the one whose `sent_seq == floor`): set its
    /// `evicted` flag (the forwarder task observes it on its next wake — the
    /// `publish_snapshot` at the end of `push_event` provides that wake — and
    /// returns, closing its write half) and prune its handle from `forwarders`
    /// HERE so it immediately drops out of the floor `min`. The trim then
    /// proceeds and growth is bounded.
    ///
    /// This is NOT a silent in-place gap (which §6 forbids for the owner): the
    /// disconnected client gets a clean EOF, reconnects, and rebuilds from base
    /// via `resolve_cursor` → `FromBase` (surfacing the `CompactedSummary`
    /// marker). The owner is therefore NOT exempt — a wedged owner that crosses
    /// the mark is bounced and reclaims its lease deterministically on reconnect
    /// (phase-4 same-`client_id` resume). The lease lives in `ManagedSession`
    /// keyed by `client_id`, independent of the forwarder task, so evicting a
    /// forwarder does NOT touch or free the lease.
    ///
    /// Loops so multiple forwarders past the mark are all evicted in one pass:
    /// after dropping the slowest, the floor rises to the next-slowest, which may
    /// itself still be past the bound. No live forwarders → floor is `u64::MAX`,
    /// the backlog is `0` (saturating), nothing to evict (cap-only mode, spec §6
    /// "no live subscribers" — floor = tip).
    fn enforce_high_water(&mut self) {
        use std::sync::atomic::Ordering;
        let high_water = sketch::event_log::event_log_high_water() as u64;
        let tip = self.event_log.tip_seq();
        loop {
            // Prune dead handles, then find the slowest live forwarder.
            self.forwarders.retain(|p| Arc::strong_count(p) > 1);
            let Some((slowest_idx, floor)) = self
                .forwarders
                .iter()
                .enumerate()
                .map(|(i, p)| (i, p.sent_seq.load(Ordering::Acquire)))
                .min_by_key(|&(_, seq)| seq)
            else {
                return; // no live forwarders → cap-only mode, nothing to evict
            };
            // Backlog is tip minus the slowest forwarded position. Saturating so a
            // forwarder somehow ahead of tip can't underflow.
            let backlog = tip.saturating_sub(floor);
            if backlog <= high_water {
                return; // slowest forwarder is within the bound — done
            }
            // Force-disconnect the slowest forwarder: flag it (the forwarder task
            // exits at its next wake) and drop the actor's handle now so the floor
            // `min` no longer includes it and the trim can proceed.
            let handle = self.forwarders.swap_remove(slowest_idx);
            handle.evicted.store(true, Ordering::Release);
            tracing::warn!(
                session_id = %&self.id[..8.min(self.id.len())],
                backlog,
                high_water,
                "high-water disconnect: evicting slowest forwarder (sent_seq {floor}) — \
                 in-memory backlog past threshold (wedged/paused consumer)"
            );
            // Loop: the floor rises to the next-slowest, which may still be past
            // the bound.
        }
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
    fn record_agent(&mut self, kind: sketch::agent_event::AgentEventKind) {
        let event = sketch::agent_event::AgentEvent::new(
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
                    use sketch::agent_event::AgentEventKind;
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

    /// Broadcast a `LeaseChanged` to all attached connections by publishing the
    /// current lease (as the wire [`Lease`]) on the lease watch. Not appended to
    /// `event_log` — lease state is transient, not transcript. Call ONLY on a
    /// holder change (acquire / release / promote / sweep), never on a pure
    /// heartbeat renew, so each transition fires exactly one notification.
    fn broadcast_lease_changed(&self, now: Instant) {
        let wire = self.lease.as_ref().map(|l| lease_to_wire(l, now));
        let _ = self.lease_tx.send_replace(wire);
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
    fn apply_channel_state(&mut self, mut handle: TransportHandle, is_respawn: bool) {
        handle.set_permission_mode(self.permission_mode);
        for text in std::mem::take(&mut self.pending_prompts) {
            if let Err(e) = handle.send(&text) {
                tracing::error!(error = %e, "failed to flush queued prompt");
            }
        }
        let acp_session_id = handle.session_id();
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
        self.record_agent(sketch::agent_event::channel_opened_kind(
            acp_session_id.is_some(),
        ));
        self.record(Notification::SessionAttached {
            session_id: self.id.clone(),
            acp_session_id,
        });
    }
}

// ── Session manager ────────────────────────────────────────────────

/// Build a fresh `ManagedSession` for a brand-new session.
fn new_managed_session(
    id: ServerSessionId,
    label: String,
    cwd: PathBuf,
    permission_mode: PermissionMode,
    wal: Option<sketch::session_wal::SessionWal>,
) -> ManagedSession {
    let event_log = sketch::event_log::EventLog::new();
    let (log_tx, _) = watch::channel(LogSnapshot {
        log: event_log.clone(),
        generation: 0,
    });
    let (lease_tx, _) = watch::channel(None);
    let (gen_watch, _) = watch::channel(0u64);
    ManagedSession {
        id,
        label,
        cwd,
        channel: None,
        channel_generation: 0,
        gen_watch,
        turns: 0,
        permission_mode,
        log_tx,
        forwarders: Vec::new(),
        lease_tx,
        lease: None,
        pending_prompts: Vec::new(),
        event_log,
        replay_fence: 0,
        wal,
        agent_seq: 0,
    }
}

/// A pending ACP resume job produced by WAL recovery — the seed map plus the
/// data each resume worker needs to re-spawn its subprocess.
struct ResumeJob {
    session_id: ServerSessionId,
    cwd: PathBuf,
    acp_session_id: String,
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
) -> Option<sketch::session_wal::SessionWal> {
    let dir = session_wal_dir()?;
    match sketch::session_wal::SessionWal::create(&dir, id, label, cwd, permission_mode) {
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
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Create {
            cwd,
            label,
            resume_session_id,
            reply,
        });
        rx.await.expect("actor dropped a Create reply")
    }

    async fn send_attach(
        &self,
        sid: &str,
        mode: AttachMode,
        client_id: String,
        cursor: Option<(u64, u64)>,
    ) -> Result<
        (
            watch::Receiver<Option<Lease>>,
            watch::Receiver<LogSnapshot>,
            u64,
            ForwarderProgress,
            bool,
        ),
        String,
    > {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Attach {
            sid: sid.to_string(),
            mode,
            client_id,
            cursor,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_detach(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Detach {
            sid: sid.to_string(),
            client_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_heartbeat(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Heartbeat {
            sid: sid.to_string(),
            client_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_promote(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Promote {
            sid: sid.to_string(),
            client_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_prompt(&self, sid: &str, text: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Prompt {
            sid: sid.to_string(),
            text: text.to_string(),
            client_id,
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

    async fn send_cancel(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Cancel {
            sid: sid.to_string(),
            client_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_close(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Close {
            sid: sid.to_string(),
            client_id,
            reply,
        });
        rx.await.unwrap_or_else(|_| Err("actor unavailable".into()))
    }

    async fn send_restart(&self, sid: &str, client_id: String) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::Restart {
            sid: sid.to_string(),
            client_id,
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

    async fn send_set_permission_mode(
        &self,
        sid: &str,
        mode: PermissionMode,
        client_id: String,
    ) -> Result<(), String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.cmd_tx.send(Command::SetPermissionMode {
            sid: sid.to_string(),
            mode,
            client_id,
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
fn restore_seed_from_disk() -> (HashMap<ServerSessionId, ManagedSession>, Vec<ResumeJob>) {
    let mut sessions = HashMap::new();
    let mut jobs = Vec::new();
    let Some(dir) = session_wal_dir() else {
        return (sessions, jobs);
    };
    let recovered = sketch::session_wal::recover_all(&dir);
    for rs in recovered {
        let sid = rs.server_session_id.clone();
        let Some(acp_session_id) = rs.acp_session_id.clone() else {
            tracing::warn!(
                session_id = %&sid[..8.min(sid.len())],
                "discarding recovered session: no acp_session_id to resume"
            );
            let _ = std::fs::remove_file(&rs.path);
            continue;
        };

        let wal = match sketch::session_wal::SessionWal::reopen(rs.path.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::error!(
                    session_id = %&sid[..8.min(sid.len())],
                    error = %e,
                    "WAL reopen failed"
                );
                None
            }
        };

        // Phase-8 Stage A: resume the agent seq one past the highest persisted
        // `Agent` seq on this (generation-0) recovered log, so post-restore
        // agent events extend the durable seq space monotonically (spec §2/§5 —
        // turn/seq forwarded verbatim, then continue).
        let agent_seq = rs
            .event_log
            .iter()
            .filter_map(|n| match n {
                Notification::Agent { event } => Some(event.seq),
                _ => None,
            })
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        // Stage B: recovery always starts from `log_base == 0` — the on-disk WAL
        // is never trimmed, so the restored transcript is a faithful append-
        // ordered prefix from seq 0 (spec §6 / ringbuffer note: on restart
        // log_base resets to the seq of the first recovered event, which is 0).
        let event_log = sketch::event_log::EventLog::from_recovered(rs.event_log, 0);
        // Seed the watch with the recovered log so the first tail sees history.
        let (log_tx, _) = watch::channel(LogSnapshot {
            log: event_log.clone(),
            generation: 0,
        });
        let (lease_tx, _) = watch::channel(None);
        let (gen_watch, _) = watch::channel(0u64);
        let session = ManagedSession {
            id: sid.clone(),
            label: rs.label.clone(),
            cwd: rs.cwd.clone(),
            channel: None,
            channel_generation: 0,
            gen_watch,
            turns: rs.turns,
            permission_mode: rs.permission_mode,
            log_tx,
            forwarders: Vec::new(),
            lease_tx,
            lease: None,
            pending_prompts: Vec::new(),
            event_log,
            replay_fence: rs.turns,
            wal,
            agent_seq,
        };

        tracing::info!(
            session_id = %&sid[..8.min(sid.len())],
            events = session.event_log.len(),
            turns = rs.turns,
            acp_session_id = %&acp_session_id[..8.min(acp_session_id.len())],
            "recovering session"
        );

        sessions.insert(sid.clone(), session);
        jobs.push(ResumeJob {
            session_id: sid,
            cwd: rs.cwd,
            acp_session_id,
        });
    }
    (sessions, jobs)
}

/// Spawn the OS thread that re-spawns a recovered session's ACP subprocess with
/// `--resume`, then publishes the transport via the actor inlet.
fn spawn_resume_worker(
    cmd_tx: mpsc::UnboundedSender<Command>,
    job: ResumeJob,
    spawner: Arc<dyn AgentSpawner>,
) {
    let ResumeJob {
        session_id,
        cwd,
        acp_session_id,
    } = job;
    std::thread::Builder::new()
        .name(format!(
            "acp-resume-{}",
            &session_id[..8.min(session_id.len())]
        ))
        .spawn(move || {
            // SAFETY: dedicated spawn thread; see create worker.
            unsafe {
                std::env::set_var("SKETCH_SESSION_MANAGED", "1");
            }
            let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
            match spawner.spawn(&cmd, Some(cwd), Some(acp_session_id), SketchFrontend::Gpui) {
                Ok(client) => {
                    // Resume from disk → is_respawn=false (generation stays 0).
                    //
                    // MINOR (5): `apply_channel_state` then records a ChannelOpened
                    // at generation 0 — but the RECOVERED log may ALREADY contain a
                    // gen-0 ChannelOpened from the original session, so the restored
                    // transcript can carry TWO gen-0 ChannelOpened events. This is
                    // HARMLESS for the §4 generation-delta rebaseline: a consumer
                    // rebaselines only on a STRICTLY-newer generation, and both are
                    // gen 0, so the second is an idempotent no-op (its
                    // `reset_for_replay` would just rebuild the same prefix). Noted
                    // so a future reader doesn't treat the duplicate as a bug.
                    publish_channel(&cmd_tx, &session_id, client, false);
                }
                Err(e) => {
                    tracing::error!(
                        session_id = %&session_id[..8.min(session_id.len())],
                        error = %e,
                        "failed to resume session"
                    );
                    let _ = cmd_tx.send(Command::SpawnFailed {
                        sid: session_id,
                        reason: format!("resume failed: {e}"),
                    });
                }
            }
        })
        .ok();
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
) {
    let mut mgr = Manager {
        sessions,
        events,
        default_permission_mode,
        cmd_tx,
        spawner,
    };
    // Phase 4: the loop also drives a periodic lease sweep. The sweep is a
    // PROACTIVE side-effect only (lazy expiry in every gate/attach already
    // governs who-may-act); its job is to emit `LeaseChanged{None}` within
    // ~`LEASE_SWEEP_INTERVAL` so an idle observing candidate learns a crashed
    // owner's lease freed. Keeping it INSIDE this select preserves the
    // single-writer invariant (ADR-0012): `apply`/`sweep` are the only mutators
    // and they never run concurrently. (Do NOT spawn a second task touching the
    // map.)
    let mut sweep = tokio::time::interval(LEASE_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                match maybe_cmd {
                    Some(cmd) => mgr.apply(cmd),
                    None => break, // inlet closed: shutdown
                }
            }
            _ = sweep.tick() => {
                mgr.sweep_expired_leases();
            }
        }
    }
}

impl Manager {
    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Create {
                cwd,
                label,
                resume_session_id,
                reply,
            } => {
                let info = self.do_create(cwd, label, resume_session_id);
                let _ = reply.send(info);
            }
            Command::Attach {
                sid,
                mode,
                client_id,
                cursor,
                reply,
            } => {
                let _ = reply.send(self.do_attach(&sid, mode, &client_id, cursor));
            }
            Command::Detach {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_detach(&sid, &client_id));
            }
            Command::Heartbeat {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_heartbeat(&sid, &client_id));
            }
            Command::Promote {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_promote(&sid, &client_id));
            }
            Command::Prompt {
                sid,
                text,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_prompt(&sid, &text, &client_id));
            }
            Command::AdminPrompt {
                session_id,
                text,
                reply,
            } => {
                // Ungated: enqueue directly, no owner check (ADR-0015).
                let _ = reply.send(self.enqueue_prompt(&session_id, &text));
            }
            Command::Cancel {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_cancel(&sid, &client_id));
            }
            Command::Close {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_close(&sid, &client_id));
            }
            Command::Restart {
                sid,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_restart(&sid, &client_id));
            }
            Command::Rename { sid, label, reply } => {
                let _ = reply.send(self.do_rename(&sid, label));
            }
            Command::SetPermissionMode {
                sid,
                mode,
                client_id,
                reply,
            } => {
                let _ = reply.send(self.do_set_permission_mode(&sid, mode, &client_id));
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
                is_respawn,
                reply,
            } => {
                let published = match self.sessions.get_mut(&sid) {
                    Some(s) => {
                        s.apply_channel_state(handle, is_respawn);
                        Some((
                            s.channel_generation,
                            s.gen_watch.subscribe(),
                            s.replay_fence,
                        ))
                    }
                    None => None,
                };
                let _ = reply.send(published);
            }
            Command::SpawnFailed { sid, reason } => {
                if let Some(s) = self.sessions.get_mut(&sid) {
                    s.record(Notification::SessionDetached {
                        session_id: sid.clone(),
                        reason,
                    });
                }
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
                if let Some(kind) = sketch::agent_event::agent_kind_from_reply(&event) {
                    s.record_agent(kind);
                } else if matches!(event, sketch::acp_channel::ReplyEvent::ReplayComplete) {
                    s.record_agent(sketch::agent_event::replay_end_kind());
                }
                // NOTE: a worker `ReplyEvent::TurnEnded { count }` (only emitted
                // under SKETCH_EMIT_TURN_ENDED=1) is intentionally NOT mapped
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
                // A `turns <= replay_fence` signal is the pump telling us replay
                // is complete: clear the fence, no TurnEnded for a replay turn.
                if s.replay_fence > 0 && turns <= s.replay_fence {
                    s.replay_fence = 0;
                    return;
                }
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
                    event: sketch::agent_event::AgentEvent::new(
                        sid.clone(),
                        channel_generation,
                        completed_turn,
                        agent_seq,
                        sketch::agent_event::turn_ended_kind(
                            sketch::agent_event::TurnOutcome::Completed,
                        ),
                    ),
                });
                s.record(Notification::TurnEnded {
                    session_id: sid.clone(),
                    turn_count: turns,
                    generation: channel_generation,
                });
            }
            Command::AgentDisconnected { sid, generation } => {
                let Some(s) = self.sessions.get_mut(&sid) else {
                    return;
                };
                if generation != s.channel_generation {
                    return; // stale reader (superseded by a restart)
                }
                s.record(Notification::SessionDetached {
                    session_id: sid.clone(),
                    reason: "agent disconnected".into(),
                });
                s.channel = None;
            }
        }
    }

    fn do_create(
        &mut self,
        cwd: PathBuf,
        label: String,
        resume_session_id: Option<String>,
    ) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let permission_mode = self.default_permission_mode;
        // Open the durable WAL up front so even a crash immediately after create
        // can recover the session's identity.
        let wal = open_session_wal(&id, &label, &cwd, permission_mode);
        let session = new_managed_session(id.clone(), label, cwd.clone(), permission_mode, wal);

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
        std::thread::Builder::new()
            .name(format!("acp-spawn-{}", &id[..8]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; single-purpose server.
                unsafe {
                    std::env::set_var("SKETCH_SESSION_MANAGED", "1");
                }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match spawner.spawn(&cmd, Some(cwd), resume_session_id, SketchFrontend::Gpui) {
                    Ok(client) => {
                        // Fresh spawn → is_respawn = false, generation stays 0.
                        publish_channel(&cmd_tx, &session_id, client, false);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid: session_id,
                            reason: format!("spawn failed: {e}"),
                        });
                    }
                }
            })
            .ok();

        info
    }

    fn do_close(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        let now = Instant::now();
        match self.sessions.get(session_id) {
            Some(s) if holds_lease(s, client_id, now) => {
                // Removing the session drops its TransportHandle (prompt_tx
                // clone). The owning pump observes the close (inlet still open
                // but no map entry → its generation check / disconnect breaks it)
                // and drops its client off-actor. Bump gen_watch so any owning
                // pump wakes immediately to self-terminate.
                let session = self.sessions.remove(session_id);
                if let Some(s) = &session {
                    let _ = s
                        .gen_watch
                        .send_replace(s.channel_generation.wrapping_add(1));
                }
                // Explicit close = intentional end of life: delete the durable
                // WAL so this session is NOT recovered on the next start.
                if let Some(wal) = session.and_then(|s| s.wal) {
                    wal.remove();
                }
                let _ = self.events.send(Notification::SessionClosed {
                    session_id: session_id.to_string(),
                });
                Ok(())
            }
            Some(_) => Err("only the lease holder can close the session".into()),
            None => Err(format!("no such session: {session_id}")),
        }
    }

    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    fn do_attach(
        &mut self,
        session_id: &str,
        mode: AttachMode,
        client_id: &str,
        cursor: Option<(u64, u64)>,
    ) -> Result<
        (
            watch::Receiver<Option<Lease>>,
            watch::Receiver<LogSnapshot>,
            u64,
            ForwarderProgress,
            bool,
        ),
        String,
    > {
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;

        // Lease acquire/resume (phase 4). Deterministic — no retry, no error on
        // contention. Owner mode with a NON-empty client_id may take the lease:
        //   - free (None)                       -> first-claim
        //   - same client_id (live OR expired)  -> RESUME (renew / re-grant)
        //   - different LIVE client_id          -> silent downgrade to Observer
        // An empty client_id (headless / pre-phase-4) never acquires a lease.
        // Observer mode never touches the lease. The whole decision is one
        // synchronous critical section on the single-writer actor: read, the
        // Instant comparison, and the write share `now` with no await between —
        // the actor IS the mutual exclusion (no TOCTOU, first-claim-wins).
        let mut granted_drive = false;
        if mode == AttachMode::Owner && !client_id.is_empty() {
            let acquire = match &session.lease {
                None => true,                                // free
                Some(l) if l.client_id == client_id => true, // same id -> resume
                Some(l) if l.expires_at <= now => true,      // prior holder expired
                Some(_) => false,                            // different live id
            };
            if acquire {
                let changed_holder =
                    session.lease.as_ref().map(|l| l.client_id.as_str()) != Some(client_id);
                session.lease = Some(LeaseState {
                    client_id: client_id.to_string(),
                    expires_at: now + lease_ttl(),
                });
                granted_drive = true;
                // Only broadcast on holder CHANGE (never a pure same-id renew),
                // so each transition fires exactly one LeaseChanged.
                if changed_holder {
                    session.broadcast_lease_changed(now);
                }
            }
            // else: silent downgrade to Observer. Attach still Ok with full
            // replay; granted_drive stays false.
        }

        let lease_rx = session.lease_tx.subscribe();
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

        // Register this forwarder's progress handle (Bug 1b): one clone goes to
        // the forwarder task (returned), one is retained on the session so the
        // trim floor `min`s over it. Seed it at the initial `sent_seq`. Prune any
        // dead handles while we're here.
        session.forwarders.retain(|p| Arc::strong_count(p) > 1);
        let progress: ForwarderProgress = Arc::new(ForwarderHandle::new(initial_sent_seq));
        session.forwarders.push(Arc::clone(&progress));

        Ok((lease_rx, log_rx, initial_sent_seq, progress, granted_drive))
    }

    /// Renew a held lease (phase 4). Same-`client_id` live → push expiry (no
    /// LeaseChanged, holder unchanged); same-`client_id` but expired/free →
    /// lazy re-grant; otherwise the caller no longer holds the lease → Err so
    /// the GUI re-attaches (resumes-or-observes).
    fn do_heartbeat(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        match &session.lease {
            Some(l) if l.client_id == client_id => {
                // Live OR expired but same id: renew/re-grant in place. The
                // holder is unchanged (a lazy re-grant of an expired-but-same
                // lease is still the same holder), so NO LeaseChanged is emitted.
                session.lease = Some(LeaseState {
                    client_id: client_id.to_string(),
                    expires_at: now + lease_ttl(),
                });
                Ok(())
            }
            _ => Err("lease lost".into()),
        }
    }

    fn do_promote(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        if client_id.is_empty() {
            return Err("promote requires a client identity".into());
        }
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        // Claim if free, expired, or already ours (idempotent). A different
        // LIVE holder blocks promote (blue-green invariant: a candidate may
        // only take over once the original releases / its lease lapses).
        let claim = match &session.lease {
            None => true,
            Some(l) if l.client_id == client_id => true,
            Some(l) if l.expires_at <= now => true,
            Some(_) => false,
        };
        if claim {
            let changed_holder =
                session.lease.as_ref().map(|l| l.client_id.as_str()) != Some(client_id);
            session.lease = Some(LeaseState {
                client_id: client_id.to_string(),
                expires_at: now + lease_ttl(),
            });
            if changed_holder {
                session.broadcast_lease_changed(now);
            }
            Ok(())
        } else {
            Err("session still leased by another GUI".into())
        }
    }

    fn do_detach(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        // EXPLICIT detach = clean handoff: release the lease immediately so a
        // deliberate blue-green handoff doesn't wait the full TTL. (Socket-EOF
        // detach does NOT clear the lease — see `detach_on_disconnect`.) Only
        // the holder may release.
        if matches!(&session.lease, Some(l) if l.client_id == client_id) {
            session.lease = None;
            session.broadcast_lease_changed(now);
        }
        Ok(())
    }

    fn do_prompt(&mut self, session_id: &str, text: &str, client_id: &str) -> Result<(), String> {
        {
            let now = Instant::now();
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if !holds_lease(session, client_id, now) {
                return Err("only the lease holder can send prompts".into());
            }
        }
        self.enqueue_prompt(session_id, text)
    }

    /// Owner-gate-free core of the prompt path: log the user's prompt durably,
    /// then hand it to the live channel (or queue it if the agent is still
    /// spawning). Used by both the owner-gated [`do_prompt`] and the ungated
    /// headless [`Command::AdminPrompt`] path (ADR-0015). The ONLY difference
    /// between the two is the owner check; everything that makes a prompt
    /// durable + drives the turn lives here.
    fn enqueue_prompt(&mut self, session_id: &str, text: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        // Log the user's prompt so re-attaching GUIs can replay it (and so it
        // survives a crash — UserPrompt is a turn boundary that fsyncs). Only
        // appended to event_log + WAL, not broadcast.
        session.log_only(Notification::UserPrompt {
            session_id: session_id.to_string(),
            text: text.to_string(),
        });
        // ADDITIVE (spec §9): the canonical AgentEvent for the user's prompt —
        // `UserMessage` folds both the live submit and the replay echo, deduped
        // by identity (session, generation, turn) per spec §2/§5. Recorded
        // alongside the legacy UserPrompt; the GUI reducer ignores the Agent
        // stream this pass.
        session.record_agent(sketch::agent_event::AgentEventKind::UserMessage {
            text: text.to_string(),
        });
        match session.channel.as_ref() {
            Some(channel) => channel.send(text).map_err(|e| format!("send failed: {e}")),
            None => {
                session.pending_prompts.push(text.to_string());
                Ok(())
            }
        }
    }

    fn do_cancel(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if !holds_lease(session, client_id, now) {
            return Err("only the lease holder can cancel".into());
        }
        if let Some(channel) = session.channel.as_ref() {
            channel.cancel();
        }
        Ok(())
    }

    fn do_restart(&mut self, session_id: &str, client_id: &str) -> Result<(), String> {
        let (cwd, resume_id) = {
            let now = Instant::now();
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            if !holds_lease(session, client_id, now) {
                return Err("only the lease holder can restart".into());
            }
            let resume = session.channel.as_ref().and_then(|c| c.session_id());
            (session.cwd.clone(), resume)
        };

        let cmd_tx = self.cmd_tx.clone();
        let spawner = Arc::clone(&self.spawner);
        let sid = session_id.to_string();
        std::thread::Builder::new()
            .name(format!("acp-restart-{}", &sid[..8.min(sid.len())]))
            .spawn(move || {
                // SAFETY: dedicated spawn thread; see do_create.
                unsafe {
                    std::env::set_var("SKETCH_SESSION_MANAGED", "1");
                }
                let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
                match spawner.spawn(&cmd, Some(cwd), resume_id, SketchFrontend::Gpui) {
                    Ok(client) => {
                        // is_respawn=true bumps generation + gen_watch so the OLD
                        // pump self-terminates and drops its client off-actor.
                        publish_channel(&cmd_tx, &sid, client, true);
                    }
                    Err(e) => {
                        let _ = cmd_tx.send(Command::SpawnFailed {
                            sid,
                            reason: format!("restart failed: {e}"),
                        });
                    }
                }
            })
            .ok();
        Ok(())
    }

    fn do_rename(&mut self, session_id: &str, label: String) -> Result<(), String> {
        {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("no such session: {session_id}"))?;
            session.label = label.clone();
        }
        let _ = self.events.send(Notification::SessionRenamed {
            session_id: session_id.to_string(),
            label,
        });
        Ok(())
    }

    fn do_set_permission_mode(
        &mut self,
        session_id: &str,
        mode: PermissionMode,
        client_id: &str,
    ) -> Result<(), String> {
        let now = Instant::now();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("no such session: {session_id}"))?;
        if !holds_lease(session, client_id, now) {
            return Err("only the lease holder can change permission mode".into());
        }
        session.permission_mode = mode;
        if let Some(channel) = &session.channel {
            channel.set_permission_mode(mode);
        }
        Ok(())
    }

    fn do_admin_status(&self) -> AdminSnapshot {
        let now = Instant::now();
        let infos = self
            .sessions
            .values()
            .map(|s| AdminSessionInfo {
                session_id: s.id.clone(),
                label: s.label.clone(),
                connected: s.channel.is_some(),
                has_owner: s.is_leased(now),
                // Only report a LIVE lease as the holder; a held-but-expired
                // lease reads as unleased (lazy expiry), matching has_owner.
                lease_holder: s
                    .lease
                    .as_ref()
                    .filter(|l| l.expires_at > now)
                    .map(|l| lease_to_wire(l, now)),
                turns: s.turns,
                event_log_len: s.event_log.len(),
                log_base: s.event_log.log_base(),
                subscriber_count: s.log_tx.receiver_count(),
                channel_generation: s.channel_generation,
                permission_mode: s.permission_mode,
            })
            .collect();
        AdminSnapshot {
            session_count: self.sessions.len(),
            sessions: infos,
        }
    }

    /// Periodic sweep (phase 4): clear every expired lease and emit
    /// `LeaseChanged{None}` so an idle observing candidate learns a crashed
    /// owner's lease freed within ~`LEASE_SWEEP_INTERVAL`. This is a PROACTIVE
    /// side-effect only — lazy expiry in the gates already governs who-may-act,
    /// so correctness does not depend on this running on time. Stays on the
    /// single-writer actor task (called from the run_manager select), so it
    /// never races `apply` (ADR-0012).
    fn sweep_expired_leases(&mut self) {
        let now = Instant::now();
        for s in self.sessions.values_mut() {
            if matches!(&s.lease, Some(l) if l.expires_at <= now) {
                s.lease = None;
                s.broadcast_lease_changed(now);
            }
        }
    }
}

/// Whether `session` is currently leased to `client_id` and that lease has not
/// expired at `now`. The single gate predicate applied identically at every
/// write verb (prompt / cancel / restart / set-permission / close). An empty
/// `client_id` never holds a lease. Lazy expiry: an expired lease fails the
/// gate even before a sweep clears it.
fn holds_lease(session: &ManagedSession, client_id: &str, now: Instant) -> bool {
    !client_id.is_empty() && matches!(&session.lease, Some(l) if l.is_live_for(client_id, now))
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
    is_respawn: bool,
) {
    let handle = client.handle();
    let (reply, rx) = tokio::sync::oneshot::channel();
    if cmd_tx
        .send(Command::PublishChannel {
            sid: session_id.clone(),
            handle,
            is_respawn,
            reply,
        })
        .is_err()
    {
        drop(client); // actor gone — drop the client on this worker thread.
        return;
    }
    // Blocking recv on this OS worker thread — never on the actor task.
    match rx.blocking_recv() {
        Ok(Some((generation, gen_rx, replay_fence))) => {
            spawn_pump_thread(
                cmd_tx.clone(),
                session_id.clone(),
                client,
                generation,
                gen_rx,
                replay_fence,
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
) {
    std::thread::Builder::new()
        .name(format!("pump-{}", &session_id[..8.min(session_id.len())]))
        .spawn(move || {
            // Per-session generation watch: a restart (generation bump) wakes us
            // to self-terminate + drop the client off the actor task.
            let gen_rx = gen_rx;

            let mut last_turns: usize = 0;
            // Local mirror of the session's replay fence. Suppression decisions
            // stay pump-side (cycle granularity); the actor only sees Records
            // that should be logged.
            let mut replay_fence: usize = initial_replay_fence;

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

                let tail_events: Vec<sketch::acp_channel::ReplyEvent> = if turn_ended {
                    std::iter::from_fn(|| client.try_recv()).collect()
                } else {
                    Vec::new()
                };

                // ── Replay fence: suppress duplicate events ──────────
                // A restored/resumed session replays prior turns. Drain them
                // (so the channel doesn't back up) but emit no Records until the
                // agent moves past the fence. The fence-clear is signalled to
                // the actor via a TurnCount whose `turns <= replay_fence`.
                if replay_fence > 0 && current_turns <= replay_fence {
                    let drained = !events.is_empty();
                    if turn_ended {
                        last_turns = current_turns;
                        if current_turns == replay_fence {
                            // Replay complete — tell the actor to clear the
                            // session's fence (no TurnEnded for a replay turn).
                            let _ = cmd_tx.send(Command::TurnCount {
                                sid: session_id.clone(),
                                generation: my_generation,
                                turns: current_turns,
                            });
                            replay_fence = 0;
                            tracing::info!(
                                session_id = %&session_id[..8.min(session_id.len())],
                                turn = current_turns,
                                "replay fence cleared"
                            );
                        }
                    }
                    if !drained && !more_pending {
                        std::thread::sleep(PUMP_IDLE_SLEEP);
                    }
                    continue;
                }

                let drained_events = !events.is_empty();

                // Forward events first (in order).
                for ev in events {
                    if std::env::var("SKETCH_CHUNKLOG").is_ok()
                        && let sketch::acp_channel::ReplyEvent::Chunk(t) = &ev
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
                    let _ = cmd_tx.send(Command::TurnCount {
                        sid: session_id.clone(),
                        generation: my_generation,
                        turns: current_turns,
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
                resume_session_id,
            } => {
                let info = manager.send_create(cwd, label, resume_session_id).await;
                Response::Ok {
                    data: ResponseData::Session { session: info },
                }
            }

            Request::Attach {
                session_id,
                mode,
                client_id,
                cursor,
            } => {
                match manager
                    .send_attach(&session_id, mode, client_id, cursor)
                    .await
                {
                    Ok((lease_rx, log_rx, initial_sent_seq, progress, granted_drive)) => {
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
                            Arc::clone(&manager),
                            session_id.clone(),
                            w,
                            lease_rx,
                            log_rx,
                            initial_sent_seq,
                            progress,
                        ));
                        subscribed.insert(session_id, handle);
                        // Phase 4: tell the client its role explicitly so it
                        // never has to infer ownership from an error string.
                        Response::Ok {
                            data: ResponseData::Attached {
                                driver: granted_drive,
                            },
                        }
                    }
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Detach {
                session_id,
                client_id,
            } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                // Explicit Detach with the holder's client_id releases the lease
                // immediately (clean handoff). An empty id just tears down the
                // forwarder without touching the lease.
                match manager.send_detach(&session_id, client_id).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Heartbeat {
                session_id,
                client_id,
            } => match manager.send_heartbeat(&session_id, client_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::Promote {
                session_id,
                client_id,
            } => match manager.send_promote(&session_id, client_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::Prompt {
                session_id,
                text,
                client_id,
            } => match manager.send_prompt(&session_id, &text, client_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::AdminPrompt { session_id, text } => {
                match manager.send_admin_prompt(&session_id, &text).await {
                    Ok(()) => Response::Ok {
                        data: ResponseData::Ack,
                    },
                    Err(e) => Response::Error { message: e },
                }
            }

            Request::Cancel {
                session_id,
                client_id,
            } => match manager.send_cancel(&session_id, client_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::RestartSession {
                session_id,
                client_id,
            } => match manager.send_restart(&session_id, client_id).await {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::SetPermissionMode {
                session_id,
                mode,
                client_id,
            } => match manager
                .send_set_permission_mode(&session_id, mode, client_id)
                .await
            {
                Ok(()) => Response::Ok {
                    data: ResponseData::Ack,
                },
                Err(e) => Response::Error { message: e },
            },

            Request::CloseSession {
                session_id,
                client_id,
            } => {
                if let Some(handle) = subscribed.remove(&session_id) {
                    handle.abort();
                }
                match manager.send_close(&session_id, client_id).await {
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

    // Connection closed (socket EOF) — tear down the forwarders, but do NOT
    // release any lease. Under the phase-4 lease model the lease keys on the
    // STABLE client_id, not this ephemeral connection, and EOF deliberately
    // "starts the clock": the lease is left to expire on its TTL so a fast
    // same-client_id reconnect resumes with zero contention (the race the old
    // attach_owner_with_retry masked). A *clean* handoff uses an explicit
    // Request::Detach carrying the holder's client_id (do_detach releases now);
    // a crash/close frees the lease only when the TTL lapses (then the sweep
    // broadcasts LeaseChanged{None} so a candidate can promote). Passing the
    // empty client_id here makes send_detach a lease no-op.
    for (sid, handle) in &subscribed {
        handle.abort();
        let _ = manager.send_detach(sid, String::new()).await;
    }
    manager_events.abort();
}

/// Forward a session's notifications to one GUI connection's writer.
///
/// **Source of truth is `event_log`, not the broadcast.** The broadcast
/// channel is used only as a wake signal: on any wake (including a `Lagged`
/// overflow) we re-read `event_log[sent..]` and forward whatever the client
/// hasn't seen. This makes a slow/lagging subscriber *self-healing* — it can
/// never permanently lose transcript content the way the old "forward the
/// broadcast payload and drop on Lagged" path did (that was the source of the
/// `fingerLet`-style merge artifacts). The first tail pass (`sent == 0`) also
/// subsumes the attach-time replay, so history and live stream share one
/// ordered path with no replay/live seam.
async fn forward_notifications(
    _manager: Arc<SessionManager>,
    session_id: ServerSessionId,
    writer: Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    mut lease_rx: watch::Receiver<Option<Lease>>,
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
        // forwarder is the slowest and its backlog crossed the high-water bound.
        // Shut down the write half so the CLIENT sees a clean EOF and does a
        // from-base reconnect (NOT a silent gap) — merely returning would only
        // stop this forwarder task while the connection's read loop kept the
        // socket open (a wedged owner under App Nap would never notice). The
        // progress handle drops on return, falling out of the trim-floor `min`
        // (the actor already pruned it from `forwarders`, so the trim resumed).
        if progress.evicted.load(Ordering::Acquire) {
            tracing::warn!(
                session_id = %&session_id[..8.min(session_id.len())],
                "high-water disconnect: backlog past threshold — closing wedged forwarder's socket"
            );
            use tokio::io::AsyncWriteExt as _;
            let _ = writer.lock().await.shutdown().await;
            return false;
        }
        let offset = match snap.log.resolve_sent(*sent_seq, snap.generation) {
            sketch::event_log::CursorResolution::FromBase => 0,
            sketch::event_log::CursorResolution::Tail { vec_index } => vec_index,
        };
        let entries = snap.log.entries();
        if entries.len() > offset {
            if !flush_tail(writer, session_id, &entries[offset..]).await {
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

    // Once the control (LeaseChanged) channel closes, stop selecting on it so
    // a closed broadcast doesn't busy-loop; keep serving the transcript log.
    let mut lease_open = true;

    loop {
        tokio::select! {
            // Transcript log channel: a new snapshot was published. Tail the
            // latest snapshot lock-free from the cloned snapshot — no manager
            // lock in the hot path. Coalesced wakes self-heal: we always
            // re-resolve `sent_seq` against the latest published `log_base`.
            changed = log_rx.changed() => {
                match changed {
                    Ok(()) => {
                        let snap = log_rx.borrow_and_update().clone();
                        if !tail_snapshot(&snap, &writer, &session_id, &mut sent_seq, &progress).await {
                            return;
                        }
                    }
                    Err(_) => {
                        // Sender dropped (session closing). One final tail of
                        // the last snapshot, then exit.
                        let snap = log_rx.borrow().clone();
                        let _ = tail_snapshot(&snap, &writer, &session_id, &mut sent_seq, &progress).await;
                        return;
                    }
                }
            }

            // Control channel: lease state (watch<Option<Lease>>). On change,
            // synthesize a single `LeaseChanged` control note and forward it —
            // the only control note, never logged. The watch already holds the
            // WIRE Lease form, so no conversion happens here.
            changed = lease_rx.changed(), if lease_open => {
                match changed {
                    Ok(()) => {
                        let lease = lease_rx.borrow_and_update().clone();
                        let frame = Frame::Notification {
                            note: Notification::LeaseChanged {
                                session_id: session_id.clone(),
                                lease,
                            },
                        };
                        if let Ok(mut line) = serde_json::to_string(&frame) {
                            line.push('\n');
                            let dur = slow_sub_write_timeout();
                            let mut w = writer.lock().await;
                            match tokio::time::timeout(dur, w.write_all(line.as_bytes())).await {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => return,
                                Err(_) => {
                                    tracing::warn!(
                                        session_id = %&session_id[..8.min(session_id.len())],
                                        "slow subscriber: LeaseChanged write stalled >{}ms — disconnecting",
                                        dur.as_millis()
                                    );
                                    return;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Control channel closed: keep serving the transcript
                        // log channel until IT closes.
                        lease_open = false;
                    }
                }
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
/// falsely reaped. Override via `SKETCH_SLOW_SUB_TIMEOUT_MS` (u64 ms); `0` or
/// unset → the 60s default.
fn slow_sub_write_timeout() -> std::time::Duration {
    // Resolved once per process (env can't change mid-run) so the hot
    // streaming write path doesn't lock + parse the env on every write.
    static TIMEOUT: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        const DEFAULT_MS: u64 = 60_000;
        let ms = std::env::var("SKETCH_SLOW_SUB_TIMEOUT_MS")
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
                let client = match sketch::session_client::SessionServerClient::connect_existing() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "error: could not connect to a running session server ({e}). \
                             Start one with `sketch-session-server` (or `sketch-session-server install`)."
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
    let config = sketch::config::Config::load().unwrap_or_default();
    let default_permission_mode = config.default_permission_mode;
    tracing::info!(
        default_permission_mode = config.default_permission_mode.short_label(),
        "loaded config"
    );

    let (mgr, cmd_rx, default_permission_mode) =
        SessionManager::new_with_inlet(default_permission_mode);
    let manager = Arc::new(mgr);

    // Recover sessions from a prior run BEFORE the actor starts (recovery must
    // precede the accept loop). The seed map is moved into the actor; the resume
    // jobs spawn workers that re-spawn ACP subprocesses and post `PublishChannel`
    // back into the actor once it's running.
    let (seed_sessions, resume_jobs) = restore_seed_from_disk();

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
    ));

    // Now the actor is running, kick off the resume workers.
    for job in resume_jobs {
        spawn_resume_worker(manager.cmd_tx.clone(), job, Arc::clone(&spawner));
    }

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
        let active_sessions = manager.send_session_count().await;
        if conn_id == 1 {
            tracing::info!(
                conn_id,
                active_sessions,
                "client connected (conn {conn_id}); {active_sessions} active session(s)"
            );
        } else {
            tracing::info!(
                conn_id,
                active_sessions,
                "client reconnected (conn {conn_id}); {active_sessions} active session(s)"
            );
        }
        tokio::spawn(handle_connection(stream, mgr, conn_id));
    }
}
