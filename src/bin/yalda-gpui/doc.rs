//! Doc (Buffer `Viewing`) data layer: [`DocState`] — the rendered blocks, the
//! block cursor and the virtualized block list of one Doc tile — and
//! [`DocSource`], its handle onto the pooled `SharedCore`.
//!
//! The view-layer methods (nav, mouse selection, key dispatch, the painted-Doc
//! re-derive) live in `doc_ui.rs`; the cached body component in `doc_view.rs`.
//!
//! Every mutation of the block set goes through [`DocState::set_blocks`], which
//! bumps `blocks_seq` AND reconciles the list right there — the list's item
//! count is current the moment a nav action reveals into it, and no render
//! path has to splice (graph 4f1 doc-view).

use super::*;

/// State held while the user is viewing a rendered markdown document.
pub(crate) struct DocState {
    /// The rendered blocks, shared by pointer with the `'static` list render
    /// closure (`blocks_rc`) — a re-parse swaps the `Rc`, never deep-clones
    /// (C3). Replaced only via `set_blocks`.
    pub(crate) blocks: Rc<Vec<RenderedBlock>>,
    /// `spans[i]` = where `blocks[i]` came from in the source text; empty when
    /// the blocks are unmapped (string-backed docs). See `Rendered`.
    pub(crate) spans: Vec<SourceSpan>,
    pub(crate) file_label: SharedString,
    pub(crate) cursor_block: usize,
    /// Variable-height virtualized list driving the doc body. Only the visible
    /// block window is built/laid-out per frame (not one element per block), so
    /// render is O(visible). j/k/g/G/ctrl-d/u nav reveals the focused block via
    /// `scroll_to_reveal_item`. Reconciled by splicing the changed block range
    /// (never `reset()`) so scroll stays anchored across a live edit-flush — see
    /// `ScrollAnchoredList`. Reconciled at the mutation site (`set_blocks`).
    ///
    /// Lives here (not in `DocView`) because root-side actions address it
    /// without a `cx`: block nav reveals, the outline rail's jump
    /// (`outline_jump_to`), and the Edit→Doc landing (`back_to_doc`, which
    /// `land`s a fresh Doc before it has a body). The cached body paints it and
    /// its logical scroll top is part of `DocSeqs`, so any root-side scroll
    /// re-renders the body.
    pub(crate) list: ScrollAnchoredList<RenderedBlock>,
    /// Monotonic version of `blocks`, bumped by `set_blocks` on every
    /// reassignment — the list-reconcile gate and a `DocSeqs` input.
    pub(crate) blocks_seq: u64,
    /// The pooled, shared source this Doc renders (D2 / 5c). `Some` for
    /// file-backed Docs — the SAME `SharedCore` an Edit view of the file
    /// binds to, so editing in Edit shows live in Doc and undo is unified.
    /// `None` for string-backed Docs (help/welcome) and transient
    /// placeholders. Replaces the old `edit_cache` stash: the shared core IS
    /// the live state, so there is nothing to shuttle across a Doc↔Edit
    /// round-trip.
    pub(crate) source: Option<DocSource>,
    /// The cached body view (`DocView`), created lazily by `render_doc` (the
    /// constructors have no `cx`) and dropped with this state.
    pub(crate) body: Option<Entity<DocView>>,
    /// Mouse hit-test sink: each painted line's `TextLayout`, keyed
    /// `(block_idx, line_idx)`, registered at PAINT time by the body. Cleared
    /// only when the body actually re-renders — a cache-hit frame replays the
    /// previous paint, whose geometry is unchanged, so the entries stay valid.
    /// Per tile (the old root-global sink mixed two Docs' lines). Read by the
    /// root's mouse handlers (`doc_pos_in`).
    pub(crate) line_layouts: DocLineLayouts,
    /// Folded headings (UXI-Buffer-13, `doc_fold.rs`), keyed by source line.
    /// Mutated only by the fold commands, `set_blocks` (re-key) and
    /// `restore_folds`, each of which rebuilds `fold_layout` and bumps `fold_seq`.
    pub(crate) folds: DocFolds,
    /// Per-block visibility derived from `(blocks, folds)`; read by the body.
    pub(crate) fold_layout: Rc<FoldLayout>,
    /// Monotonic version of `fold_layout` — a `DocSeqs` input.
    pub(crate) fold_seq: u64,
}

