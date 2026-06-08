//! In-memory event-log ringbuffer with a logical seq space (spec-event-stream
//! §6) — Phase 8, Stage B.
//!
//! The server's per-session transcript is an append-only `Vec<Notification>`.
//! Stage A made `AgentEvent` self-attributing (`seq` is a logical offset, NOT a
//! `Vec` index); Stage B bounds the IN-MEMORY `Vec` to a generous cap so a
//! long-lived session can't grow it without bound, while keeping the on-disk WAL
//! append-only / unbounded (locked decision).
//!
//! ## The one invariant that matters (spec §6 / risk #3)
//!
//! `seq` is a LOGICAL offset into the per-`(session, generation)` stream; a
//! `Vec` entry at index `i` has `seq = log_base + i`. `log_base` is the lowest
//! `seq` still resident in the in-memory `Vec` (starts `0`). When the front of
//! the `Vec` is trimmed, `log_base` advances by the number of dropped entries,
//! so the seq space is **STABLE across compaction** — a client's acked `seq`
//! keeps meaning the same logical position. The subtle bug this module exists to
//! prevent is mixing a `Vec` index and a `seq`: ALL cursor / forwarder
//! arithmetic goes through [`EventLog::seq_of`] / [`EventLog::resolve_cursor`] so
//! the `seq ↔ Vec-offset` translation lives in exactly ONE place.
//!
//! ## Back-compat with phase-5 cursor reconnect
//!
//! Before any trim, `log_base == 0`, so `seq == Vec index` and every behaviour
//! is byte-identical to the phase-5 steady state — the existing `(generation,
//! index)` attach path keeps working with `index` reinterpreted as an acked
//! `seq`. A client that fell off the trimmed tail (`acked_seq < log_base`) gets
//! a clean from-base rebuild instead of a silent gap.

use std::sync::Arc;

use crate::session_proto::Notification;

/// Default in-memory `event_log` cap (entries). Generous: a session would need a
/// great many turns of streamed chunks to reach it; the on-disk WAL is unbounded
/// regardless. Override at runtime with `SKETCH_EVENT_LOG_CAP` (a `usize`; `0`
/// or unset → this default). A tiny override is what the Stage B tests use to
/// force a trim deterministically.
pub const DEFAULT_EVENT_LOG_CAP: usize = 50_000;

/// Resolve `SKETCH_EVENT_LOG_CAP` once (env can't change mid-run). A value `< 2`
/// is clamped to `2` so there is always room for the prepended `CompactedSummary`
/// marker plus at least one surviving event.
pub fn event_log_cap() -> usize {
    use std::sync::OnceLock;
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("SKETCH_EVENT_LOG_CAP")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .map(|n| n.max(2))
            .unwrap_or(DEFAULT_EVENT_LOG_CAP)
    })
}

/// Multiplier applied to [`event_log_cap`] to derive the default high-water
/// backlog bound (spec §6). The trim floor (`min(sent_seq)` over live
/// forwarders) is a HARD ceiling — the owner is never silently gapped — so a
/// slow/paused forwarder (e.g. a backgrounded GUI owner under macOS App Nap that
/// stops draining its socket) pins the floor and lets the in-memory `Vec` grow
/// past `cap`. `HIGH_WATER = cap * K` is the backlog ceiling at which that
/// wedged forwarder is force-DISCONNECTED (a clean from-base reconnect, NOT a
/// silent in-place gap), dropping it from the floor `min` so the trim resumes.
pub const DEFAULT_HIGH_WATER_MULTIPLIER: usize = 4;

/// Resolve the high-water backlog bound once (spec §6). Override with
/// `SKETCH_EVENT_LOG_HIGH_WATER` (a `usize`; `0` or unset → [`event_log_cap`] ×
/// [`DEFAULT_HIGH_WATER_MULTIPLIER`]). A tiny override is what the high-water
/// disconnect test uses to force a wedged-forwarder eviction deterministically.
///
/// Clamped to `>= cap`: a high-water below the cap makes no sense (the cap-only
/// trim already bounds growth to `cap` when no floor pins it), so the floor /
/// high-water mechanism only ever ADDS the disconnect-before-gap behaviour on
/// top of the existing cap.
pub fn event_log_high_water() -> usize {
    use std::sync::OnceLock;
    static HW: OnceLock<usize> = OnceLock::new();
    *HW.get_or_init(|| {
        let cap = event_log_cap();
        std::env::var("SKETCH_EVENT_LOG_HIGH_WATER")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .map(|n| n.max(cap))
            .unwrap_or(cap.saturating_mul(DEFAULT_HIGH_WATER_MULTIPLIER))
    })
}

