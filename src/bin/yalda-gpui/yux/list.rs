//! `ScrollAnchoredList` — a virtualized `gpui::list` whose item set is kept in
//! sync by **splicing the minimal changed range**, never `reset()`.
//!
//! `ListState::reset()` nulls `logical_scroll_top` AND marks every row
//! unmeasured; a `scroll_to_reveal_item` issued in the same render frame then
//! computes the caret position against zero-height rows and snaps the viewport
//! to item 0 — the "view jumps to the top of the file on every newline" class of
//! bug. Splicing only the changed range preserves the scroll anchor (gpui shifts
//! `logical_scroll_top` by the edit) and keeps unchanged rows measured, so the
//! reveal lands correctly.
//!
//! Every variable-height scroll surface (the raw Edit view, the rendered Doc
//! view, the compose box) owns one of these instead of re-deriving the
//! `(ListState, last-synced-items, version)` bookkeeping and the splice. The
//! agent transcript keeps its own `TranscriptScroll` — it reconciles by item
//! COUNT (with a streaming tail-invalidation + follow-output semantics), not by
//! a content diff, so it doesn't fit this shape.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{ListAlignment, ListState, Pixels};

/// Splice a `gpui::ListState` from `old` items to `new` items by replacing only
/// the minimal changed range (shared prefix + shared suffix trimmed), rather
/// than `reset()`-ing the whole list. Preserving the unchanged head/tail keeps
/// their height measurements and lets gpui re-anchor `logical_scroll_top` across
/// the edit, so the viewport doesn't jump. Free function (not a method) so it
/// stays unit-testable against a bare `ListState`.
pub(crate) fn splice_list_to_items<T: PartialEq>(list: &ListState, old: &[T], new: &[T]) {
    let max_pre = old.len().min(new.len());
    let mut pre = 0;
    while pre < max_pre && old[pre] == new[pre] {
        pre += 1;
    }
    let max_suf = max_pre - pre;
    let mut suf = 0;
    while suf < max_suf && old[old.len() - 1 - suf] == new[new.len() - 1 - suf] {
        suf += 1;
    }
    let old_changed = pre..(old.len() - suf);
    let new_len = new.len() - suf - pre;
    // Nothing structurally changed (identical content) ⇒ leave the list alone.
    if old_changed.is_empty() && new_len == 0 {
        return;
    }
    list.splice(old_changed, new_len);
}

/// The top visible line that keeps the caret in view, with MINIMAL scrolling —
/// a text-editor-style window over uniform-height rows.
///
/// The compose box renders **non-wrapping, uniform-height** rows, so "which
/// line sits at the top so the caret is visible" is exact integer arithmetic —
/// it needs ZERO height measurement. This is the architectural fix for the
/// recurring "caret scrolls off-screen in the chatbox" bug: GPUI's
/// `scroll_to_reveal_item` derives the offset from cached/estimated row heights,
/// and freshly-spliced rows are unmeasured (they fall back to the list's default
/// item height) — so the reveal lands at the wrong offset and strands the caret.
/// Anchoring the top item by this function instead makes caret-visibility hold
/// **by construction**, independent of GPUI's measurement timing.
///
/// `prev_top` is the line currently at the top of the window (read back from the
/// list's own scroll anchor, so the window only moves when the caret would
/// otherwise leave it). The result is clamped so the window never scrolls past
/// the end into blank space, while still always containing `cursor_line`.
pub(crate) fn compose_first_visible_line(
    cursor_line: usize,
    prev_top: usize,
    line_count: usize,
    visible: usize,
) -> usize {
    let visible = visible.max(1);
    let max_top = line_count.saturating_sub(visible);
    let prev_top = prev_top.min(max_top);
    let first = if cursor_line < prev_top {
        // Caret above the window → scroll up so it's the top line.
        cursor_line
    } else if cursor_line >= prev_top + visible {
        // Caret below the window → scroll down so it's the bottom line.
        cursor_line + 1 - visible
    } else {
        // Already visible → don't move (stable, minimal scroll).
        prev_top
    };
    first.min(max_top)
}

