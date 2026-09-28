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

/// Length of the shared PREFIX and the (non-overlapping) shared SUFFIX of two
/// sequences — the minimal-changed-range alignment every incremental surface
/// reconciles through (list splice, highlight cache, WP line kinds). The
/// changed range is `old[pre..old.len()-suf]` → `new[pre..new.len()-suf]`.
pub(crate) fn common_prefix_suffix<T: PartialEq>(old: &[T], new: &[T]) -> (usize, usize) {
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
    (pre, suf)
}

/// Splice a `gpui::ListState` from `old` items to `new` items by replacing only
/// the minimal changed range (shared prefix + shared suffix trimmed), rather
/// than `reset()`-ing the whole list. Preserving the unchanged head/tail keeps
/// their height measurements and lets gpui re-anchor `logical_scroll_top` across
/// the edit, so the viewport doesn't jump. Free function (not a method) so it
/// stays unit-testable against a bare `ListState`.
pub(crate) fn splice_list_to_items<T: PartialEq>(list: &ListState, old: &[T], new: &[T]) {
    let (pre, suf) = common_prefix_suffix(old, new);
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

/// Paint `child` over rows `first_row .. first_row + rows` of a virtualized
/// `gpui::list` whose rows are ALL exactly `row_h` tall, so it scrolls with
/// the list and is clipped to the list's viewport — an "inline" surface that
/// is NOT part of the list's (possibly cached) render.
///
/// Why: a cached list body (e.g. `DiffView`) re-renders only when its own
/// inputs move, and a child entity notifying inside it would dirty it too
/// (gpui marks ancestors dirty). A text input that must LOOK inline (the Diff
/// tile's GitHub-style comment compose) therefore reserves its height with
/// spacer rows inside the list and is painted by the (uncached) parent through
/// this element, placed over the spacers. The parent must add it AFTER the
/// list in tree order: its `prepaint` reads the list's scroll + viewport,
/// which the list (or its reused cached prepaint) has settled by then.
///
/// Placement is exact integer arithmetic over the uniform row height and the
/// list's logical scroll top (no measurement), so it holds for rows the list
/// has not measured yet. Takes no layout space itself (absolute, zero-size);
/// skips prepaint/paint entirely while the rows are scrolled out of view.
pub(crate) fn list_rows_overlay(
    state: ListState,
    first_row: usize,
    rows: usize,
    row_h: Pixels,
    child: gpui::AnyElement,
) -> gpui::AnyElement {
    gpui::IntoElement::into_any_element(ListRowsOverlay {
        state,
        first_row,
        rows,
        row_h,
        child,
    })
}

/// The window-space rect of rows `first_row .. first_row + rows` of a
/// uniform-height list whose viewport is `viewport` and whose logical scroll
/// top is `(top_ix, offset_in_item)`. Pure (unit-tested).
pub(crate) fn uniform_rows_rect(
    viewport: gpui::Bounds<Pixels>,
    top_ix: usize,
    offset_in_item: Pixels,
    first_row: usize,
    rows: usize,
    row_h: Pixels,
) -> gpui::Bounds<Pixels> {
    let y = viewport.origin.y + row_h * (first_row as f32 - top_ix as f32) - offset_in_item;
    gpui::Bounds::new(
        gpui::point(viewport.origin.x, y),
        gpui::size(viewport.size.width, row_h * rows as f32),
    )
}

struct ListRowsOverlay {
    state: ListState,
    first_row: usize,
    rows: usize,
    row_h: Pixels,
    child: gpui::AnyElement,
}

impl gpui::IntoElement for ListRowsOverlay {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for ListRowsOverlay {
    type RequestLayoutState = ();
    /// The viewport clip, when the rows are (partly) visible this frame.
    type PrepaintState = Option<gpui::Bounds<Pixels>>;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let style = gpui::Style {
            position: gpui::Position::Absolute,
            ..gpui::Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<gpui::Bounds<Pixels>> {
        let viewport = self.state.viewport_bounds();
        if viewport.size.height <= Pixels::ZERO || self.rows == 0 {
            return None;
        }
        let top = self.state.logical_scroll_top();
        let rect = uniform_rows_rect(viewport, top.item_ix, top.offset_in_item, self.first_row, self.rows, self.row_h);
        if !rect.intersects(&viewport) {
            return None;
        }
        self.child.layout_as_root(
            gpui::size(
                gpui::AvailableSpace::Definite(rect.size.width),
                gpui::AvailableSpace::Definite(rect.size.height),
            ),
            window,
            cx,
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds: viewport }), |window| {
            self.child.prepaint_at(rect.origin, window, cx)
        });
        Some(viewport)
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        clip: &mut Option<gpui::Bounds<Pixels>>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        if let Some(viewport) = *clip {
            window.with_content_mask(Some(gpui::ContentMask { bounds: viewport }), |window| {
                self.child.paint(window, cx)
            });
        }
    }
}

/// Where an inline overlay's placeholder landed in the frame it was last laid
/// out: its window-space `bounds` and the `clip` (content mask) it painted
/// under. Written by [`overlay_slot`] at prepaint, read by [`slot_overlay`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OverlaySlot {
    pub(crate) bounds: gpui::Bounds<Pixels>,
    pub(crate) clip: gpui::Bounds<Pixels>,
}