/// How a reconnect cursor `(generation, acked_seq)` resolves against the current
/// in-memory log (spec §6 epoch predicate). Always evaluated under the actor's
/// single-writer lock, so `log_base` can't advance mid-decision (no TOCTOU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorResolution {
    /// Full rebuild from the in-memory base (`Vec` index 0). The forwarder
    /// streams `[0..]`, which begins with the `CompactedSummary` marker if a
    /// trim has happened. Chosen for: no/absent cursor, a generation mismatch
    /// (a force-restart bumped the epoch), or `acked_seq < log_base` (the slow
    /// subscriber fell off the trimmed tail).
    FromBase,
    /// Incremental tail: the forwarder streams `event_log[vec_index..]`. Only
    /// when the generation matches AND `log_base <= acked_seq <= tip`.
    Tail { vec_index: usize },
}

impl CursorResolution {
    /// The forwarder's initial `sent` (a `Vec` index): `0` for a from-base
    /// rebuild, else the in-range tail offset.
    pub fn initial_vec_index(self) -> usize {
        match self {
            CursorResolution::FromBase => 0,
            CursorResolution::Tail { vec_index } => vec_index,
        }
    }
}

/// Describes a completed front-trim so the caller can splice a `CompactedSummary`
/// marker (NOT a silent drop, spec §6/§7). Returned by [`EventLog::trim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimResult {
    /// How many entries were drained off the front.
    pub dropped: usize,
    /// The new `log_base` (== old base + `dropped`) — the `seq` the prepended
    /// marker should carry so a from-base rebuild starts exactly there.
    pub new_base: u64,
    /// The highest `turn` whose events were (partly or wholly) trimmed, for the
    /// marker's `through_turn`. `None` if no dropped entry carried a turn.
    pub through_turn: Option<u64>,
}

