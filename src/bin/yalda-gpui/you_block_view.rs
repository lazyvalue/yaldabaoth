//! `YouBlockView` — the ACTIVE inline You-block (UXI-AgentTile-11 rule 5) as
//! its own cached view entity (text-editing review D11).
//!
//! The You-block LOOKS like part of the transcript — it sits at its anchor and
//! scrolls with the conversation — but its content is the separate `Compose`
//! draft, which changes on every keystroke. Rendered as a transcript list item,
//! every keystroke re-rendered the whole cached `TranscriptView` (every visible
//! row). A child entity nested INSIDE the transcript can't fix that: gpui marks
//! a notified view's ancestors dirty, so the transcript would re-render anyway.
//!
//! So the block is split the way the Diff tile's inline comment compose is
//! (`list_rows_overlay`): the transcript renders a fixed-height PLACEHOLDER
//! item (`overlay_slot`, sized from the draft's visual-row count), and the root
//! paints THIS view over it (`slot_overlay`), outside the transcript's subtree.
//! A keystroke that doesn't change the block's height notifies only this view
//! (its observe filter, [`YouBlockSeqs`]); the transcript's slice
//! (`TranscriptSeqs::you_block_rows`) moves only when the height does.
//!
//! The wrap width is decided by the TRANSCRIPT (it sizes the placeholder) and
//! shared through [`YouBlockLayout`], so the painted block always matches its
//! slot. Block geometry is pinned (label + rows at fixed px) so the height is
//! exact arithmetic: [`you_block_height_px`].

use super::*;

/// Perf/test label for [`YouBlockView`] renders.
pub(crate) const YOU_BLOCK_PERF_LABEL: &str = "you_block";

/// Vertical padding above and below a You-block's content (`pt_2` / `pb_2`).
pub(crate) const YB_PAD_Y_PX: f32 = 8.0;
/// The pinned height of the block's `You` label row.
pub(crate) const YB_LABEL_H_PX: f32 = 18.0;
/// One wrapped compose row (`ChatboxRowStyle::compose`'s line height).
pub(crate) const YB_ROW_H_PX: f32 = 18.0;
/// Block chrome above the first content row (top padding + label).
pub(crate) const YB_HEADER_PX: f32 = YB_PAD_Y_PX + YB_LABEL_H_PX;

/// Painted height of a You-block holding `rows` wrapped visual rows (≥ 1).
pub(crate) fn you_block_height_px(rows: usize) -> f32 {
    YB_HEADER_PX + rows.max(1) as f32 * YB_ROW_H_PX + YB_PAD_Y_PX
}

/// The compose's MEASURED wrap width in columns (its box width captured at the
/// last paint ÷ [`CHATBOX_CHAR_W`]), or 0 before it has painted.
pub(crate) fn compose_measured_cols(compose: &Compose) -> usize {
    let w = compose.bounds.get().2;
    if w > 1.0 {
        (w / CHATBOX_CHAR_W).floor().max(1.0) as usize
    } else {
        0
    }
}

/// The layout the transcript sized the active block's placeholder for: the
/// wrap width in columns and the resulting visual-row count. Shared (the
/// transcript writes it at render; [`YouBlockView`] reads it) so the painted
/// block wraps exactly like the slot it covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct YouBlockLayout {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

/// Theme colors a You-block paints with (resolved once per render).
#[derive(Clone, Copy)]
pub(crate) struct YouBlockColors {
    /// The caret + ACTIVE-block accent (deep red, `at.cursor`).
    pub(crate) cursor: Hsla,
    /// The parked / sent accent (teal, `at.warm_accent`).
    pub(crate) resting: Hsla,
    pub(crate) fg: Hsla,
    pub(crate) selection_bg: Hsla,
    pub(crate) dim: Hsla,
}

impl YouBlockColors {
    pub(crate) fn of(at: &yalda::theme::AgentTheme, fg: Hsla) -> Self {
        Self {
            cursor: nc(at.cursor),
            resting: nc(at.warm_accent),
            fg,
            selection_bg: nc(at.selection_bg),
            dim: nc(at.dim),
        }
    }
}

