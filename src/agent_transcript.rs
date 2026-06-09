//! Pure, GPUI-free reconciliation for user turns in the agent transcript.
//!
//! ## Why this module exists
//!
//! A single logical user turn can be announced to the GUI from **three**
//! independent sites:
//!
//! 1. the optimistic local echo when the user presses submit
//!    (`submit_chatbox`),
//! 2. the session server's `UserPrompt` notification (the prompt is logged to
//!    the server `event_log` and tailed back to every subscriber, including
//!    the one that sent it — and replayed verbatim on re-attach), and
//! 3. the agent's own `UserMessageChunk`, surfaced as
//!    `ReplyEvent::UserMessage` (emitted unconditionally by the worker —
//!    `acp_channel.rs` — both live and on `session/load` replay).
//!
//! Historically these were de-duplicated by a *content + position* heuristic
//! (`document_trimmed_end_ends_with`: "skip if the transcript currently ends
//! with this text"). That check is **order-dependent**: the instant anything
//! lands between the local echo and the echoed/replayed copy — an assistant
//! chunk that streams first, a tool line, a system notice, a second turn — the
//! suffix no longer matches and the turn is inserted a **second** time. That
//! is the "double-rendered input" bug.
//!
//! This module replaces the heuristic with reconciliation by **origin +
//! identity**, which is order-independent. It is deliberately pure (no `cx`,
//! no GPUI, no I/O) so the rules — the part that kept regressing — are
//! unit-/permutation-testable in isolation. The GUI is a thin adapter:
//! translate each announcement into [`UserTurnOrigin`], ask the reconciler for
//! a [`UserTurnAction`], and apply it.
//!
//! See also [`crate::acp_channel::ReplayTurns`], the sibling pure state
//! machine that owns the turn-number (`k`) attribution this module's
//! `advance_boundary` flag feeds into.

use std::collections::VecDeque;

/// Hard cap on the optimistic-echo backlog. An agent that never echoes a
/// submitted prompt (some configurations don't emit `UserMessageChunk`) would
/// otherwise grow this without bound; dropping the oldest entry keeps it O(1)
/// and bounded while still suppressing the common case (echo arrives within a
/// turn or two of the submit). 64 is far above any real in-flight depth.
const MAX_PENDING_ECHOES: usize = 64;

/// Where a user-turn insertion request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTurnOrigin {
    /// The local user pressed submit. **Always** a new turn — even if its text
    /// is identical to a previous one — so it is never suppressed.
    LocalSubmit,
    /// An echo observed on the event/replay stream: the server's `UserPrompt`
    /// or the agent's `UserMessageChunk`. May be a live echo of our own
    /// submit (suppress), a duplicate second source for a turn we just
    /// inserted (suppress), or a genuine replayed/foreign turn (insert).
    Echo,
}

/// What the GUI should do with a user-turn insertion request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTurnAction {
    /// Insert the turn. `advance_boundary` is `true` only for a genuinely
    /// **replayed** boundary on the direct-channel path, where no `TurnEnded`
    /// will bump the live counter and the [`crate::acp_channel::ReplayTurns`]
    /// cursor must be stepped explicitly. It is `false` for every live
    /// insertion (where `k = ReplayTurns::current_turn()` is correct), so a
    /// live turn can never wrongly drive the turn machine into replay mode.
    Insert { advance_boundary: bool },
    /// Skip — this content was already inserted, either as our own optimistic
    /// echo or as the first of two sources (`UserPrompt` + `UserMessage`) for
    /// the same replayed turn.
    Skip,
}

/// Canonical form for comparing a submitted prompt against its stream echo.
///
/// Both the optimistic echo (from the chatbox text) and the stream echo (from
/// the protocol) are normalized through this one function before comparison,
/// so trailing-newline / trailing-whitespace differences between the two
/// sources can never cause a spurious miss (a miss is what re-introduces the
/// double-render). It is intentionally **not** compared against the rendered
/// rope — comparing the two *source strings* directly is what makes the
/// decision independent of what else is in the transcript.
pub fn normalize_user_text(s: &str) -> String {
    s.trim_end().to_string()
}