/// Per-Doc mouse hit-test sink: `(block_idx, line_idx)` → painted `TextLayout`.
pub(crate) type DocLineLayouts = Rc<RefCell<HashMap<(usize, usize), TextLayout>>>;

/// A file-backed Doc's handle onto its pooled `SharedCore` (5c). Held so the
/// Doc renders the file's *live* rope (shared with any Edit view) and so the
/// pool's `Rc`-strong-count liveness keeps the buffer alive while the Doc is
/// open.
pub(crate) struct DocSource {
    pub(crate) buffer_id: workspace::FileBufferId,
    pub(crate) core: workspace::SharedCore,
    /// `Document.edit_seq()` the current `blocks` were derived at.
    /// `refresh_blocks` re-derives only when the core has advanced past this —
    /// O(1) when idle, one re-parse per change (the two-tile live path).
    pub(crate) rendered_seq: u64,
}

impl DocSource {
    /// Build a source from a pooled `(buffer_id, core)`, stamping
    /// `rendered_seq` at the core's current `edit_seq` (caller renders the
    /// matching initial `blocks`).
    pub(crate) fn new(buffer_id: workspace::FileBufferId, core: workspace::SharedCore) -> Self {
        let rendered_seq = core.borrow().document().edit_seq();
        Self {
            buffer_id,
            core,
            rendered_seq,
        }
    }
    pub(crate) fn full_text(&self) -> String {
        self.core.borrow().document().full_text()
    }
    pub(crate) fn edit_seq(&self) -> u64 {
        self.core.borrow().document().edit_seq()
    }
    pub(crate) fn is_modified(&self) -> bool {
        self.core.borrow().document().is_modified()
    }
}