/// Shared cell carrying an [`OverlaySlot`] from a (possibly cached) list body
/// to the uncached parent that paints the overlay. `None` = the placeholder
/// was not laid out (scrolled away / absent) the last time the body rendered.
pub(crate) type OverlaySlotCell = Rc<Cell<Option<OverlaySlot>>>;

/// Wrap a PLACEHOLDER element inside a virtualized list so its laid-out bounds
/// (and clip) are recorded into `cell` at prepaint — the exact position the
/// list chose, whatever its alignment or scroll model (non-uniform rows,
/// bottom-pinned follow-tail). Pair with [`slot_overlay`].
///
/// The body that renders the placeholder must CLEAR the cell at the top of
/// each render (so a placeholder that scrolled out of the laid-out range reads
/// `None`); a cached body whose prepaint is reused leaves the cell untouched —
/// correct, since nothing it laid out moved.
pub(crate) fn overlay_slot(cell: OverlaySlotCell, child: gpui::AnyElement) -> gpui::AnyElement {
    gpui::IntoElement::into_any_element(OverlaySlotSink { cell, child })
}

/// Paint `child` exactly over the placeholder recorded in `cell` (see
/// [`overlay_slot`]), clipped like the placeholder — an "inline" surface that
/// is NOT part of the list's (cached) render. The non-uniform-row sibling of
/// [`list_rows_overlay`]: the agent transcript's inline You-block (D11) is one
/// variable-height item in a bottom-aligned list, so its position is read
/// back from the placeholder instead of computed.
///
/// Why: a child entity notifying INSIDE a cached body dirties the body too
/// (gpui marks ancestors dirty), so a text input that must LOOK inline reserves
/// its height with a placeholder and is painted by the (uncached) parent
/// through this element. The parent must add it AFTER the body in tree order:
/// its `prepaint` reads the slot the body's list settled this frame. Takes no
/// layout space itself (absolute, zero-size); skips prepaint/paint entirely
/// while the slot is absent or clipped away.
pub(crate) fn slot_overlay(cell: OverlaySlotCell, child: gpui::AnyElement) -> gpui::AnyElement {
    gpui::IntoElement::into_any_element(SlotOverlay { cell, child })
}

struct OverlaySlotSink {
    cell: OverlaySlotCell,
    child: gpui::AnyElement,
}

impl gpui::IntoElement for OverlaySlotSink {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for OverlaySlotSink {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.cell.set(Some(OverlaySlot {
            bounds,
            clip: window.content_mask().bounds,
        }));
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.child.paint(window, cx);
    }
}

struct SlotOverlay {
    cell: OverlaySlotCell,
    child: gpui::AnyElement,
}

impl gpui::IntoElement for SlotOverlay {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for SlotOverlay {
    type RequestLayoutState = ();
    /// The clip, when the slot is (partly) visible this frame.
    type PrepaintState = Option<gpui::Bounds<Pixels>>;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let style = gpui::Style {
            position: gpui::Position::Absolute,
            ..gpui::Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<gpui::Bounds<Pixels>> {
        let slot = self.cell.get()?;
        if !slot.bounds.intersects(&slot.clip) {
            return None;
        }
        self.child.layout_as_root(
            gpui::size(
                gpui::AvailableSpace::Definite(slot.bounds.size.width),
                gpui::AvailableSpace::Definite(slot.bounds.size.height),
            ),
            window,
            cx,
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds: slot.clip }), |window| {
            self.child.prepaint_at(slot.bounds.origin, window, cx)
        });
        Some(slot.clip)
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        clip: &mut Option<gpui::Bounds<Pixels>>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        if let Some(clip) = *clip {
            window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
                self.child.paint(window, cx)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_first_visible_line, uniform_rows_rect};

    /// `list_rows_overlay`'s placement: rows below the scroll top land at
    /// `(first_row - top) * row_h - offset` under the viewport top; scrolling
    /// by one row (or a partial row) moves the rect by exactly that much.
    #[test]
    fn uniform_rows_rect_tracks_the_scroll_top() {
        use gpui::{point, px, size, Bounds};
        let vp = Bounds::new(point(px(10.0), px(100.0)), size(px(500.0), px(300.0)));
        let r = uniform_rows_rect(vp, 0, px(0.0), 4, 3, px(20.0));
        assert_eq!((r.origin.x, r.origin.y, r.size.width, r.size.height), (px(10.0), px(180.0), px(500.0), px(60.0)));
        let r = uniform_rows_rect(vp, 2, px(5.0), 4, 3, px(20.0));
        assert_eq!(r.origin.y, px(135.0), "scrolled 2 rows + 5px");
        let r = uniform_rows_rect(vp, 6, px(0.0), 4, 3, px(20.0));
        assert_eq!(r.origin.y, px(60.0), "scrolled past: above the viewport");
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