/// Build one inline You-block element — the ACTIVE block (painted by
/// [`YouBlockView`]) and the read-only PARKED blocks (rendered in the
/// transcript) share it. `caret` = `(line, display col)` when the block holds
/// focus; `sink` = the active block's width capture (drives the wrap width).
#[allow(clippy::too_many_arguments)]
pub(crate) fn you_block_element(
    lines: &[String],
    caret: Option<(usize, usize)>,
    mode: EditMode,
    selection: Option<((usize, usize), (usize, usize))>,
    active: bool,
    nav_on_anchor: bool,
    wrap_cols: usize,
    code_font: &SharedString,
    colors: YouBlockColors,
    sink: Option<(std::rc::Rc<std::cell::Cell<(f32, f32, f32, f32)>>, OnWidthChange)>,
) -> AnyElement {
    let focused = caret.is_some();
    // The ACTIVE (live-compose) block reads RED — border + label + wash — so an
    // in-progress turn is visually distinct from the SENT turns above it, which
    // keep the teal accent. A parked (read-only) insertion block stays teal.
    let ws_accent = crate::screens::you_block_accent(active, colors.cursor, colors.resting);
    let row_style =
        ChatboxRowStyle::compose(code_font.clone(), colors.fg, colors.cursor, colors.selection_bg);
    // INTENT — co-authoring a document: the block renders EVERY line and GROWS
    // with its content (never a fixed box scrolling its own text). Keeping the
    // caret visible is the TRANSCRIPT scroll's job (UXI-TextEditing-1).
    let (caret_line, caret_col) = caret.unwrap_or((usize::MAX, 0));
    let mut inner = div().flex().flex_col().w_full().min_w_0();
    for (i, line) in lines.iter().enumerate() {
        inner = inner.child(build_chatbox_wrapped_line(
            line,
            i == caret_line,
            caret_col,
            mode,
            selection,
            i,
            wrap_cols,
            &row_style,
        ));
    }
    // you-div SCOPED-NORMAL indicator: focused in Normal mode (Esc-once: editing
    // the reply with motions) badges the label `You · NORMAL`.
    let scoped_normal = focused && mode == EditMode::Normal;
    let row_bg: Hsla = if active && focused {
        // Light RED wash behind the live draft — slighter while typing
        // (Insert) than at rest (Normal).
        let mut h = crate::screens::worksheet_wash_red();
        h.a = crate::screens::worksheet_backdrop_alpha(mode);
        h
    } else if nav_on_anchor {
        // Nav-focus highlight: the transcript cursor sits on the block's anchor.
        let mut h = colors.dim;
        h.a = 0.2;
        h
    } else {
        rgba(0x00000000).into()
    };
    let label = if scoped_normal {
        SharedString::from("You · NORMAL")
    } else {
        SharedString::new_static("You")
    };
    let block = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .pt(px(YB_PAD_Y_PX))
        .pb(px(YB_PAD_Y_PX))
        .pl_2()
        .border_l_2()
        .border_color(ws_accent)
        .bg(row_bg)
        .child(
            // Pinned label row so the block's height is exact arithmetic
            // (`you_block_height_px`) — the transcript sizes the placeholder
            // the overlay paints into from it.
            div()
                .flex_none()
                .h(px(YB_LABEL_H_PX))
                .line_height(px(YB_LABEL_H_PX))
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(ws_accent)
                .font_family(code_font.clone())
                .child(label),
        );
    // Only the ACTIVE block measures its width (it owns the bounds Cell that
    // drives the wrap width); parked blocks render plainly.
    let block = match sink {
        Some((sink, on_width_change)) => block.child(CaptureBounds {
            inner: inner.into_any_element(),
            sink,
            on_width_change,
        }),
        None => block.child(inner),
    };
    probe_bounds("you-block", block.into_any_element())
}