impl DocState {
    /// Build a `Viewing` Doc from rendered blocks — the SINGLE construction path
    /// for every Doc tile (load / reload / split / restore / theme re-render).
    /// The list is reconciled here, so it holds every block from birth (a nav
    /// action or a `land` before the first paint addresses real items).
    pub(crate) fn viewing(
        rendered: impl Into<Rendered>,
        file_label: SharedString,
        source: Option<DocSource>,
    ) -> Self {
        let Rendered { blocks, spans } = rendered.into();
        let mut d = DocState {
            blocks: Rc::new(blocks),
            spans,
            file_label,
            cursor_block: 0,
            // Top-aligned: a doc reads from its first block (the agent transcript
            // tails the bottom). 512px default item-height estimate as before.
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, gpui::px(512.0)),
            blocks_seq: 0,
            source,
            body: None,
            line_layouts: DocLineLayouts::default(),
            folds: DocFolds::new(),
            fold_layout: Rc::default(),
            fold_seq: 0,
        };
        d.reconcile_list();
        d.refold();
        d
    }

    /// Replace `blocks`, bump `blocks_seq`, and splice the list to match —
    /// the only path that mutates `blocks` after construction, so the list is
    /// never behind the blocks and no render path reconciles.
    pub(crate) fn set_blocks(&mut self, rendered: impl Into<Rendered>) {
        let Rendered { blocks, spans } = rendered.into();
        self.blocks = Rc::new(blocks);
        self.spans = spans;
        self.blocks_seq = self.blocks_seq.wrapping_add(1);
        self.reconcile_list();
        // Folds follow their headings across the re-parse (UXI-Buffer-13).
        self.rekey_folds();
    }

    /// Re-derive `blocks` from the shared core if it has advanced since the
    /// last derivation (5c live path: an Edit view's keystroke bumps the
    /// shared `edit_seq`). O(1) when idle; at most one markdown parse per
    /// root notify however many edits landed since the last one (keyed on
    /// `edit_seq`, so rapid edits coalesce). C3: called ONLY for Docs that can
    /// be painted (`refresh_painted_docs`, run from the root's self-observe —
    /// the effect path, never a render). Uses a READ-ONLY borrow of the core —
    /// never `borrow_mut` here — so a concurrent Edit mutation on the same core
    /// cannot trigger a `RefCell` double-borrow panic. No-op for string-backed
    /// Docs (`source == None`).
    pub(crate) fn refresh_blocks(&mut self, theme: &Theme) {
        let (seq, text) = match &self.source {
            Some(src) => {
                let seq = src.edit_seq();
                if seq == src.rendered_seq {
                    return;
                }
                (seq, src.full_text())
            }
            None => return,
        };
        #[cfg(test)]
        DOC_REFRESH_PARSES.with(|m| {
            *m.borrow_mut()
                .entry(self.file_label.to_string())
                .or_insert(0) += 1
        });
        let path = PathBuf::from(self.file_label.as_ref());
        self.set_blocks(render_with_wiki_mapped(&text, theme, Some(&path)));
        if let Some(src) = self.source.as_mut() {
            src.rendered_seq = seq;
        }
    }

    /// O(1) pointer clone of the blocks for the `'static` list render closure.
    /// `blocks` is itself an `Rc`, so there is no snapshot to rebuild and no
    /// deep clone per re-parse (C3).
    pub(crate) fn blocks_rc(&self) -> Rc<Vec<RenderedBlock>> {
        self.blocks.clone()
    }

    /// Scroll the virtualized list so `idx` is on-screen. The list is
    /// reconciled at every block mutation, so its count is current; the guard
    /// only covers an out-of-range index.
    pub(crate) fn reveal_block(&self, idx: usize) {
        if idx < self.list.len() {
            self.list.state().scroll_to_reveal_item(idx);
        }
    }

    /// Put the block cursor on `to` (clamped to the last block) and reveal it
    /// — even when it is already there, so `g` scrolls a wheel-scrolled view
    /// back to its cursor. `false` only for an empty doc. The one body every
    /// block-nav action (j/k, ctrl-d/u, g/G, local-menu goto, outline preview)
    /// shares.
    pub(crate) fn move_cursor_to(&mut self, to: usize) -> bool {
        let Some(last) = self.blocks.len().checked_sub(1) else {
            return false;
        };
        let to = to.min(last);
        // A jump into a folded section opens the folds hiding it (UXI-Buffer-13);
        // block nav itself only ever targets painted blocks.
        self.reveal_fold(to);
        self.cursor_block = to;
        self.reveal_block(to);
        true
    }

    /// Reconcile the virtualized block list to the current `blocks`, preserving
    /// scroll. Delegates to `ScrollAnchoredList`, gated on `blocks_seq` so a
    /// repeat call with no block change does zero work.
    pub(crate) fn reconcile_list(&self) {
        self.list.reconcile(&self.blocks_rc(), self.blocks_seq);
    }
}

// Test-only counter of how many `block_element`s the virtualized doc list
// builds. The latency gate (verify_harness) asserts this stays O(visible) —
// a few dozen for a 3000-block doc — proving render is no longer O(document).
#[cfg(test)]
thread_local! {
    pub(crate) static DOC_BLOCK_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// C3: per-`file_label` count of `DocState::refresh_blocks` re-parses (the
    /// full_text copy + markdown parse). The hidden-Doc guard asserts a Doc
    /// that isn't painted does ZERO of these while a sibling Edit tile types.
    static DOC_REFRESH_PARSES: std::cell::RefCell<HashMap<String, usize>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Test-only: C3 re-parse count for the Doc labelled `label` since the last
/// [`test_reset_doc_refresh_parses`].
#[cfg(test)]
pub(crate) fn test_doc_refresh_parses(label: &str) -> usize {
    DOC_REFRESH_PARSES.with(|m| m.borrow().get(label).copied().unwrap_or(0))
}

/// Test-only: zero every C3 re-parse counter.
#[cfg(test)]
pub(crate) fn test_reset_doc_refresh_parses() {
    DOC_REFRESH_PARSES.with(|m| m.borrow_mut().clear());
}
