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

/// The left visible COLUMN that keeps the caret column in view — the exact
/// horizontal mirror of [`compose_first_visible_line`], for the compose box's
/// uniform-advance (monospace) grid (spec-chatbox-caret-containment.md
/// Behavior 4). See that function for the why; this is the same minimal-scroll,
/// clamp-to-content window on the column axis.
///
/// The ONE deliberate asymmetry: the vertical window keeps `cursor_line` in
/// `[top, top + visible)` (exclusive far edge), but the horizontal window keeps
/// `cursor_col` in `[left, left + visible - 1]` — it reserves the **rightmost
/// column for the caret glyph**, which paints as a block slightly wider than one
/// column advance. Without the reservation a caret at end-of-line overflows the
/// right clip by ~one caret width (the recurring "text/caret off the right edge"
/// half of the bug). `saturating_sub` floors every subtraction at 0 so an empty
/// or short line yields `left = 0` (no underflow).
pub(crate) fn compose_first_visible_col(
    cursor_col: usize,
    prev_left: usize,
    line_len: usize,
    visible: usize,
) -> usize {
    let visible = visible.max(1);
    // The caret may sit one past the last char (EOL), so the scrollable extent
    // is `line_len` columns wide with the caret allowed at column `line_len`.
    let max_left = (line_len + 1).saturating_sub(visible);
    let prev_left = prev_left.min(max_left);
    // Usable columns before the reserved caret column.
    let inner = visible.saturating_sub(1).max(1);
    let left = if cursor_col < prev_left {
        // Caret left of the window → it becomes the left column.
        cursor_col
    } else if cursor_col > prev_left + inner {
        // Caret at/right of the reserved edge → scroll so it sits on it.
        cursor_col - inner
    } else {
        prev_left
    };
    left.min(max_left)
}

/// The compose box's visible top-left grid cell — the single authoritative
/// scroll offset for both axes (spec-chatbox-caret-containment.md). Recomputed
/// every frame by [`compose_window`] from the current caret + measured extent;
/// the list is scrolled *to* `top_line` (it is never read back from the list's
/// own anchor) and every row is sliced to the column window starting at
/// `left_col`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ComposeWindow {
    pub(crate) top_line: usize,
    pub(crate) left_col: usize,
}

/// The ONE chokepoint that decides the compose box's scroll offset on both axes
/// (spec-chatbox-caret-containment.md Behavior 3). Pure: it calls the two
/// axis-window functions and returns the new [`ComposeWindow`]. Every compose
/// render builds the box from this result; no other code sets scroll offset.
///
/// `cursor_line_len` is the length (in chars) of the line the caret is on, so
/// the horizontal clamp never scrolls past that line's end.
pub(crate) fn compose_window(
    cursor_line: usize,
    cursor_col: usize,
    cursor_line_len: usize,
    prev: ComposeWindow,
    line_count: usize,
    visible_rows: usize,
    visible_cols: usize,
) -> ComposeWindow {
    ComposeWindow {
        top_line: compose_first_visible_line(cursor_line, prev.top_line, line_count, visible_rows),
        left_col: compose_first_visible_col(cursor_col, prev.left_col, cursor_line_len, visible_cols),
    }
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
    use super::{
        compose_first_visible_col, compose_first_visible_line, compose_window, ComposeWindow,
    };

    /// Horizontal mirror of `caret_is_always_within_the_chosen_window`: whatever
    /// LEFT column the function picks, the caret column is inside the window AND
    /// the rightmost column stays reserved for the caret glyph
    /// (`cursor_col <= left + visible - 1`). The permanent guard against the
    /// "caret/text off the RIGHT edge" half of the recurring bug.
    #[test]
    fn caret_col_is_always_within_window_with_caret_slack() {
        for visible in [1usize, 2, 8, 40] {
            for line_len in [0usize, 1, 7, 40, 200] {
                for prev_left in [0usize, 3, 40, 199, 1000] {
                    // Caret can sit anywhere from col 0 to col line_len (EOL).
                    for cursor_col in 0..=line_len {
                        let left =
                            compose_first_visible_col(cursor_col, prev_left, line_len, visible);
                        let inner = visible.saturating_sub(1).max(1);
                        assert!(
                            cursor_col >= left && cursor_col <= left + inner,
                            "caret col {cursor_col} escaped window [{left}, {}] \
                             (visible={visible}, line_len={line_len}, prev_left={prev_left})",
                            left + inner,
                        );
                        // Never scroll past the end into blank space (caret may
                        // be at column line_len, so extent is line_len + 1).
                        let max_left = (line_len + 1).saturating_sub(visible);
                        assert!(
                            left <= max_left,
                            "left {left} scrolled past end (line_len={line_len}, visible={visible})",
                        );
                    }
                }
            }
        }
    }

    /// Boundary inputs the reviewer flagged (V2): empty line and a line shorter
    /// than the window must both yield `left = 0` with no underflow.
    #[test]
    fn short_and_empty_lines_pin_left_to_zero() {
        assert_eq!(compose_first_visible_col(0, 0, 0, 8), 0); // empty line
        assert_eq!(compose_first_visible_col(0, 5, 0, 8), 0); // empty, stale prev
        assert_eq!(compose_first_visible_col(3, 0, 3, 8), 0); // line_len < visible
        assert_eq!(compose_first_visible_col(7, 99, 7, 8), 0); // fits exactly, stale prev
    }

    /// `compose_window` keeps the caret CELL inside the box on BOTH axes for a
    /// wide sweep — the combined invariant the whole spec exists to guarantee.
    #[test]
    fn compose_window_contains_caret_cell_on_both_axes() {
        for visible_rows in [1usize, 8] {
            for visible_cols in [1usize, 20, 80] {
                for line_count in [1usize, 8, 50] {
                    for cursor_line in 0..line_count {
                        for &line_len in &[0usize, 5, 120] {
                            for &cursor_col in &[0usize, line_len / 2, line_len] {
                                let w = compose_window(
                                    cursor_line,
                                    cursor_col,
                                    line_len,
                                    ComposeWindow { top_line: 999, left_col: 999 },
                                    line_count,
                                    visible_rows,
                                    visible_cols,
                                );
                                assert!(
                                    cursor_line >= w.top_line
                                        && cursor_line < w.top_line + visible_rows,
                                    "line {cursor_line} escaped {w:?} (rows={visible_rows})",
                                );
                                let inner = visible_cols.saturating_sub(1).max(1);
                                assert!(
                                    cursor_col >= w.left_col && cursor_col <= w.left_col + inner,
                                    "col {cursor_col} escaped {w:?} (cols={visible_cols})",
                                );
                            }
                        }
                    }
                }
            }
        }
    }


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
