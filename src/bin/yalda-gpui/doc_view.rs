//! `DocView` — the cached body of a Buffer tile in `Viewing` mode (the rendered
//! markdown block list). Graph 4f1 doc-view: the body used to be built inline
//! by `render_doc` on EVERY root notify (typing in another tile, agent
//! streaming, a menu opening…), re-running the visible blocks' element build
//! each time. Now it is a yux component (see `yux/CLAUDE.md`) embedded via
//! `cached_child` in `render_doc` (`screens.rs`), exactly like the Edit body
//! (`edit_view.rs`, `EditBodyView`).
//!
//! # Why this observes the ROOT (the `DiffView` / `EditBodyView` shape)
//!
//! `DocState` is a plain struct in the workspace layout tree, not a GPUI
//! entity, and the text lives in the pooled `SharedCore`. The only entity it is
//! reachable through is the root view, so this view observes the root and
//! filters every notify through a cheap fingerprint ([`DocSeqs`]). Global
//! inputs (theme, zoom, fonts, the diagram cache) live on that same root, so
//! they fall out of the fingerprint without a separate `notify_*_views` walk.
//!
//! # State ownership
//!
//! The view READS `DocState` (blocks, cursor, list) and the root's
//! `doc_selection` / globals. The block list stays on `DocState` because
//! root-side actions (block nav, the outline rail's jump, the Edit→Doc
//! landing) scroll it without a `cx` — its logical scroll top is in the
//! fingerprint, so any such scroll re-renders this body. Nothing here mutates
//! `DocState`: the blocks are re-derived and the list reconciled on the
//! mutation/effect path (`DocState::set_blocks`, `refresh_painted_docs` from the
//! root's self-observe). What this render does touch is render bookkeeping —
//! the hit-test sink it repopulates as it paints, and the post-landing settle
//! (UXI-Buffer-9), whose follow-up frame is `cx.defer`red, never notified.
//!
//! # Interactive rows (yux rule 4)
//!
//! Link clicks capture the link target of the block they were built from; the
//! fingerprint covers the blocks, so a replayed (cache-hit) paint always holds
//! the targets of the blocks it shows. Mouse selection listeners live on the
//! UNCACHED wrapper in `render_doc`, capture only the tile id, and resolve the
//! Doc + its hit-test sink at event time (`doc_mouse_*` in `doc_ui.rs`).

use super::*;

/// The render-input watermark the root-observe filter compares across
/// renders. EVERY input the body reads must be covered here (yux rule 2).
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(crate) struct DocSeqs {
    /// Identity + version of the rendered blocks (a re-parse or `set_blocks`).
    blocks_ptr: usize,
    blocks_seq: u64,
    /// The pooled core's content generation — moves on a sibling Edit
    /// keystroke even before `refresh_painted_docs` re-derives the blocks, so
    /// the body never depends on observer ordering.
    source_seq: Option<u64>,
    /// The block carrying the cursor bar.
    cursor_block: usize,
    /// The list's logical scroll top `(item, offset bits)` — root-side nav /
    /// outline jumps / landings scroll the list without notifying the body.
    scroll_top: (usize, u32),
    /// The root's view-mode mouse selection (painted as a run background).
    selection: Option<DocSelection>,
    /// Global zoom (`text_scale.to_bits()`).
    text_scale_bits: u32,
    /// Global theme (block colors).
    theme: yalda::theme::ThemeName,
    /// Hash of the body/code font family names.
    fonts: u64,
    /// Settled mermaid renders (`DiagramCache::generation`) — a diagram that
    /// finished rendering repaints its block.
    diagrams: u64,
    /// The transient "Copied" flag of a code block's copy button.
    code_copied_seq: u64,
}

impl DocSeqs {
    pub(crate) fn of(d: &DocState, root: &YaldaGpuiView) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        root.body_font.hash(&mut h);
        root.code_font.hash(&mut h);
        let top = d.list.state().logical_scroll_top();
        DocSeqs {
            blocks_ptr: Rc::as_ptr(&d.blocks) as *const () as usize,
            blocks_seq: d.blocks_seq,
            source_seq: d.source.as_ref().map(DocSource::edit_seq),
            cursor_block: d.cursor_block,
            scroll_top: (top.item_ix, f32::from(top.offset_in_item).to_bits()),
            selection: root.doc_selection,
            text_scale_bits: root.text_scale.to_bits(),
            theme: root.theme.name,
            fonts: h.finish(),
            diagrams: root.diagrams.borrow().generation(),
            code_copied_seq: d.code_copied_seq,
        }
    }
}