/// A virtualized list that re-syncs to a new item sequence by splicing the
/// changed range (see [`splice_list_to_items`]) so scroll stays anchored across
/// edits. One per scrollable surface.
///
/// All methods take `&self`: `ListState` is already interior-mutable, and the
/// sync bookkeeping is `Cell`/`RefCell`, so a surface whose render borrows it
/// immutably (the Doc view renders through `&DocState`) reconciles without a
/// `&mut`.
pub(crate) struct ScrollAnchoredList<T> {
    state: ListState,
    /// The items the list was last reconciled against (the prefix/suffix diff
    /// baseline) and the caller's content version. The version gate makes an
    /// idle frame (no edit) a no-op — no re-diff. `u64::MAX` = never synced.
    synced: RefCell<Rc<Vec<T>>>,
    synced_seq: Cell<u64>,
}

impl<T: PartialEq> ScrollAnchoredList<T> {
    pub(crate) fn new(alignment: ListAlignment, default_item_height: Pixels) -> Self {
        Self {
            state: ListState::new(0, alignment, default_item_height),
            synced: RefCell::new(Rc::new(Vec::new())),
            synced_seq: Cell::new(u64::MAX),
        }
    }

    /// The underlying `ListState` — to paint (`gpui::list(list.state().clone(),
    /// …)`), reveal into (`scroll_to_reveal_item`), or scroll.
    pub(crate) fn state(&self) -> &ListState {
        &self.state
    }

    /// Item count currently registered (drives reveal-bounds guards).
    pub(crate) fn len(&self) -> usize {
        self.state.item_count()
    }

    /// Reconcile the list to `items`, splicing only the changed range so scroll
    /// stays anchored. No-op when `seq` is unchanged AND `items` is the same
    /// `Rc` as last time (the cursor-blink / selection / cross-tile-notify
    /// frame). Idempotent within a content version.
    pub(crate) fn reconcile(&self, items: &Rc<Vec<T>>, seq: u64) {
        if self.synced_seq.get() == seq && Rc::ptr_eq(&self.synced.borrow(), items) {
            return;
        }
        self.synced_seq.set(seq);
        let old = self.synced.borrow().clone();
        splice_list_to_items(&self.state, &old, items);
        *self.synced.borrow_mut() = items.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::compose_first_visible_line;

    /// The load-bearing invariant: whatever window `compose_first_visible_line`
    /// picks, the caret line is ALWAYS within it `[first, first + visible)`.
    /// This is the permanent guard against the "caret off-screen in the chatbox"
    /// regression — it pins the property directly, for every caret position.
    #[test]
    fn caret_is_always_within_the_chosen_window() {
        const VISIBLE: usize = 8;
        for line_count in [1usize, 8, 9, 50, 200] {
            for prev_top in [0usize, 3, 40, 199, 1000] {
                for cursor_line in 0..line_count {
                    let first =
                        compose_first_visible_line(cursor_line, prev_top, line_count, VISIBLE);
                    assert!(
                        cursor_line >= first && cursor_line < first + VISIBLE,
                        "caret {cursor_line} escaped window [{first}, {}) \
                         (line_count={line_count}, prev_top={prev_top})",
                        first + VISIBLE,
                    );
                    // Never scroll past the end into blank space.
                    assert!(
                        first <= line_count.saturating_sub(VISIBLE),
                        "window top {first} scrolled past end (line_count={line_count})",
                    );
                }
            }
        }
    }

    #[test]
    fn stable_when_caret_already_visible_and_minimal_otherwise() {
        // Caret inside the current window → window doesn't move.
        assert_eq!(compose_first_visible_line(12, 10, 100, 8), 10);
        // Caret just below the window → scroll down exactly one line's worth.
        assert_eq!(compose_first_visible_line(18, 10, 100, 8), 11);
        // Caret above the window → caret becomes the top line.
        assert_eq!(compose_first_visible_line(2, 10, 100, 8), 2);
        // Caret at the very end of a long draft → window pinned to the tail.
        assert_eq!(compose_first_visible_line(99, 0, 100, 8), 92);
        // Everything fits → always top.
        assert_eq!(compose_first_visible_line(5, 0, 6, 8), 0);
    }
}