/// The slice of session state [`YouBlockView`] renders — its observe filter.
/// A keystroke moves `compose_edit_seq` / `cursor`; focus, mode, selection and
/// the transcript nav highlight each change the painted block too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct YouBlockSeqs {
    pub(crate) active: bool,
    pub(crate) compose_edit_seq: u64,
    pub(crate) cursor: (usize, usize),
    pub(crate) mode: EditMode,
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    pub(crate) focused: bool,
    pub(crate) nav_on_anchor: bool,
}

impl YouBlockSeqs {
    pub(crate) fn of(c: &AgentState) -> Self {
        if !c.inline_you_block_active() {
            return Self::default();
        }
        let compose = c.input_surface.compose();
        let cc = compose.editor.cursor();
        let last_line = c.editor.document().line_count().saturating_sub(1);
        let anchor_line = c.effective_you_block_anchor().unwrap_or(last_line);
        Self {
            active: true,
            compose_edit_seq: compose.edit_seq(),
            cursor: (cc.line, cc.col),
            mode: compose.mode,
            selection: compose.editor.selection_range(),
            focused: c.focus == AgentFocus::Compose,
            nav_on_anchor: c.focus == AgentFocus::Transcript && c.editor.cursor().line == anchor_line,
        }
    }

    /// Digest used to KEY the overlay's element id in `render_agent` — the same
    /// dropped-self-notify backstop the transcript uses
    /// (`TranscriptSeqs::fingerprint_hash`).
    pub(crate) fn fingerprint_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// The ACTIVE inline You-block, a cached view painted by the root over the
/// transcript's placeholder (see the module docs). One per `TranscriptView`.
pub(crate) struct YouBlockView {
    session: Entity<AgentSession>,
    root: WeakEntity<YaldaGpuiView>,
    /// The wrap width the transcript sized the placeholder for.
    layout: std::rc::Rc<std::cell::Cell<YouBlockLayout>>,
    /// The owning transcript: a block width change re-sizes ITS placeholder.
    transcript: gpui::EntityId,
    last_rendered: YouBlockSeqs,
}

impl YouBlockView {
    pub(crate) fn new(
        session: Entity<AgentSession>,
        root: WeakEntity<YaldaGpuiView>,
        layout: std::rc::Rc<std::cell::Cell<YouBlockLayout>>,
        transcript: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> Self {
        // Self-invalidate on a slice move (observe runs in effect flush —
        // outside the draw; never notify from render).
        cx.observe(&session, |this: &mut YouBlockView, session, cx| {
            let now = YouBlockSeqs::of(&session.read(cx).state);
            if now != this.last_rendered {
                record_notify(YOU_BLOCK_PERF_LABEL, MissReason::Dirtied);
                cx.notify();
            }
        })
        .detach();
        Self {
            session,
            root,
            layout,
            transcript,
            last_rendered: YouBlockSeqs::default(),
        }
    }
}

impl Render for YouBlockView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(YOU_BLOCK_PERF_LABEL);
        let seqs = YouBlockSeqs::of(&self.session.read(cx).state);
        self.last_rendered = seqs;
        let Some(root) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        if !seqs.active {
            return div().size_full().into_any_element();
        }
        let (colors, code_font) = {
            let r = root.read(cx);
            (YouBlockColors::of(&r.theme.agent, r.editor_fg()), r.code_font.clone())
        };
        let layout = self.layout.get();
        let cols = layout.cols.max(1);
        let s = self.session.read(cx);
        let compose = s.state.input_surface.compose();
        // The same `(edit_seq, cols)` snapshot the transcript sized the slot
        // from — a cache hit, not a second O(draft) pass.
        let snap = compose.render_snapshot(cols);
        let doc = compose.editor.document();
        let cc = compose.editor.cursor();
        let caret = seqs
            .focused
            .then(|| (cc.line, display_col(doc, cc.line, cc.col)));
        let selection = display_selection(doc, compose.editor.selection_range());
        you_block_element(
            &snap.lines,
            caret,
            compose.mode,
            selection,
            true,
            seqs.nav_on_anchor,
            cols,
            &code_font,
            colors,
            Some((compose.bounds.clone(), OnWidthChange::Notify(self.transcript))),
        )
    }
}