/// The cached Doc body. One per Viewing tile, owned by its `DocState`
/// (`body`), dropped with it — no registry.
pub(crate) struct DocView {
    root: WeakEntity<YaldaGpuiView>,
    /// The tile this body belongs to — how `render` finds its `DocState`.
    window_id: workspace::WindowId,
    last_rendered: DocSeqs,
    perf_label: &'static str,
}

impl DocView {
    /// Construct the body and register the root-observe subscription that
    /// self-notifies only when [`DocSeqs`] moved (mirrors `EditBodyView::new`).
    pub(crate) fn new(
        root: Entity<YaldaGpuiView>,
        window_id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&root, |this: &mut DocView, root_ent, cx| {
            let now = root_ent.read(cx).doc_seqs_for(this.window_id);
            if this.last_rendered != now {
                record_notify(this.perf_label, MissReason::Dirtied);
                cx.notify();
            }
        })
        .detach();
        DocView {
            root: root.downgrade(),
            window_id,
            last_rendered: DocSeqs::default(),
            perf_label: "doc-body",
        }
    }
}

impl DocView {
    /// The tile this body renders (the `DocState` lookup key).
    pub(crate) fn window_id(&self) -> workspace::WindowId {
        self.window_id
    }
}

impl Render for DocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let r = root_ent.read(cx);
        let Some(d) = r.doc_state_ref(self.window_id) else {
            return div().size_full().into_any_element();
        };
        // A Doc landed from Edit (UXI-Buffer-9) re-checks, against the last
        // layout, that the cursor block painted on-screen; may scroll the list,
        // so it runs BEFORE the fingerprint snapshot. The follow-up frame is
        // scheduled via defer below — never a notify inside render.
        let settling = d.list.settle();
        self.last_rendered = DocSeqs::of(d, r);
        // The reading measure derives from the body font + zoom — both
        // already in the fingerprint — so it adds no new render input.
        let measure = reading_measure(window, &r.body_font, r.text_scale);
        let body = build_doc_body(r, d, self.root.clone(), self.window_id, measure)
            .into_any_element();
        #[cfg(test)]
        let body = probe_bounds("doc-body", body);
        if settling {
            let me = cx.entity_id();
            cx.defer(move |app| app.notify(me));
        }
        body
    }
}

/// The virtualized block list of Doc `d` — the body `DocView` caches.
///
/// **Virtualized**: a `gpui::list`, so only the visible block window is built
/// and laid out, and (graph 4f1 doc-view) only when this body's own inputs
/// moved — a root notify from elsewhere is a cache hit. The hit-test sink
/// (`d.line_layouts`) only holds the visible lines, so `doc_pos_in`'s scan is
/// O(visible) too.
/// The reading measure (`TYPE_SCALE.measure_ch` × the body font's `ch`
/// advance at zoom `text_scale`): the widest the Doc's text column gets.
fn reading_measure(window: &Window, body_font: &SharedString, text_scale: f32) -> Pixels {
    let size = px(TYPE_SCALE.body_px * text_scale);
    let ts = window.text_system();
    let id = ts.resolve_font(&gpui::font(body_font.clone()));
    // A font without a `0` glyph: a typical proportional ch (≈0.55em).
    let ch = ts.ch_advance(id, size).unwrap_or(size * 0.55);
    TYPE_SCALE.measure(ch)
}

/// Width of the chrome `block_element` puts left of a block's text (the 3px
/// cursor bar + the content column's `pl_3`), so the TEXT gets the full
/// measure.
const DOC_BLOCK_CHROME_PX: f32 = 3.0 + 12.0;

