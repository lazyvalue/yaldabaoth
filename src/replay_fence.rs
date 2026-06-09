//! Replay-fence state machine for consumers of a resumed agent channel that
//! already hold the session's history.
//!
//! A resumed channel (`session/load`) re-emits the entire prior conversation
//! as a notification burst before any live event. A consumer that already has
//! that history — the session-server's recovered/in-memory `event_log` — must
//! discard the burst, or every resume duplicates the transcript. The worker
//! marks the end of the burst with [`ReplyEvent::ReplayComplete`], sent
//! strictly after the last replayed event and strictly before any live one,
//! on EVERY spawn that *attempted* a resume — including when `session/load`
//! failed or timed out and the worker fell back to `session/new`.
//!
//! The fence is therefore **marker-based**: suppress until `ReplayComplete`,
//! then deliver from the marker (inclusive) onward. It must NOT be keyed on
//! the channel's turn counter: that counter restarts at 0 on every spawn and
//! never moves during replay (commit 092c218 replaced the post-load bump with
//! the marker), so a turn-count fence never clears — the "resume hangs" bug,
//! where every post-resume event, replayed *and live*, was silently discarded
//! while the agent kept working invisibly.
//!
//! This lives in the library (not inline in the session-server pump) so the
//! pump and its tests share one implementation — the same source-of-truth
//! discipline as the viewport wrap math.

use crate::acp_channel::ReplyEvent;

/// What the pump should do with one drained batch while the fence is up.
/// Returned by [`ReplayFence::on_batch`]; `None` means the fence is down and
/// the batch flows through untouched.
#[derive(Debug, PartialEq, Eq)]
pub enum FenceAction {
    /// Still inside the replay burst: discard the whole batch.
    Discard,
    /// The end-of-replay marker is at `marker_index`. Discard everything
    /// before it; deliver the marker itself (consumers map it to the durable
    /// `ReplayEnd` the GUI finalizes on) and everything after it. The fence
    /// is now down.
    ClearAtMarker { marker_index: usize },
    /// A LIVE turn completed while the fence was still up — the marker was
    /// lost. This is a defensive valve, not an expected path (the worker
    /// emits the marker on every resume attempt): unwedge by delivering the
    /// whole batch. The turn's earlier chunks were already discarded.
    ForceClear,
}

/// See the module docs. Construct with `armed = true` only when a resume was
/// attempted AND the consumer already holds the history the replay will
/// duplicate; an unarmed fence passes everything through.
#[derive(Debug)]
pub struct ReplayFence {
    up: bool,
}

impl ReplayFence {
    pub fn new(armed: bool) -> Self {
        Self { up: armed }
    }

    pub fn is_up(&self) -> bool {
        self.up
    }

    /// Classify one drained batch. `live_turn_ended` is the pump's
    /// turn-boundary inference for this cycle (`turn_count()` climbed) — it
    /// can only be true for a *live* turn, since replay never moves the
    /// channel's turn counter.
    pub fn on_batch(
        &mut self,
        events: &[ReplyEvent],
        live_turn_ended: bool,
    ) -> Option<FenceAction> {
        if !self.up {
            return None;
        }
        if let Some(marker_index) = events
            .iter()
            .position(|e| matches!(e, ReplyEvent::ReplayComplete))
        {
            self.up = false;
            return Some(FenceAction::ClearAtMarker { marker_index });
        }
        if live_turn_ended {
            self.up = false;
            return Some(FenceAction::ForceClear);
        }
        Some(FenceAction::Discard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(s: &str) -> ReplyEvent {
        ReplyEvent::Chunk(s.to_string())
    }

    #[test]
    fn unarmed_fence_passes_everything_through() {
        let mut fence = ReplayFence::new(false);
        assert!(!fence.is_up());
        assert_eq!(fence.on_batch(&[chunk("live")], false), None);
        assert_eq!(fence.on_batch(&[ReplyEvent::ReplayComplete], false), None);
        assert_eq!(fence.on_batch(&[], true), None);
    }

    #[test]
    fn armed_fence_discards_replay_then_clears_at_marker() {
        let mut fence = ReplayFence::new(true);
        // Replay burst arrives over several drain cycles: all discarded.
        assert_eq!(
            fence.on_batch(&[chunk("replayed-1"), chunk("replayed-2")], false),
            Some(FenceAction::Discard)
        );
        assert_eq!(fence.on_batch(&[], false), Some(FenceAction::Discard));
        assert!(fence.is_up());
        // Marker mid-batch: pre-marker events are replay, the rest is live.
        assert_eq!(
            fence.on_batch(
                &[
                    chunk("replayed-3"),
                    ReplyEvent::ReplayComplete,
                    chunk("live-1"),
                ],
                false,
            ),
            Some(FenceAction::ClearAtMarker { marker_index: 1 })
        );
        assert!(!fence.is_up());
        // Down for good: subsequent batches flow untouched.
        assert_eq!(fence.on_batch(&[chunk("live-2")], false), None);
    }

    #[test]
    fn marker_first_in_batch_discards_nothing() {
        let mut fence = ReplayFence::new(true);
        assert_eq!(
            fence.on_batch(&[ReplyEvent::ReplayComplete, chunk("live-1")], false),
            Some(FenceAction::ClearAtMarker { marker_index: 0 })
        );
    }

    /// The regression shape of the resume-hang bug: a fence keyed on turn
    /// counts never cleared, so a live turn's events were discarded forever.
    /// The marker-based fence must instead force-clear the moment a live turn
    /// boundary proves the marker was lost.
    #[test]
    fn live_turn_end_without_marker_force_clears() {
        let mut fence = ReplayFence::new(true);
        assert_eq!(
            fence.on_batch(&[chunk("live-tail")], true),
            Some(FenceAction::ForceClear)
        );
        assert!(!fence.is_up());
        assert_eq!(fence.on_batch(&[chunk("live-2")], false), None);
    }

    /// A marker in the same batch as a live turn end clears AT the marker —
    /// the marker wins over the force-clear valve, so replayed duplicates
    /// before it are still discarded.
    #[test]
    fn marker_wins_over_turn_end_in_same_batch() {
        let mut fence = ReplayFence::new(true);
        assert_eq!(
            fence.on_batch(
                &[chunk("replayed"), ReplyEvent::ReplayComplete, chunk("live")],
                true,
            ),
            Some(FenceAction::ClearAtMarker { marker_index: 1 })
        );
    }
}