/// Order-independent reconciler for the three user-turn announcement sites.
///
/// Invariants it upholds (exercised by the permutation tests):
/// - every [`UserTurnOrigin::LocalSubmit`] yields exactly one `Insert`;
/// - a live `Echo` of a just-submitted prompt yields `Skip` regardless of how
///   much assistant/tool content streamed in between;
/// - two sources for the same replayed turn yield one `Insert` + one `Skip`;
/// - a `Skip` never advances the turn boundary (so the turn counter is never
///   corrupted by a suppressed echo).
#[derive(Debug, Default, Clone)]
pub struct UserTurnReconciler {
    /// Normalized contents the local user submitted, awaiting suppression of
    /// their stream echo. FIFO so N identical rapid submits are matched by N
    /// echoes. Bounded by [`MAX_PENDING_ECHOES`].
    pending_local: VecDeque<String>,
    /// Normalized content of the most-recently *inserted* user turn, cleared
    /// when assistant content (a chunk/tool call) or a turn boundary follows.
    /// Suppresses a duplicate second source for the *same* turn (the server's
    /// `UserPrompt` and the agent's `UserMessageChunk` can both appear for one
    /// turn) without suppressing a legitimately-distinct later turn.
    last_inserted: Option<String>,
}

impl UserTurnReconciler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide what to do with a user-turn insertion request. `replaying` is
    /// `true` only while a direct-channel `session/load` replay burst is
    /// active; it is forwarded verbatim into `Insert { advance_boundary }` so
    /// only genuine replayed boundaries step the turn cursor.
    pub fn reconcile(
        &mut self,
        origin: UserTurnOrigin,
        text: &str,
        replaying: bool,
    ) -> UserTurnAction {
        let n = normalize_user_text(text);
        match origin {
            UserTurnOrigin::LocalSubmit => {
                // A local submit is always a new turn. Remember it so the
                // stream echo that follows (in any order relative to streamed
                // assistant content) is suppressed.
                if self.pending_local.len() >= MAX_PENDING_ECHOES {
                    self.pending_local.pop_front();
                }
                self.pending_local.push_back(n.clone());
                self.last_inserted = Some(n);
                // Live: k = current_turn(); never advance the replay cursor.
                UserTurnAction::Insert {
                    advance_boundary: false,
                }
            }
            UserTurnOrigin::Echo => {
                // 1. Live echo of our own optimistic submit? Match by content
                //    against the pending queue (NOT by transcript position), so
                //    intervening assistant/tool content is irrelevant. This is
                //    the case that fixes the live double-render; it applies on
                //    every path.
                if let Some(pos) = self.pending_local.iter().position(|p| *p == n) {
                    self.pending_local.remove(pos);
                    self.last_inserted = Some(n);
                    return UserTurnAction::Skip;
                }
                // 2. Second *source* for the turn we just inserted — the server
                //    `event_log` can carry both a `UserPrompt` and the agent's
                //    `UserMessageChunk` for one turn. Gated on `!replaying`
                //    (the server/live path, which never advances the boundary):
                //    the direct-channel replay path has only one source per
                //    turn, so two identical *consecutive* replayed turns there
                //    must BOTH insert. `last_inserted` persists for the whole
                //    turn (cleared on the boundary via `note_turn_progressed`,
                //    NOT on a chunk), so this dedup is robust to the echo
                //    arriving before or after the assistant's response.
                if !replaying && self.last_inserted.as_deref() == Some(n.as_str()) {
                    return UserTurnAction::Skip;
                }
                // 3. A genuinely new replayed/foreign turn.
                self.last_inserted = Some(n);
                UserTurnAction::Insert {
                    advance_boundary: replaying,
                }
            }
        }
    }

    /// A turn boundary (`TurnEnded` / `ReplayComplete`) closed the current
    /// user turn. Clearing `last_inserted` lets the next turn's echo — even
    /// with identical text — be recognised as a genuinely new turn (case 3)
    /// rather than a duplicate second source for the previous one (case 2).
    ///
    /// Call this on the turn **boundary**, NOT on every assistant chunk:
    /// clearing on a chunk would make case 2 fragile to whether the agent
    /// echoes the prompt before or after it starts streaming its response. The
    /// `pending_local` queue is deliberately not cleared here — an echo can
    /// legitimately arrive a turn or two after its submit, and it's bounded.
    pub fn note_turn_progressed(&mut self) {
        self.last_inserted = None;
    }

    /// Wipe all reconciliation state. Called when the transcript itself is
    /// wiped (reconnect replay via `reset_for_replay`, or an explicit clear):
    /// the authoritative `event_log` will rebuild every turn from scratch, so
    /// nothing is "pending local" any more. The clear MUST happen-before any
    /// replayed echo is processed (both run on the single foreground thread,
    /// and replay notes can only arrive after re-attach — see the seam test).
    pub fn reset(&mut self) {
        self.pending_local.clear();
        self.last_inserted = None;
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.pending_local.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_channel::ReplayTurns;

    // ---- A tiny pure transcript model used to assert end-state invariants
    // over randomized event interleavings. It records, per applied event, the
    // (k, text) of any user turn actually inserted plus a marker for assistant
    // chunks, so we can assert "each submit appears exactly once" and "k is
    // monotonic + contiguous" without any GPUI/editor machinery.

    #[derive(Debug, Clone, PartialEq)]
    enum Row {
        User { k: usize, text: String },
        Assistant { k: usize },
    }

    /// Drives the reconciler + ReplayTurns exactly as the GUI adapter does,
    /// so the test harness can't drift from production semantics.
    #[derive(Default)]
    struct Model {
        rec: UserTurnReconciler,
        rt: ReplayTurns,
        replaying: bool,
        rows: Vec<Row>,
    }

    impl Model {
        fn user(&mut self, origin: UserTurnOrigin, text: &str) {
            match self.rec.reconcile(origin, text, self.replaying) {
                UserTurnAction::Skip => {}
                UserTurnAction::Insert { advance_boundary } => {
                    let k = if advance_boundary {
                        self.rt.advance_user_boundary()
                    } else {
                        self.rt.current_turn()
                    };
                    self.rows.push(Row::User {
                        k,
                        text: text.trim_end().to_string(),
                    });
                }
            }
        }
        fn chunk(&mut self) {
            // NB: a chunk does NOT clear the reconciler's last-inserted — only
            // a turn boundary does (mirrors production: note_turn_progressed is
            // called on TurnEnded/ReplayComplete, not on Chunk).
            let k = self.rt.current_turn();
            self.rows.push(Row::Assistant { k });
        }
        /// Live turn settled (server `TurnEnded` / direct prompt-response).
        fn turn_ended(&mut self, count: usize) {
            self.rt.last_seen = count;
            self.rec.note_turn_progressed();
        }
        fn replay_complete(&mut self) {
            self.rt.finish_replay();
            self.replaying = false;
            self.rec.note_turn_progressed();
        }
    }

    fn user_rows(m: &Model) -> Vec<&Row> {
        m.rows
            .iter()
            .filter(|r| matches!(r, Row::User { .. }))
            .collect()
    }

    // ---------- the original bug: echo after assistant chunk ----------

    #[test]
    fn live_echo_after_chunk_is_suppressed() {
        // submit -> assistant streams FIRST -> echo arrives. The old suffix
        // check double-inserted here; the reconciler must Skip.
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "hello agent");
        m.chunk(); // assistant content lands before the echo
        m.user(UserTurnOrigin::Echo, "hello agent\n"); // newline-normalized
        let users = user_rows(&m);
        assert_eq!(users.len(), 1, "user turn must appear exactly once");
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "hello agent".into()
            }
        );
    }

    #[test]
    fn live_echo_before_chunk_is_suppressed() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "hi");
        m.user(UserTurnOrigin::Echo, "hi");
        m.chunk();
        assert_eq!(user_rows(&m).len(), 1);
    }

    #[test]
    fn echo_never_arrives_is_fine() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "no echo agent");
        m.chunk();
        m.turn_ended(1);
        assert_eq!(user_rows(&m).len(), 1);
        // A stale pending entry lingers harmlessly; bounded, never poisons a
        // *distinct* later submit because LocalSubmit always inserts.
    }

    // ---------- rapid duplicate submits ----------

    #[test]
    fn two_identical_rapid_submits_both_appear() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "ping");
        m.user(UserTurnOrigin::LocalSubmit, "ping");
        // both echoes arrive later, in order
        m.user(UserTurnOrigin::Echo, "ping");
        m.user(UserTurnOrigin::Echo, "ping");
        assert_eq!(user_rows(&m).len(), 2, "two distinct submits => two turns");
    }

    #[test]
    fn stale_pending_does_not_poison_later_identical_submit() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "again"); // echo never comes
        m.chunk();
        m.turn_ended(1);
        m.user(UserTurnOrigin::LocalSubmit, "again"); // distinct later turn
        let users = user_rows(&m);
        assert_eq!(users.len(), 2);
        // Even though a stale "again" sits in pending, the second LocalSubmit
        // is an unconditional insert, so the real turn is never dropped.
    }

    // ---------- the live-submit-twice counter-leak guard (adversary M2) ----

    #[test]
    fn live_submits_never_enter_replay_mode() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "a");
        m.chunk();
        m.turn_ended(1);
        m.user(UserTurnOrigin::LocalSubmit, "b");
        m.chunk();
        m.turn_ended(2);
        assert_eq!(m.rt.replay_turn, 0, "live path must never set replay_turn");
        let users = user_rows(&m);
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "a".into()
            }
        );
        assert_eq!(
            users[1],
            &Row::User {
                k: 2,
                text: "b".into()
            }
        );
    }

    #[test]
    fn echo_that_misses_queue_on_live_does_not_advance_boundary() {
        // A live echo that fails to match the pending queue (simulated by a
        // wholly different text) is treated as a foreign/new live turn and is
        // inserted with advance_boundary=false (replaying=false), so it can
        // NOT drive the live counter into replay mode.
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "submitted");
        m.chunk();
        m.user(UserTurnOrigin::Echo, "totally different live echo");
        assert_eq!(
            m.rt.replay_turn, 0,
            "live insert must not enter replay mode"
        );
    }

    // ---------- server replay: UserPrompt + UserMessage for one turn -------

    #[test]
    fn server_replay_dual_source_single_turn_not_doubled() {
        // On the server path the event_log can hold BOTH a UserPrompt and an
        // agent UserMessage echo for the same turn. Replaying=false on the
        // server path (boundaries come via replayed TurnEnded).
        let mut m = Model::default();
        m.user(UserTurnOrigin::Echo, "q1"); // UserPrompt
        m.user(UserTurnOrigin::Echo, "q1"); // UserMessage, same turn
        m.chunk();
        m.turn_ended(1);
        m.user(UserTurnOrigin::Echo, "q2");
        m.chunk();
        m.turn_ended(2);
        let users = user_rows(&m);
        assert_eq!(users.len(), 2);
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "q1".into()
            }
        );
        assert_eq!(
            users[1],
            &Row::User {
                k: 2,
                text: "q2".into()
            }
        );
    }

    #[test]
    fn server_replay_dual_source_echo_after_chunk_not_doubled() {
        // The robustness case: on the server path the agent's UserMessage echo
        // can arrive AFTER an assistant chunk for the same turn. Because
        // last_inserted is cleared on the BOUNDARY (not on the chunk), the
        // second source is still suppressed.
        let mut m = Model::default();
        m.user(UserTurnOrigin::Echo, "q1"); // UserPrompt
        m.chunk(); // assistant streams before the echo
        m.user(UserTurnOrigin::Echo, "q1"); // UserMessage echo, same turn, after chunk
        m.turn_ended(1);
        assert_eq!(
            user_rows(&m).len(),
            1,
            "dual source must not double even after a chunk"
        );
    }

    #[test]
    fn worksheet_multiline_local_submit_echo_suppressed() {
        // A worksheet submit joins its non-blank editable lines with '\n' and
        // registers that JOINED body as a LocalSubmit (commit_worksheet_turn).
        // The server then echoes the same multi-line body as a UserPrompt, and
        // the agent may also echo it as a UserMessage — both must be suppressed
        // so the in-place worksheet lines aren't re-rendered as an appended
        // turn. `normalize_user_text` only trim_end()s, so internal newlines are
        // preserved: this pins the join-vs-echo equivalence at the reconciler
        // layer (the GUI seam can only cover the single-line case headlessly).
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "first line\nsecond line");
        m.chunk(); // assistant streams BEFORE the echo (the hard ordering)
        m.user(UserTurnOrigin::Echo, "first line\nsecond line"); // server UserPrompt
        m.user(UserTurnOrigin::Echo, "first line\nsecond line"); // agent UserMessage, same turn
        let users = user_rows(&m);
        assert_eq!(
            users.len(),
            1,
            "a multi-line worksheet submit must render exactly once"
        );
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "first line\nsecond line".into()
            }
        );
    }

    #[test]
    fn direct_replay_identical_consecutive_turns_both_appear() {
        // On the direct-channel replay path each UserMessage is its own turn
        // (single source), so two identical consecutive replayed turns must
        // BOTH insert — case-2 dedup is gated off when advancing the boundary.
        let mut m = Model {
            replaying: true,
            ..Default::default()
        };
        m.user(UserTurnOrigin::Echo, "same");
        m.chunk();
        m.user(UserTurnOrigin::Echo, "same");
        m.chunk();
        m.replay_complete();
        let users = user_rows(&m);
        assert_eq!(
            users.len(),
            2,
            "identical consecutive direct-replay turns must both appear"
        );
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "same".into()
            }
        );
        assert_eq!(
            users[1],
            &Row::User {
                k: 2,
                text: "same".into()
            }
        );
    }

    // ---------- direct-channel replay advances the boundary ---------------

    #[test]
    fn direct_replay_advances_boundary_per_user_message() {
        // replaying: session/load burst active
        let mut m = Model {
            replaying: true,
            ..Default::default()
        };
        m.user(UserTurnOrigin::Echo, "u1");
        m.chunk();
        m.user(UserTurnOrigin::Echo, "u2");
        m.chunk();
        m.replay_complete();
        let users = user_rows(&m);
        assert_eq!(users.len(), 2);
        assert_eq!(
            users[0],
            &Row::User {
                k: 1,
                text: "u1".into()
            }
        );
        assert_eq!(
            users[1],
            &Row::User {
                k: 2,
                text: "u2".into()
            }
        );
        // After ReplayComplete the live counter is reconciled and we leave
        // replay mode, so the next live submit continues from k=3.
        assert_eq!(m.rt.replay_turn, 0);
        m.user(UserTurnOrigin::LocalSubmit, "u3");
        assert_eq!(
            user_rows(&m).last().unwrap(),
            &&Row::User {
                k: 3,
                text: "u3".into()
            }
        );
    }

    // ---------- reset clears state before a fresh replay ------------------

    #[test]
    fn reset_drops_pending_then_replay_rebuilds() {
        let mut m = Model::default();
        m.user(UserTurnOrigin::LocalSubmit, "mid-turn"); // never echoed
        // disconnect -> reset wipes editor + reconciler
        m.rec.reset();
        m.rt = ReplayTurns::default();
        m.rows.clear();
        // server replay rebuilds from the log
        m.user(UserTurnOrigin::Echo, "mid-turn");
        m.chunk();
        m.turn_ended(1);
        let users = user_rows(&m);
        assert_eq!(users.len(), 1, "replay reinserts cleanly after reset");
        assert_eq!(m.rec.pending_len(), 0);
    }

    // ---------- permutation fuzz: every interleaving of echo vs chunk ------

    #[test]
    fn permutation_echo_chunk_orderings_single_user_turn() {
        // For a single live turn, the echo can arrive at any point relative to
        // a run of assistant chunks. In EVERY ordering the user turn must
        // appear exactly once and the counter must stay live (replay_turn==0).
        for chunks_before in 0..6usize {
            for chunks_after in 0..6usize {
                let mut m = Model::default();
                m.user(UserTurnOrigin::LocalSubmit, "permute");
                for _ in 0..chunks_before {
                    m.chunk();
                }
                m.user(UserTurnOrigin::Echo, "permute\n");
                for _ in 0..chunks_after {
                    m.chunk();
                }
                m.turn_ended(1);
                assert_eq!(
                    user_rows(&m).len(),
                    1,
                    "doubled at chunks_before={chunks_before} chunks_after={chunks_after}"
                );
                assert_eq!(m.rt.replay_turn, 0);
            }
        }
    }

    #[test]
    fn permutation_multi_turn_live_session_no_doubles() {
        // N live turns, each with a random-ish split of chunks around the echo.
        // Assert: N user rows, k strictly 1..=N contiguous, replay mode never
        // entered. Deterministic pseudo-pattern keeps the test reproducible.
        let n = 7usize;
        let mut m = Model::default();
        for turn in 1..=n {
            m.user(UserTurnOrigin::LocalSubmit, &format!("turn {turn}"));
            let split = turn % 3; // vary echo position
            for _ in 0..split {
                m.chunk();
            }
            m.user(UserTurnOrigin::Echo, &format!("turn {turn}"));
            for _ in 0..(2 - (turn % 2)) {
                m.chunk();
            }
            m.turn_ended(turn);
        }
        let users = user_rows(&m);
        assert_eq!(users.len(), n);
        for (i, r) in users.iter().enumerate() {
            match r {
                Row::User { k, .. } => assert_eq!(*k, i + 1, "k must be contiguous"),
                _ => unreachable!(),
            }
        }
        assert_eq!(m.rt.replay_turn, 0);
    }
}