fn build_doc_body(
    r: &YaldaGpuiView,
    d: &DocState,
    weak_root: WeakEntity<YaldaGpuiView>,
    tile: workspace::WindowId,
    measure: Pixels,
) -> gpui::Stateful<gpui::Div> {
    // This render repaints every visible line and re-registers it; drop the
    // previous frame's entries so a line that scrolled away (or a removed
    // block) can't be hit-tested.
    d.line_layouts.borrow_mut().clear();
    // Wiki / relative links and images resolve against the Doc's directory
    // (`file_label` is the canonicalized path of the backing file).
    let doc_dir = PathBuf::from(d.file_label.as_ref())
        .parent()
        .map(|p| p.to_path_buf());

    // Owned snapshots for the `'static` per-row render closure — all cheap
    // (Theme clone once per render, Rc pointer clones, SharedString refcount
    // bumps, Copy values).
    let theme = r.theme.clone();
    let body_font = r.body_font.clone();
    let code_font = r.code_font.clone();
    let text_scale = r.text_scale;
    let cursor_block = d.cursor_block;
    let doc_selection = r.doc_selection;
    let line_layouts = d.line_layouts.clone();
    let blocks_rc = d.blocks_rc();
    let diagrams = r.diagrams.clone();
    let code_copy = CodeCopyCtx {
        tile,
        copied: d.code_copied.clone(),
    };
    let column_max = measure + px(DOC_BLOCK_CHROME_PX);

    let render_fn = move |idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
        let Some(block) = blocks_rc.get(idx) else {
            return div().into_any_element();
        };
        #[cfg(test)]
        DOC_BLOCK_BUILDS.with(|c| c.set(c.get() + 1));
        let ctx = RenderCtx {
            cursor_block: Some(cursor_block),
            doc_selection,
            line_layouts: Some(line_layouts.clone()),
            weak_view: Some(weak_root.clone()),
            doc_dir: doc_dir.clone(),
            block_count: blocks_rc.len(),
            diagrams: Some(diagrams.clone()),
            code_copy: Some(code_copy.clone()),
            // Doc view never shows raw markdown markers (agent chat only) and
            // `block_element` sets the structural path + current block itself.
            ..RenderCtx::new(&theme, body_font.clone(), code_font.clone(), text_scale)
        };
        let el = block_element(&ctx, idx, block);
        // Reading measure: prose blocks sit in a column capped at the measure
        // and centered in a wider tile (full width in a narrow one). A source
        // file's lines (the code IS the document) keep the full width.
        let el = if matches!(
            block,
            RenderedBlock::CodeBlock {
                source_file: true,
                ..
            }
        ) {
            el
        } else {
            let column = div().w_full().max_w(column_max).child(el).into_any_element();
            #[cfg(test)]
            let column = probe_bounds_dyn(format!("doc-column-{idx}"), column);
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_center()
                .child(column)
                .into_any_element()
        };
        // UXI-ParagraphSpacing-1 test seam: expose each doc block's painted
        // bounds so `verify_harness` can measure the inter-block gap.
        #[cfg(test)]
        let el = probe_bounds_dyn(format!("doc-block-{idx}"), el);
        el
    };

    div()
        .id("doc-body")
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .px_8()
        .py_4()
        .text_size(px(14.0 * r.text_scale))
        .font_family(r.body_font.clone())
        .text_color(r.editor_fg())
        .child(
            // Default (visible-only) measuring — NOT `Auto`. `Auto` means
            // "measure all items" (gpui list.rs), which builds every line to
            // measure it and registers its `TextLayout` into `line_layouts`,
            // but only the visible lines get prepainted (bounds set). Then
            // `doc_pos_in` iterating all of them calls `.bounds()` on an
            // un-prepainted layout → panic across the input callback. The
            // body fills the tile (`cached_child` is `size_full`), so the list
            // fills the viewport and scrolls without sizing to content.
            gpui::list(d.list.state().clone(), render_fn)
                .flex_1()
                .w_full(),
        )
}

impl YaldaGpuiView {
    /// The live render-input fingerprint for the Viewing tile `id` (the
    /// `DocView` root-observe filter). Default for a vanished tile.
    pub(crate) fn doc_seqs_for(&self, id: workspace::WindowId) -> DocSeqs {
        match self.doc_state_ref(id) {
            Some(d) => DocSeqs::of(d, self),
            None => DocSeqs::default(),
        }
    }
}