/// Append-only, ringbuffer-bounded in-memory event log with a logical seq space.
///
/// Wraps the `Arc<Vec<Notification>>` the server already shares with the
/// forwarder watch (so `attach` clones a pointer, not the Vec) and adds the
/// `log_base` logical offset. Pushes go through `Arc::make_mut` (cheap in-place
/// mutation in the common single-reference case).
#[derive(Debug, Clone)]
pub struct EventLog {
    entries: Arc<Vec<Notification>>,
    /// Lowest `seq` still resident in `entries`. `entries[i].seq == log_base + i`
    /// for an `AgentEvent`-carrying entry. Advances by `dropped` on every trim.
    log_base: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLog {
    /// A fresh, empty log with `log_base == 0`.
    pub fn new() -> Self {
        Self { entries: Arc::new(Vec::new()), log_base: 0 }
    }

    /// Reconstruct from a recovered transcript (WAL replay). `log_base` is the
    /// `seq` of the first recovered event — on a fresh restart the durable log is
    /// a faithful append-ordered prefix starting at seq 0, so `base == 0` is the
    /// normal case. (The on-disk WAL is never trimmed, so recovery always starts
    /// from the true base.)
    pub fn from_recovered(entries: Vec<Notification>, log_base: u64) -> Self {
        Self { entries: Arc::new(entries), log_base }
    }

    /// The shared snapshot pointer to publish on the forwarder watch.
    pub fn snapshot(&self) -> Arc<Vec<Notification>> {
        Arc::clone(&self.entries)
    }

    /// The resident entries (read-only). The forwarder slices `[vec_index..]`
    /// off this after resolving its logical cursor via [`resolve_cursor`].
    pub fn entries(&self) -> &[Notification] {
        &self.entries
    }

    /// Resolve a live forwarder's logical `sent_seq` into the `Vec` offset to
    /// tail from, against the CURRENT `log_base` (Bug 1a). This is the same
    /// `seq ↔ Vec-offset` translation [`resolve_cursor`] performs at attach,
    /// reused so the live loop can't drift from the attach resolver:
    ///
    /// - `sent_seq < log_base` → the forwarder was trimmed past; it must do a
    ///   from-base rebuild (re-slice from `Vec` index 0, which now begins with the
    ///   `CompactedSummary` marker). Returns `FromBase`.
    /// - else → `Tail { vec_index: sent_seq - log_base }` (`== len` when caught up).
    ///
    /// A live forwarder always shares the session's generation (it is the same
    /// channel epoch — a generation bump forces a fresh attach), so this passes
    /// `current_gen` for both sides; the only meaningful decision here is the
    /// `log_base` comparison.
    pub fn resolve_sent(&self, sent_seq: u64, current_gen: u64) -> CursorResolution {
        self.resolve_cursor(Some((current_gen, sent_seq)), current_gen)
    }

    /// Number of entries currently resident in memory (a `Vec` length, NOT a
    /// seq). The forwarder tails `[sent..len()]`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The lowest `seq` still resident (spec §6 `log_base`).
    pub fn log_base(&self) -> u64 {
        self.log_base
    }

    /// One-past-the-last logical `seq` (== `log_base + len`). The exclusive tip
    /// of the seq space; a cursor's `acked_seq` may legitimately equal this
    /// (caught up, nothing new).
    pub fn tip_seq(&self) -> u64 {
        self.log_base + self.entries.len() as u64
    }

    /// The logical `seq` of the entry at `Vec` index `i` (spec §6:
    /// `seq = log_base + i`). The forwarder's OUTGOING cursor for an entry it
    /// just sent is `seq_of(vec_index)` — NOT the raw `Vec` index — so the client
    /// always speaks `seq` and a later trim never re-aliases its cursor.
    pub fn seq_of(&self, vec_index: usize) -> u64 {
        self.log_base + vec_index as u64
    }

    /// Append one entry. Returns the logical `seq` it was assigned (`tip` before
    /// the push). Does NOT trim — trimming is a separate, explicit step
    /// ([`trim`](Self::trim)) so the caller can splice a marker
    /// between the push and the broadcast.
    pub fn push(&mut self, note: Notification) -> u64 {
        let seq = self.tip_seq();
        Arc::make_mut(&mut self.entries).push(note);
        seq
    }

    /// Prepend a marker at the FRONT (the new base). Used to splice the
    /// `CompactedSummary` placeholder a trim produced, so a from-base rebuild
    /// surfaces "history compacted" deterministically rather than as a gap.
    ///
    /// CRITICAL (Bug 2): prepending an entry shifts every surviving entry up by
    /// one `Vec` index, which — without a compensating adjustment — would silently
    /// shift the LOGICAL seq space too (`seq = log_base + i`), violating the module
    /// invariant "seq is STABLE across compaction". A survivor whose acked seq was
    /// `S` would then report `S + 1`, mis-aligning every client cursor by one per
    /// trim (compounding). So the prepend reuses one of the just-dropped slots:
    /// `log_base` is DECREMENTED by one, so the marker takes the seq of the
    /// last-dropped entry (`new_log_base = log_base - 1`), every survivor keeps its
    /// original seq, and `tip_seq()` is unchanged. The caller must therefore stamp
    /// the marker's seq as `new_base - 1` (the decremented base), not `new_base`.
    ///
    /// Only legal immediately after a [`trim`](Self::trim) that dropped ≥ 1 entry
    /// (so there is a slot to reuse): `log_base >= 1` is required. A `debug_assert`
    /// guards it; trims that drop 0 entries return `None`, so `push_event` never
    /// reaches `prepend` without having dropped something.
    pub fn prepend(&mut self, note: Notification) {
        debug_assert!(
            self.log_base >= 1,
            "prepend reuses a dropped slot — only valid after a trim that dropped ≥ 1 entry"
        );
        Arc::make_mut(&mut self.entries).insert(0, note);
        // Reuse the last-dropped slot so survivor seqs stay stable and tip_seq
        // is unchanged: the marker now sits at seq `log_base - 1`.
        self.log_base = self.log_base.saturating_sub(1);
    }

    /// Trim the front with HYSTERESIS: only when the log exceeds the high-water
    /// `cap` does it drop down to the low-water `target` (`target <= cap`),
    /// advancing `log_base` by the number dropped. Returns a [`TrimResult`] so
    /// the caller can build + [`prepend`](Self::prepend) a `CompactedSummary`
    /// marker; `None` when no trim was needed.
    ///
    /// Hysteresis is what keeps this cheap (spec §11 / risk #2): a `Vec`
    /// front-drain + the caller's `prepend(0)` are each O(resident), so trimming
    /// ONE entry on every push at the cap would be O(cap) per push. Instead we
    /// trim a big batch occasionally (cap → target), amortising the cost — a trim
    /// touches the log roughly once per `cap - target` pushes.
    ///
    /// `floor_seq` is a HARD CEILING the trim must never cross: an entry at `Vec`
    /// index `i` (seq `log_base + i`) is dropped only while its seq is strictly
    /// below `floor_seq`. The caller passes the owner's acked `seq` (the lease
    /// holder is never gapped, spec §6) — or `u64::MAX` when no floor applies
    /// (no live owner / cap-only mode). The caller-prepended marker occupies one
    /// slot, so the effective resident count after a trim is `dropped`-adjusted
    /// `+ 1`; `target` should be chosen below `cap` with room for it.
    pub fn trim(&mut self, cap: usize, target: usize, floor_seq: u64) -> Option<TrimResult> {
        debug_assert!(target <= cap, "target low-water must be <= cap high-water");
        let len = self.entries.len();
        if len <= cap {
            return None; // under the high-water mark — leave it (hysteresis)
        }
        // Drop down to the low-water `target`.
        let want_drop = len.saturating_sub(target);
        // Respect the floor: never drop an entry whose seq >= floor_seq.
        // The entry at Vec index `i` has seq `log_base + i`; we may drop indices
        // `[0, max_droppable)` where `log_base + max_droppable <= floor_seq`.
        let max_droppable = floor_seq.saturating_sub(self.log_base).min(len as u64) as usize;
        let dropped = want_drop.min(max_droppable);
        if dropped == 0 {
            return None;
        }

        // Highest turn among the dropped entries (for the marker's through_turn).
        let through_turn = self.entries[..dropped]
            .iter()
            .filter_map(note_turn)
            .max();

        let v = Arc::make_mut(&mut self.entries);
        v.drain(..dropped);
        self.log_base += dropped as u64;

        Some(TrimResult { dropped, new_base: self.log_base, through_turn })
    }

    /// Resolve a reconnect cursor `(cursor_gen, acked_seq)` against this log
    /// under the §6 epoch predicate. `current_gen` is the session's live
    /// `channel_generation`.
    ///
    /// - no cursor → `FromBase` (today's every-client behaviour).
    /// - `cursor_gen != current_gen` → `FromBase` (force-restart bumped epoch).
    /// - `acked_seq < log_base` → `FromBase` (fell off the trimmed tail —
    ///   the slow-subscriber gap case; a clean from-base rebuild, never a gap).
    /// - `acked_seq > tip_seq` → `FromBase` (bogus / lost-unfsynced-tail).
    /// - else → `Tail { vec_index: acked_seq - log_base }`.
    pub fn resolve_cursor(
        &self,
        cursor: Option<(u64, u64)>,
        current_gen: u64,
    ) -> CursorResolution {
        let Some((cursor_gen, acked_seq)) = cursor else {
            return CursorResolution::FromBase;
        };
        if cursor_gen != current_gen {
            return CursorResolution::FromBase;
        }
        if acked_seq < self.log_base || acked_seq > self.tip_seq() {
            return CursorResolution::FromBase;
        }
        CursorResolution::Tail { vec_index: (acked_seq - self.log_base) as usize }
    }
}

/// The `turn` a notification carries, if any — used to compute a trim marker's
/// `through_turn`. Only `Agent`-wrapped events carry a turn in the new vocab; a
/// legacy `TurnEnded` carries `turn_count` (1-based settled), so subtract 1 to
/// get the completed turn index, matching the envelope convention.
fn note_turn(note: &Notification) -> Option<u64> {
    match note {
        Notification::Agent { event } => Some(event.turn),
        Notification::TurnEnded { turn_count, .. } => Some(turn_count.saturating_sub(1) as u64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_event::{AgentEvent, AgentEventKind, ChunkRole, TurnOutcome};

    fn chunk(session: &str, g: u64, turn: u64, seq: u64, text: &str) -> Notification {
        Notification::Agent {
            event: AgentEvent::new(
                session.into(),
                g,
                turn,
                seq,
                AgentEventKind::Chunk { text: text.into(), role: ChunkRole::Message },
            ),
        }
    }

    fn turn_ended(session: &str, g: u64, turn: u64, seq: u64) -> Notification {
        Notification::Agent {
            event: AgentEvent::new(
                session.into(),
                g,
                turn,
                seq,
                AgentEventKind::TurnEnded { outcome: TurnOutcome::Completed },
            ),
        }
    }

    fn marker(session: &str, g: u64, seq: u64) -> Notification {
        Notification::Agent {
            event: AgentEvent::new(
                session.into(),
                g,
                0,
                seq,
                AgentEventKind::CompactedSummary { through_turn: 0, summary: "compacted".into() },
            ),
        }
    }

    /// Bug 2: prepending the CompactedSummary marker must NOT shift the logical
    /// seq space. Before the fix, `prepend` left `log_base` untouched, so every
    /// survivor's seq inflated by one (tip_seq 11, seq_of(last)=10 on a 10-entry
    /// log trimmed by 6 then prepended) and a client that acked a survivor's seq
    /// got a one-entry dup/misalignment per trim. After the fix the marker reuses
    /// the last-dropped slot: `log_base` decrements by one, survivor seqs stay
    /// STABLE, tip_seq is unchanged by the prepend, and the marker takes the seq
    /// of the last-dropped entry.
    #[test]
    fn prepend_marker_keeps_seq_space_stable() {
        let mut log = EventLog::new();
        // 10 entries, seqs 0..10.
        for i in 0..10 {
            log.push(chunk("s", 0, i / 3, i, &format!("c{i}")));
        }
        // Record each survivor's pre-trim seq so we can prove stability.
        // Trim drops the front 6 (seqs 0..6); survivors are original seqs 6..10.
        let trim = log.trim(4, 4, u64::MAX).expect("must trim over cap");
        assert_eq!(trim.dropped, 6);
        assert_eq!(trim.new_base, 6);
        let tip_after_trim = log.tip_seq();
        assert_eq!(tip_after_trim, 10);

        // The marker reuses the last-dropped slot: its seq is `new_base - 1`.
        let marker_seq = trim.new_base - 1; // 5
        log.prepend(marker("s", 0, marker_seq));

        // log_base decremented to make room for the marker.
        assert_eq!(log.log_base(), 5, "prepend must decrement log_base by one");
        // tip_seq UNCHANGED by the prepend (the bug inflated it to 11).
        assert_eq!(
            log.tip_seq(),
            tip_after_trim,
            "prepend must not move the tip (Bug 2: was 11)"
        );
        // The marker sits at Vec index 0 with seq == new log_base.
        assert_eq!(log.seq_of(0), 5, "marker takes the last-dropped seq");
        assert_eq!(log.seq_of(0), log.log_base());

        // Each surviving event keeps its ORIGINAL seq. Vec index 1 is the
        // survivor that was originally seq 6, index 2 was 7, ... index 5 was 10.
        // (Bug 2 reported these as 7,8,9,10 — each +1.)
        for (vec_index, original_seq) in (1..=5usize).zip(6..=10u64) {
            assert_eq!(
                log.seq_of(vec_index),
                original_seq,
                "survivor at Vec index {vec_index} must keep its pre-trim seq {original_seq}"
            );
        }

        // Resolving a cursor at a survivor's acked seq yields that SAME survivor
        // (no dup). A client that acked seq 6 must tail starting at the entry
        // AFTER seq 6 — i.e. Vec index 2 (seq 7), not re-send seq 6 or 7-as-6.
        // resolve_cursor returns the offset of the acked entry; the survivor
        // whose seq is 6 lives at Vec index 1.
        assert_eq!(
            log.resolve_cursor(Some((0, 6)), 0),
            CursorResolution::Tail { vec_index: 1 },
            "acked seq 6 resolves to the survivor that still carries seq 6"
        );
        assert_eq!(
            log.resolve_cursor(Some((0, 10)), 0),
            CursorResolution::Tail { vec_index: 5 },
            "acked seq 10 (tip-1) resolves to the last survivor"
        );
    }

    /// seq == Vec index while `log_base == 0` (phase-5 steady state).
    #[test]
    fn seq_equals_vec_index_before_any_trim() {
        let mut log = EventLog::new();
        for i in 0..5 {
            let seq = log.push(chunk("s", 0, 0, i, &format!("c{i}")));
            assert_eq!(seq, i, "push returns the assigned seq == Vec index pre-trim");
        }
        assert_eq!(log.log_base(), 0);
        assert_eq!(log.len(), 5);
        for i in 0..5 {
            assert_eq!(log.seq_of(i), i as u64);
        }
        assert_eq!(log.tip_seq(), 5);
    }

    /// A trim advances log_base; seq stays STABLE (seq = log_base + Vec index).
    #[test]
    fn trim_advances_base_and_keeps_seq_stable() {
        let mut log = EventLog::new();
        // 10 entries, seqs 0..10.
        for i in 0..10 {
            log.push(chunk("s", 0, i / 3, i, &format!("c{i}")));
        }
        // Trim to cap 4 with no floor: drop the front 6 (seqs 0..6).
        let trim = log.trim(4, 4, u64::MAX).expect("must trim over cap");
        assert_eq!(trim.dropped, 6);
        assert_eq!(trim.new_base, 6);
        assert_eq!(log.log_base(), 6);
        assert_eq!(log.len(), 4);
        // The surviving entries keep their ORIGINAL seqs: Vec index 0 is seq 6.
        for i in 0..log.len() {
            assert_eq!(log.seq_of(i), 6 + i as u64);
        }
        assert_eq!(log.tip_seq(), 10);
    }

    /// A cursor whose acked_seq fell below the new base resolves to FromBase
    /// (clean rebuild) — never a silent gap. A cursor still in range tails.
    #[test]
    fn cursor_below_base_falls_back_in_range_tails() {
        let mut log = EventLog::new();
        for i in 0..10 {
            log.push(chunk("s", 0, 0, i, &format!("c{i}")));
        }
        log.trim(4, 4, u64::MAX).unwrap(); // base now 6, resident seqs 6..10

        // Below base → FromBase.
        assert_eq!(
            log.resolve_cursor(Some((0, 3)), 0),
            CursorResolution::FromBase,
            "acked_seq below log_base must rebuild from base, not gap"
        );
        // Exactly at base → tail from Vec index 0.
        assert_eq!(
            log.resolve_cursor(Some((0, 6)), 0),
            CursorResolution::Tail { vec_index: 0 }
        );
        // In range → tail at the right Vec offset.
        assert_eq!(
            log.resolve_cursor(Some((0, 8)), 0),
            CursorResolution::Tail { vec_index: 2 }
        );
        // At tip (caught up) → tail of length 0.
        assert_eq!(
            log.resolve_cursor(Some((0, 10)), 0),
            CursorResolution::Tail { vec_index: 4 }
        );
        // Past tip → FromBase (bogus / lost tail).
        assert_eq!(log.resolve_cursor(Some((0, 11)), 0), CursorResolution::FromBase);
        // Generation mismatch → FromBase.
        assert_eq!(log.resolve_cursor(Some((1, 8)), 0), CursorResolution::FromBase);
        // No cursor → FromBase.
        assert_eq!(log.resolve_cursor(None, 0), CursorResolution::FromBase);
    }

    /// Back-compat: with log_base == 0, the cursor index IS the Vec index, so the
    /// phase-5 attach path is byte-identical.
    #[test]
    fn no_trim_cursor_is_vec_index_identical_to_phase5() {
        let mut log = EventLog::new();
        for i in 0..6 {
            log.push(chunk("s", 0, 0, i, &format!("c{i}")));
        }
        for idx in 0..=6u64 {
            assert_eq!(
                log.resolve_cursor(Some((0, idx)), 0),
                CursorResolution::Tail { vec_index: idx as usize },
                "pre-trim, acked_seq == Vec index (phase-5 steady state)"
            );
        }
    }

    /// The owner floor is a HARD ceiling: never trim past it even when over cap.
    #[test]
    fn trim_respects_owner_floor() {
        let mut log = EventLog::new();
        for i in 0..10 {
            log.push(chunk("s", 0, 0, i, &format!("c{i}")));
        }
        // Want to trim to cap 2 (drop 8), but the owner has only acked through
        // seq 3 — so we may drop at most seqs [0,3), i.e. 3 entries.
        let trim = log.trim(2, 2, 3).expect("some trim under the floor");
        assert_eq!(trim.dropped, 3, "floor caps the drop at the owner's acked_seq");
        assert_eq!(log.log_base(), 3);
        assert_eq!(log.len(), 7);
        // The owner's seq 3 is still resident (Vec index 0).
        assert_eq!(log.seq_of(0), 3);
        // A second trim with the floor at the owner's UNCHANGED ack does nothing.
        assert_eq!(log.trim(2, 2, 3), None, "floor blocks further trim");
    }

    /// through_turn is the highest turn among dropped entries.
    #[test]
    fn trim_reports_through_turn() {
        let mut log = EventLog::new();
        // seqs 0..6 across turns 0,0,1,1,2,2.
        log.push(chunk("s", 0, 0, 0, "a"));
        log.push(turn_ended("s", 0, 0, 1));
        log.push(chunk("s", 0, 1, 2, "b"));
        log.push(turn_ended("s", 0, 1, 3));
        log.push(chunk("s", 0, 2, 4, "c"));
        log.push(turn_ended("s", 0, 2, 5));
        // Drop the front 4 (turns 0,0,1,1) → through_turn 1.
        let trim = log.trim(2, 2, u64::MAX).unwrap();
        assert_eq!(trim.dropped, 4);
        assert_eq!(trim.through_turn, Some(1));
    }

    /// No trim when at/under cap.
    #[test]
    fn no_trim_under_cap() {
        let mut log = EventLog::new();
        for i in 0..4 {
            log.push(chunk("s", 0, 0, i, "x"));
        }
        assert_eq!(log.trim(4, 4, u64::MAX), None);
        assert_eq!(log.trim(10, 10, u64::MAX), None);
    }

    /// HYSTERESIS: only crossing the high-water `cap` triggers a trim, and it
    /// drops down to the low-water `target` (not to `cap`), so trims are
    /// amortised across `cap - target` pushes rather than one-per-push at the cap.
    #[test]
    fn trim_hysteresis_drops_to_target() {
        let mut log = EventLog::new();
        for i in 0..12 {
            log.push(chunk("s", 0, 0, i, "x"));
        }
        // cap 10 high-water, target 6 low-water. len 12 > 10 → drop to 6.
        let trim = log.trim(10, 6, u64::MAX).expect("over high-water must trim");
        assert_eq!(trim.dropped, 6, "trim down to the low-water target, not the cap");
        assert_eq!(log.len(), 6);
        assert_eq!(log.log_base(), 6);
        // Now resident len 6 == target. The next several pushes (up to the cap)
        // do NOT trim — hysteresis amortises the O(resident) drain.
        for i in 12..16 {
            log.push(chunk("s", 0, 0, i, "x")); // len 7..10
            assert_eq!(
                log.trim(10, 6, u64::MAX),
                None,
                "len {} is between target and cap — no trim (hysteresis)",
                log.len()
            );
        }
        // The 11th resident entry crosses the cap again.
        log.push(chunk("s", 0, 0, 16, "x")); // len 11 > cap 10
        assert!(log.trim(10, 6, u64::MAX).is_some(), "crossing cap again trims");
    }
}
