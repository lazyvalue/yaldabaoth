//! `App::Diff` methods on `YaldaGpuiView`: tile lookup, bind/refresh/apply
//! (the async git → parse → join pipeline, spec § Interfaces), and the
//! per-tile key handler. The data model lives in `diff.rs`; the cached body
//! render in `diff_view.rs`. Cog node `app-diff-tile` (nd0e).

use super::*;

use std::rc::Rc;

impl YaldaGpuiView {
    /// Open a Diff tile (Cmd-D / Ctrl-Shift-D — spec-diff-review.md B1).
    /// Mirrors `open_linear` exactly: replaces the focused tile's content
    /// with a fresh, UNBOUND `App::Diff` (which renders the selector). No-op
    /// if already on a Diff tile.
    pub(crate) fn open_diff(&mut self, _: &OpenDiff, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_diff_inner(cx);
    }

    pub(crate) fn open_diff_inner(&mut self, cx: &mut Context<Self>) {
        if matches!(self.workspace.focused_content(), Some(App::Diff(_))) {
            return;
        }
        self.set_screen(App::Diff(DiffTile::new()));
        if let Some(id) = self.workspace.focused_window_id() {
            self.diff_load_worktrees(id, cx);
        }
        cx.notify();
    }

    pub(crate) fn diff_tile_ref(&self, id: workspace::WindowId) -> Option<&DiffTile> {
        match &self.workspace.tile(id)?.content {
            App::Diff(tile) => Some(tile),
            _ => None,
        }
    }

    fn diff_tile_mut(&mut self, id: workspace::WindowId) -> Option<&mut DiffTile> {
        match &mut self.workspace.tile_mut(id)?.content {
            App::Diff(tile) => Some(tile),
            _ => None,
        }
    }

    /// The live render-input fingerprint for the Diff tile at `id` (used by
    /// `DiffView`'s root-observe filter — see `diff_view.rs` module docs).
    /// `DiffSeqs::default()` for a tile that's gone / not a Diff tile — a
    /// transient state a torn-down view's next (and last) render tolerates.
    pub(crate) fn diff_seqs_for(&self, id: workspace::WindowId) -> DiffSeqs {
        match self.diff_tile_ref(id) {
            Some(tile) => DiffSeqs::of(tile, self.text_scale),
            None => DiffSeqs::default(),
        }
    }

    /// Lazily create (or return) the cached `DiffView` for the tile at `id`.
    /// Mirrors `ensure_linear_view` — `restore_content` has no `cx`, so the
    /// view is created on first render instead.
    pub(crate) fn diff_view_for(
        &mut self,
        id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Entity<DiffView> {
        let root = cx.entity();
        if let Some(tile) = self.diff_tile_mut(id)
            && let Some(v) = &tile.view
        {
            return v.clone();
        }
        let view = cx.new(|cx| DiffView::new(root, id, cx));
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.view = Some(view.clone());
        }
        view
    }

    /// Bind a Diff tile to `worktree` and kick its first derive (spec rev 2
    /// B1). The single bind path — picker Enter, row click, and `p` all land
    /// here via [`diff_picker_activate`](Self::diff_picker_activate).
    pub(crate) fn bind_diff_worktree(
        &mut self,
        id: workspace::WindowId,
        worktree: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        tile.worktree = Some(worktree);
        tile.clear_derived();
        tile.error = None;
        tile.needs_load = false;
        tile.picker = WorktreePicker::default();
        self.refresh_diff(id, cx);
        self.save_workspace_state();
        cx.notify();
    }

    /// Return a bound tile to the worktree picker and reload the list (spec
    /// B1 `space → Switch worktree`). Bumps `req` so an in-flight derive for
    /// the old worktree is discarded when it lands.
    pub(crate) fn diff_unbind(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        tile.worktree = None;
        tile.clear_derived();
        tile.error = None;
        tile.refreshing = false;
        tile.needs_load = false;
        tile.req = tile.req.wrapping_add(1);
        tile.picker = WorktreePicker::default();
        self.diff_load_worktrees(id, cx);
        self.save_workspace_state();
        cx.notify();
    }

    /// The directory whose repo the picker lists: the active workspace's cwd,
    /// falling back to the process cwd (spec B1). Also what "Pick a folder…"
    /// binds.
    fn diff_picker_dir(&self) -> PathBuf {
        self.active_workspace_cwd().unwrap_or_else(process_cwd)
    }

    /// Load the worktree list for the picker of tile `id` on the background
    /// executor (spec C2 — `git worktree list` never runs on the paint path),
    /// folding the result in via [`diff_worktrees_apply`](Self::diff_worktrees_apply).
    /// Event-handler / spawned-task only: it notifies.
    pub(crate) fn diff_load_worktrees(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let dir = self.diff_picker_dir();
        let req = {
            let Some(tile) = self.diff_tile_mut(id) else {
                return;
            };
            let p = &mut tile.picker;
            p.needs_load = false;
            p.req = p.req.wrapping_add(1);
            p.loading = true;
            p.bump();
            p.req
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { list_worktrees(&dir) })
                .await;
            let _ = this.update(cx, |this, cx| this.diff_worktrees_apply(id, req, result, cx));
        })
        .detach();
    }

    /// Fold a finished worktree-list load into the picker (discarding a stale
    /// one). Not-a-repo is a plain state, not an error (spec B1); any other
    /// failure shows inline. The folder row is always offered.
    pub(crate) fn diff_worktrees_apply(
        &mut self,
        id: workspace::WindowId,
        req: u64,
        result: Result<Vec<WorktreeEntry>, GitDiffError>,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        let p = &mut tile.picker;
        if p.req != req {
            return;
        }
        p.loading = false;
        match result {
            Ok(rows) => {
                p.rows = rows;
                p.not_a_repo = false;
                p.error = None;
            }
            Err(e) => {
                p.rows.clear();
                p.not_a_repo = e.is_not_a_repo();
                p.error = (!p.not_a_repo).then(|| e.to_string());
            }
        }
        p.selected = p.selected.min(p.folder_index());
        p.bump();
        cx.notify();
    }

    /// Activate picker row `index` of tile `id` (Enter on the selection, a row
    /// click, or `p` for the folder row). Resolves the row AT EVENT TIME from
    /// the tile's current picker (yux rule 4 — a click handler captured by a
    /// cached render carries only the index): a worktree row binds its path;
    /// the trailing folder row binds the picker directory.
    pub(crate) fn diff_picker_activate(
        &mut self,
        id: workspace::WindowId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let target = match self.diff_tile_ref(id) {
            Some(t) if t.worktree.is_none() => {
                if let Some(row) = t.picker.rows.get(index) {
                    Some(row.path.clone())
                } else if index == t.picker.folder_index() {
                    Some(self.diff_picker_dir())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(path) = target {
            self.bind_diff_worktree(id, path, cx);
        }
    }

    /// Per-frame Diff reconcile, run from the ROOT render (like
    /// `cog_reconcile_loads`): mutation-only, every load/derive is SPAWNED so
    /// its notifies land outside the draw (yux rule 1).
    ///
    /// Three duties:
    /// 1. an unbound tile whose picker list was never loaded (restored from
    ///    disk unbound) gets its `list_worktrees` load kicked;
    /// 2. a bound tile whose first derive was never kicked (restored bound)
    ///    gets its derive kicked;
    /// 3. **focus-gain refresh (spec B3, UXI-Diff-13)** — this is THE
    ///    chokepoint for "the focused tile changed": focus moves through ~20
    ///    `Workspace` mutators (motion, cycle, tab/workspace switch, click,
    ///    jump palette, …), but every one of them is followed by a root
    ///    render, so an edge detector on `focused_window_id()` here catches
    ///    all of them by construction. When the newly focused window is a
    ///    bound Diff tile, it re-derives.
    pub(crate) fn diff_reconcile(&mut self, cx: &mut Context<Self>) {
        let focused = self.workspace.focused_window_id();
        let gained = focused != self.diff_last_focused;
        self.diff_last_focused = focused;
        let mut derives: Vec<workspace::WindowId> = Vec::new();
        let mut loads: Vec<workspace::WindowId> = Vec::new();
        for wsp in self.workspace.workspaces.iter_mut() {
            wsp.for_each_attached_window_mut(&mut |w| {
                let wid = w.id();
                let App::Diff(tile) = &mut w.content else {
                    return;
                };
                if tile.worktree.is_some() {
                    let focus_gained = gained && focused == Some(wid);
                    if tile.needs_load || focus_gained {
                        tile.needs_load = false;
                        derives.push(wid);
                    }
                } else if tile.picker.needs_load {
                    tile.picker.needs_load = false;
                    loads.push(wid);
                }
            });
        }
        for wid in derives {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |v, cx| v.refresh_diff(wid, cx));
            })
            .detach();
        }
        for wid in loads {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |v, cx| v.diff_load_worktrees(wid, cx));
            })
            .detach();
        }
    }

    /// Re-derive the diff for the bound tile at `id` (spec B3): on the
    /// background executor run `collect_raw_diff` → `parse_diff`, resolve the
    /// review file (`review_path_for` shells out to git), `load_review`, and
    /// reconcile it against the new model (`recompute_outdated` + stale-Viewed
    /// pruning, saving when either changed) — none of it on the foreground
    /// thread (spec C2). The result lands via [`diff_apply`](Self::diff_apply).
    /// No-op on an unbound tile.
    pub(crate) fn refresh_diff(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(worktree) = self.diff_tile_ref(id).and_then(|t| t.worktree.clone()) else {
            return;
        };

        let (req, review_gen, saved) = {
            let Some(tile) = self.diff_tile_mut(id) else {
                return;
            };
            tile.req = tile.req.wrapping_add(1);
            tile.refreshing = true;
            tile.needs_load = false;
            (tile.req, tile.review_gen, tile.review_saved.clone())
        };
        cx.notify();

        cx.spawn(async move |this, cx| {
            let wt = worktree.clone();
            let outcome: Result<DiffDerived, GitDiffError> = cx
                .background_executor()
                .spawn(async move {
                    let raw = collect_raw_diff(wt.clone(), None).await?;
                    let model =
                        parse_diff(&raw.diff_text, wt.clone(), &raw.branch, &raw.base, &raw.merge_base);
                    let review_path = review_path_for(&wt, Some(&raw.branch));
                    let review = review_path.as_ref().map(|path| {
                        let mut review = load_review(path);
                        let stale_viewed = review.viewed.len();
                        let outdated_moved = review.recompute_outdated(&model);
                        review.prune_viewed(&model);
                        if outdated_moved || review.viewed.len() != stale_viewed {
                            if let Err(e) = save_review_latest(path, &mut review, Some(&model), review_gen, &saved) {
                                eprintln!("[yalda-gpui] review: save {} failed: {e}", path.display());
                            }
                        }
                        if review.branch.is_empty() {
                            review.branch = model.branch.clone();
                        }
                        review.base = model.base.clone();
                        review.worktree = model.worktree.clone();
                        review
                    });
                    Ok(DiffDerived {
                        model,
                        review,
                        review_path,
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.diff_apply(id, req, review_gen, outcome, cx);
            });
        })
        .detach();
    }

    /// Fold a completed derive into the tile that requested it (by stable
    /// `WindowId`), discarding a superseded (stale) request. Never panics on
    /// failure (spec B1) — an error just replaces the inline error state; the
    /// previous model stays on screen until a new one lands (spec B3).
    ///
    /// `review_gen_at_start` is the tile's `review_gen` when the derive was
    /// kicked: if the user toggled Viewed while it ran, the loaded review is
    /// older than the in-memory one, so the in-memory review is kept
    /// (reconciled against the new model) and re-persisted instead.
    ///
    /// The cursor survives (UXI-Diff-13): same file, nearest line.
    pub(crate) fn diff_apply(
        &mut self,
        id: workspace::WindowId,
        req: u64,
        review_gen_at_start: u64,
        result: Result<DiffDerived, GitDiffError>,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if tile.req != req {
            return;
        }
        tile.refreshing = false;
        let mut persist = false;
        match result {
            Ok(derived) => {
                let anchor = tile.cursor_anchor();
                let old_cursor = tile.cursor;
                let model = Rc::new(derived.model);
                let edited_meanwhile = tile.review_gen != review_gen_at_start
                    && tile.review.is_some()
                    && tile.review_path == derived.review_path;
                if edited_meanwhile {
                    if let Some(r) = tile.review.as_mut() {
                        r.recompute_outdated(&model);
                        r.prune_viewed(&model);
                    }
                    persist = true;
                } else {
                    tile.review = derived.review;
                    tile.review_path = derived.review_path;
                }
                tile.review_gen = tile.review_gen.wrapping_add(1);
                tile.model = Some(model.clone());
                tile.error = None;
                tile.model_gen = tile.model_gen.wrapping_add(1);
                tile.rebuild_rows();
                tile.cursor = anchor
                    .map(|a| resolve_anchor(&model, &tile.rows, &a, old_cursor))
                    .unwrap_or(0);
            }
            Err(e) => {
                tile.error = Some(e.to_string());
            }
        }
        if persist {
            self.diff_persist_review(id, cx);
        }
        cx.notify();
    }

    /// Write the tile's review on the background executor (spec C2 / UXI-Diff-17
    /// — never on the render path; atomic tmp + rename inside `save_review`).
    /// Serialized + generation-guarded by `save_review_latest`, so rapid
    /// toggles can't land out of order. No-op without a resolved review path.
    pub(crate) fn diff_persist_review(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_ref(id) else {
            return;
        };
        let (Some(path), Some(review)) = (tile.review_path.clone(), tile.review.clone()) else {
            return;
        };
        let save_gen = tile.review_gen;
        let saved = tile.review_saved.clone();
        cx.background_executor()
            .spawn(async move {
                let mut review = review;
                if let Err(e) = save_review_latest(&path, &mut review, None, save_gen, &saved) {
                    eprintln!("[yalda-gpui] review: save {} failed: {e}", path.display());
                }
            })
            .detach();
    }

    /// `v` / the checkbox (spec B4, UXI-Diff-14): toggle Viewed on the cursor's
    /// file, fold/advance per `DiffTile::toggle_viewed_at_cursor`, persist async.
    pub(crate) fn toggle_file_viewed(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if tile.toggle_viewed_at_cursor().is_none() {
            return;
        }
        self.diff_persist_review(id, cx);
        cx.notify();
    }

    /// A click on body row `index` (spec B2): move the cursor there. The index
    /// is resolved against the tile's CURRENT rows at event time (yux rule 4).
    pub(crate) fn diff_click_row(&mut self, id: workspace::WindowId, index: usize, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if index < tile.rows.len() {
            tile.set_cursor(index);
            cx.notify();
        }
    }

    /// A click on the Viewed checkbox of file-header row `index` (spec B4):
    /// cursor to that header, then the same toggle as `v`.
    pub(crate) fn diff_click_checkbox(
        &mut self,
        id: workspace::WindowId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if !tile.rows.get(index).is_some_and(RowRef::is_file) {
            return;
        }
        tile.set_cursor(index);
        self.toggle_file_viewed(id, cx);
    }

    /// `r` / the tile-menu "refresh" verb (spec B3 manual refresh) for the
    /// FOCUSED Diff tile.
    pub(crate) fn diff_refresh_focused(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        if !matches!(self.workspace.focused_content(), Some(App::Diff(_))) {
            return;
        }
        self.refresh_diff(id, cx);
    }

    /// Tile-menu "Switch worktree" verb (spec B1/B8) — return the focused Diff
    /// tile to the worktree picker and reload the list.
    pub(crate) fn diff_switch_worktree_focused(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        if !matches!(self.workspace.focused_content(), Some(App::Diff(_))) {
            return;
        }
        self.diff_unbind(id, cx);
    }

    /// Key handler for a focused Diff tile. Unbound ⇒ the worktree picker
    /// (spec B1: `j`/`k`/arrows move, Enter binds the selection, `p` binds
    /// the folder row); bound ⇒ diff navigation + comments (spec B5).
    ///
    /// The comment compose is checked FIRST (before the platform/control bail
    /// and `leader_intercept`) so Ctrl-/Cmd-Enter reach it;
    /// `focused_in_insert_mode` keys off `tile.compose`, so the leaders are
    /// suppressed while composing.
    pub(crate) fn handle_diff_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        if self.diff_tile_ref(id).is_some_and(|t| t.compose.is_some()) {
            if is_ctrl_w_shell_prefix(&press) {
                return;
            }
            self.handle_diff_comment_key(id, press, cx);
            return;
        }
        // spec B6: the send picker owns every key while open (typing → query,
        // ctrl-n/ctrl-p move) — so it too precedes the ctrl/cmd bail and the
        // leaders (`focused_in_insert_mode` keys off `tile.send_picker`).
        if self.diff_tile_ref(id).is_some_and(|t| t.send_picker.is_some()) {
            if is_ctrl_w_shell_prefix(&press) {
                return;
            }
            self.handle_diff_send_picker_key(id, press, cx);
            return;
        }
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        if self.leader_intercept(&press, cx) {
            return;
        }
        let unbound = matches!(self.diff_tile_ref(id), Some(t) if t.worktree.is_none());
        if unbound {
            match press.key {
                Key::Char('j') | Key::Down => {
                    if let Some(tile) = self.diff_tile_mut(id) {
                        tile.picker.move_selection(1);
                    }
                    cx.notify();
                }
                Key::Char('k') | Key::Up => {
                    if let Some(tile) = self.diff_tile_mut(id) {
                        tile.picker.move_selection(-1);
                    }
                    cx.notify();
                }
                Key::Enter => {
                    if let Some(sel) = self.diff_tile_ref(id).map(|t| t.picker.selected) {
                        self.diff_picker_activate(id, sel, cx);
                    }
                }
                Key::Char('p') => {
                    if let Some(folder) = self.diff_tile_ref(id).map(|t| t.picker.folder_index()) {
                        self.diff_picker_activate(id, folder, cx);
                    }
                }
                _ => {}
            }
            return;
        }
        // Every bound-tile key replaces the previous one-shot hint, and any
        // key but a second `x` disarms a pending delete (spec C6).
        let had_status = self.transient_status.take().is_some();
        if press.key != Key::Char('x')
            && let Some(tile) = self.diff_tile_mut(id)
        {
            tile.pending_delete = None;
        }
        // j/k extend an active `V` range (clamped to its file); every other
        // navigation key drops the range first.
        let nav: Option<(fn(&mut DiffTile), bool)> = match press.key {
            Key::Char('j') | Key::Down => Some((|t| t.move_cursor(1), true)),
            Key::Char('k') | Key::Up => Some((|t| t.move_cursor(-1), true)),
            Key::Char('}') => Some((|t| t.jump_hunk(true), false)),
            Key::Char('{') => Some((|t| t.jump_hunk(false), false)),
            Key::Char(']') => Some((|t| t.jump_file(true), false)),
            Key::Char('[') => Some((|t| t.jump_file(false), false)),
            Key::Char('G') => Some((|t| t.cursor_to_end(), false)),
            Key::Char('z') => Some((|t| t.toggle_fold_at_cursor(), false)),
            _ => None,
        };
        if let Some((nav, keeps_range)) = nav {
            if let Some(tile) = self.diff_tile_mut(id) {
                if !keeps_range {
                    tile.range_anchor = None;
                }
                nav(tile);
            }
            cx.notify();
            return;
        }
        match press.key {
            Key::Char('r') => self.refresh_diff(id, cx),
            Key::Char('v') => self.toggle_file_viewed(id, cx),
            Key::Char('o') => self.open_in_zed(id, cx),
            Key::Char('V') => self.diff_toggle_range(id, cx),
            Key::Char('c') => self.open_comment_compose(id, cx),
            Key::Char('e') => self.edit_comment_at_cursor(id, cx),
            Key::Char('x') => self.delete_comment_at_cursor(id, cx),
            Key::Char('s') => self.open_send_picker(id, false, cx),
            Key::Char('S') => self.open_send_picker(id, true, cx),
            Key::Esc => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.range_anchor = None;
                }
                cx.notify();
            }
            _ => {
                if had_status {
                    cx.notify();
                }
            }
        }
    }

    // ── Cog graph 8g7 node `comments-ui`: spec B5, UXI-Diff-15 ─────────────

    /// A one-shot hint in the status toast (spec C6: every refusal explains).
    fn diff_hint(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.transient_status = Some(msg.into());
        cx.notify();
    }

    /// `V`: start a line-range selection at the cursor, or clear the active
    /// one. On a non-line row it is a no-op with a hint.
    pub(crate) fn diff_toggle_range(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if tile.toggle_range() {
            cx.notify();
        } else {
            self.diff_hint("V starts a range on a diff line — move to a line first", cx);
        }
    }

    /// `c` (spec B5): open the comment compose anchored on the active `V`
    /// range, else the cursor's line. Anywhere else: a hint.
    pub(crate) fn open_comment_compose(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if tile.model.is_none() {
            return;
        }
        match tile.comment_anchor_at_cursor() {
            Some(anchor) => {
                tile.open_compose(ComposeTarget::New, anchor, "");
                cx.notify();
            }
            None if tile.cursor_comment().is_some() => {
                self.diff_hint("e edits this comment · x deletes it", cx)
            }
            None => self.diff_hint("c comments on a diff line — move to a line (or V to select a range)", cx),
        }
    }

    /// `e` on any row of a comment card: reopen the compose prefilled with
    /// its body; saving replaces the body.
    pub(crate) fn edit_comment_at_cursor(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        let Some(c) = tile.cursor_comment().cloned() else {
            self.diff_hint("e edits the comment under the cursor — move onto a comment card", cx);
            return;
        };
        tile.open_compose(ComposeTarget::Edit(c.id.clone()), CommentAnchor::of_comment(&c), &c.body);
        cx.notify();
    }

    /// `x` on a comment card: first press arms ("x again to delete c3"),
    /// second deletes + persists.
    pub(crate) fn delete_comment_at_cursor(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        match tile.delete_at_cursor() {
            DeleteOutcome::NotOnComment => {
                self.diff_hint("x deletes the comment under the cursor — move onto a comment card", cx)
            }
            DeleteOutcome::Armed(cid) => self.diff_hint(format!("x again to delete {cid}"), cx),
            DeleteOutcome::Deleted(cid) => {
                self.diff_persist_review(id, cx);
                self.diff_hint(format!("Deleted {cid}"), cx);
            }
        }
    }

    /// Ctrl-/Cmd-Enter in the compose: save (spec B5) — written to the
    /// review file immediately as unsent (async, off the render path).
    pub(crate) fn submit_comment(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        match tile.save_compose(chrono::Utc::now()) {
            Ok(cid) => {
                self.diff_persist_review(id, cx);
                self.diff_hint(format!("Saved {cid} (unsent)"), cx);
            }
            Err(msg) => self.diff_hint(msg, cx),
        }
    }

    /// Key dispatch while the comment compose is open. Ctrl-Enter / Cmd-Enter
    /// save; Esc closes an empty draft, and on a non-empty one arms first
    /// ("Esc again to discard") so a stray Esc can't lose text; everything
    /// else is typing through the shared insert core (`dispatch_insert_core`
    /// — Enter = newline, like the agent compose).
    fn handle_diff_comment_key(&mut self, id: workspace::WindowId, press: KeyPress, cx: &mut Context<Self>) {
        self.transient_status = None;
        let enter_save = press.key == Key::Enter
            && (press.modifiers.contains(KMods::CONTROL) || press.modifiers.contains(KMods::PLATFORM));
        if enter_save {
            self.submit_comment(id, cx);
            return;
        }
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        let Some(compose) = tile.compose.as_mut() else {
            return;
        };
        if press.key == Key::Esc && press.modifiers.is_empty() {
            if compose.input.text().trim().is_empty() || compose.esc_armed {
                tile.close_compose();
                cx.notify();
            } else {
                compose.esc_armed = true;
                self.diff_hint("Esc again to discard this comment · ctrl-enter saves", cx);
            }
            return;
        }
        compose.esc_armed = false;
        Self::dispatch_insert_core(&mut compose.input.editor, &mut compose.input.mode, press);
        cx.notify();
    }

    // ── Cog graph 8g7 node `send-picker`: spec B6, UXI-Diff-16 ──────────────

    /// The session key recorded for a LOCAL session: its server sid, or
    /// `local-<n>` for a session with no server sid (never attached / tests).
    pub(crate) fn send_key_for_local(&self, id: SessionId) -> String {
        self.sessions
            .sid_of(id)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| format!("local-{}", id.0))
    }

    /// A short secondary label for a session's cwd: its project's name, else
    /// the directory name.
    fn send_detail_for_cwd(&self, cwd: &std::path::Path) -> String {
        match self.projects.by_cwd(cwd) {
            Some(p) => self.projects.name_of(p).to_string(),
            None => cwd
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| cwd.display().to_string()),
        }
    }

    fn send_local_candidate(&self, id: SessionId, detail: Option<String>, cx: &gpui::App) -> Option<SendCandidate> {
        let ent = self.sessions.get(id)?;
        let s = ent.read(cx);
        let status = if s.state.turn_phase.is_awaiting() {
            AgentDotStatus::Working
        } else {
            AgentDotStatus::WaitingForYou
        };
        Some(SendCandidate {
            item: PaletteItem {
                target: SendTarget::Local(id),
                label: s.label.clone(),
                detail: detail.unwrap_or_else(|| self.send_detail_for_cwd(&s.cwd)),
                is_agent: true,
                status: Some(status),
                active: false,
            },
            key: self.send_key_for_local(id),
        })
    }

    /// Every session the send picker offers (spec B6), in the jump palette's
    /// order: the agent tiles `Cmd-P` lists (live, loaded sessions first),
    /// then sessions loaded in the store that no tile binds, then every other
    /// server-known session in the universal roster (by label). Archived
    /// sessions are hidden, as in the palette and selector.
    pub(crate) fn send_picker_candidates(&self, cx: &gpui::App) -> Vec<SendCandidate> {
        let mut out: Vec<SendCandidate> = Vec::new();
        let mut seen_local: std::collections::HashSet<SessionId> = Default::default();
        let mut seen_sid: std::collections::HashSet<String> = Default::default();
        let archived = |sid: &str| {
            self.jump_archived_sessions.contains(sid)
                || self.agent_roster.get(sid).is_some_and(|i| i.archived)
        };
        let push_local = |this: &Self,
                              out: &mut Vec<SendCandidate>,
                              seen_local: &mut std::collections::HashSet<SessionId>,
                              seen_sid: &mut std::collections::HashSet<String>,
                              id: SessionId,
                              detail: Option<String>| {
            if !seen_local.insert(id) {
                return;
            }
            if let Some(sid) = this.sessions.sid_of(id) {
                seen_sid.insert(sid.as_str().to_string());
            }
            if let Some(c) = this.send_local_candidate(id, detail, cx) {
                out.push(c);
            }
        };
        for item in self.jump_palette_items(cx) {
            let PaletteTarget::Tile(wid) = item.target else {
                continue;
            };
            let Some(App::Agent(tile)) = self.workspace.tile(wid).map(|w| &w.content) else {
                continue;
            };
            if let Some(local) = tile.session() {
                push_local(self, &mut out, &mut seen_local, &mut seen_sid, local, Some(item.detail.clone()));
            } else if let Some(sid) = tile.remembered_sid(|l| self.sessions.sid_of(l).cloned()) {
                let sid = sid.as_str().to_string();
                if !archived(&sid) && seen_sid.insert(sid.clone()) {
                    out.push(SendCandidate {
                        item: PaletteItem {
                            target: SendTarget::Server(sid.clone()),
                            label: item.label.clone(),
                            detail: item.detail.clone(),
                            is_agent: true,
                            status: item.status,
                            active: false,
                        },
                        key: sid,
                    });
                }
            }
        }
        let store_ids: Vec<SessionId> = self.sessions.iter().map(|(id, _)| id).collect();
        for id in store_ids {
            if self.sessions.sid_of(id).is_some_and(|sid| archived(sid.as_str())) {
                continue;
            }
            push_local(self, &mut out, &mut seen_local, &mut seen_sid, id, None);
        }
        for info in self.agent_roster.entries_by_label() {
            let sid = info.session_id.clone();
            if archived(&sid) || seen_sid.contains(&sid) {
                continue;
            }
            if let Some(local) = self.sessions.locate(&ServerSid::new(sid.clone())) {
                push_local(self, &mut out, &mut seen_local, &mut seen_sid, local, None);
                continue;
            }
            seen_sid.insert(sid.clone());
            let status = if !info.connected {
                AgentDotStatus::Neutral
            } else if info.busy {
                AgentDotStatus::Working
            } else {
                AgentDotStatus::WaitingForYou
            };
            out.push(SendCandidate {
                item: PaletteItem {
                    target: SendTarget::Server(sid.clone()),
                    label: info.label.clone(),
                    detail: self.send_detail_for_cwd(&info.cwd),
                    is_agent: true,
                    status: Some(status),
                    active: false,
                },
                key: sid,
            });
        }
        out
    }

    /// `s` (`include_sent = false`: the unsent comments) / `S` (every
    /// comment) / `space → send comments…`: open the send picker (spec B6).
    /// Nothing to send ⇒ no picker, a status hint. No sessions ⇒ a hint.
    pub(crate) fn open_send_picker(&mut self, id: workspace::WindowId, include_sent: bool, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_ref(id) else {
            return;
        };
        if tile.worktree.is_none() || tile.compose.is_some() {
            return;
        }
        let (ids, last_sent) = match &tile.review {
            Some(r) => (
                if include_sent { r.all_comment_ids() } else { r.unsent_ids() },
                r.last_sent_session.clone(),
            ),
            None => (Vec::new(), None),
        };
        if ids.is_empty() {
            self.diff_hint(if include_sent { "No comments." } else { "No unsent comments." }, cx);
            return;
        }
        let candidates = self.send_picker_candidates(cx);
        if candidates.is_empty() {
            self.diff_hint("No agent sessions to send to — start one first.", cx);
            return;
        }
        let picker = SendPicker::new(ids, candidates, last_sent.as_deref());
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.range_anchor = None;
            tile.send_picker = Some(picker);
        }
        self.transient_status = None;
        cx.notify();
    }

    /// Tile-menu "send comments…" verb (spec B8) for the FOCUSED Diff tile.
    pub(crate) fn diff_send_comments_focused(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        if !matches!(self.workspace.focused_content(), Some(App::Diff(_))) {
            return;
        }
        self.open_send_picker(id, false, cx);
    }

    /// Key dispatch while the send picker is open: Esc closes; Enter sends to
    /// the highlighted row; ↑/↓ and ctrl-p/ctrl-n move (j/k are query
    /// letters); Backspace/characters edit the fuzzy query.
    fn handle_diff_send_picker_key(&mut self, id: workspace::WindowId, press: KeyPress, cx: &mut Context<Self>) {
        let ctrl = press.modifiers.contains(KMods::CONTROL);
        let Some(picker) = self.diff_tile_mut(id).and_then(|t| t.send_picker.as_mut()) else {
            return;
        };
        match press.key {
            Key::Esc => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.send_picker = None;
                }
            }
            Key::Enter => {
                let row = picker.selected;
                self.send_picker_activate(id, row, cx);
                return;
            }
            Key::Down => picker.move_selection(1),
            Key::Up => picker.move_selection(-1),
            Key::Char('n') if ctrl => picker.move_selection(1),
            Key::Char('p') if ctrl => picker.move_selection(-1),
            _ => {
                if !picker.query_key(&press).handled() {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// Hover over display row `row` of the open send picker.
    pub(crate) fn send_picker_hover(&mut self, id: workspace::WindowId, row: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.diff_tile_mut(id).and_then(|t| t.send_picker.as_mut())
            && p.selected != row
        {
            p.selected = row;
            cx.notify();
        }
    }

    /// Enter / a row click: send to display row `row` (resolved at event time
    /// against the picker's current ranking). A no-match query is a no-op
    /// that leaves the picker open (the jump palette's rule). The picker
    /// closes; the outcome lands in the status line.
    pub(crate) fn send_picker_activate(&mut self, id: workspace::WindowId, row: usize, cx: &mut Context<Self>) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        let Some(picker) = tile.send_picker.as_mut() else {
            return;
        };
        picker.selected = row;
        let Some(idx) = picker.selected_item() else {
            return;
        };
        let target = picker.items[idx].target.clone();
        let label = picker.items[idx].label.clone();
        let key = picker.keys[idx].clone();
        let ids = picker.ids.clone();
        tile.send_picker = None;
        self.send_review_comments(id, target, key, label, ids, cx);
    }

    /// Deliver `ids` of tile `id`'s review to `target` (spec B6). The review
    /// file is written FIRST (on the background executor — never on the
    /// render path) so the agent can read every named comment; only after
    /// that write succeeds is the prompt delivered, and only a successful
    /// delivery records `sent` entries + `last_sent_session`.
    pub(crate) fn send_review_comments(
        &mut self,
        id: workspace::WindowId,
        target: SendTarget,
        key: String,
        label: String,
        ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_ref(id) else {
            return;
        };
        let (Some(path), Some(review)) = (tile.review_path.clone(), tile.review.clone()) else {
            self.diff_hint("Send failed: this worktree's review file location couldn't be resolved.", cx);
            return;
        };
        let path = std::path::absolute(&path).unwrap_or(path);
        let prompt = build_send_prompt(&review, &path, &ids);
        let save_gen = tile.review_gen;
        let saved = tile.review_saved.clone();
        self.diff_hint(format!("Sending {} to {label}…", comments_phrase(ids.len())), cx);
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let mut review = review;
                    save_review_latest(&path, &mut review, None, save_gen, &saved).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |v, cx| v.diff_send_after_save(id, target, key, label, ids, prompt, written, cx));
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn diff_send_after_save(
        &mut self,
        id: workspace::WindowId,
        target: SendTarget,
        key: String,
        label: String,
        ids: Vec<String>,
        prompt: String,
        written: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = written {
            self.diff_hint(format!("Send failed: couldn't write the review file: {e}"), cx);
            return;
        }
        if let Err(reason) = self.deliver_to_send_target(&target, &prompt, cx) {
            self.diff_hint(format!("Send failed: {reason}"), cx);
            return;
        }
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if let Some(review) = tile.review.as_mut() {
            review.record_sent(&ids, &key, &label, chrono::Utc::now());
        }
        tile.review_gen = tile.review_gen.wrapping_add(1);
        tile.rebuild_rows();
        self.diff_persist_review(id, cx);
        self.diff_hint(format!("Sent {} to {label}.", comments_phrase(ids.len())), cx);
    }

    /// Deliver one prompt to a send-picker target WITHOUT attaching, binding
    /// or focusing it. A session loaded here goes through the normal
    /// `send_prompt_to_session` (transcript echo + turn start, same Codex
    /// steer rule as a compose submit); a roster-only session is prompted by
    /// server sid (the server's prompt is not owner-gated).
    fn deliver_to_send_target(&mut self, target: &SendTarget, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let local = match target {
            SendTarget::Local(id) => Some(*id),
            SendTarget::Server(sid) => self.sessions.locate(&ServerSid::new(sid.clone())),
        };
        if let Some(id) = local {
            if self.sessions.get(id).is_none() {
                return Err("that session is closed".to_string());
            }
            let steer_codex = self
                .read_session(id, cx, |c| {
                    c.provider == AgentProvider::Codex && matches!(c.turn_phase, TurnPhase::Awaiting { .. })
                })
                .unwrap_or(false);
            return if self.send_prompt_to_session(id, text, &[], None, steer_codex, cx) {
                Ok(())
            } else {
                Err("the session isn't connected".to_string())
            };
        }
        let SendTarget::Server(sid) = target else {
            return Err("that session is closed".to_string());
        };
        let Some(server) = self.session_server.as_ref() else {
            return Err("the session server isn't connected".to_string());
        };
        let steer = self
            .agent_roster
            .get(sid)
            .is_some_and(|i| i.provider == AgentProvider::Codex && i.busy);
        let r = if steer {
            server.steer_with_images(sid, text, Vec::new())
        } else {
            server.prompt_with_images(sid, text, Vec::new())
        };
        r.map_err(|e| e.to_string())
    }

    // ── Cog node `open-in-zed` (oc72): spec B8 ──────────────────────────────

    /// `o` (spec B7 "Open in Zed"): spawn `zed <abs-path>:<line>` for the
    /// cursor row, fire-and-forget — the child is spawned and immediately
    /// detached, so a slow or hung `zed` can never block the UI thread. The
    /// line is [`zed_target`]'s (a Line row's new-side number; a hunk/file
    /// header's first new line). A missing/failing `zed` binary surfaces a
    /// `transient_status` hint instead of panicking; the binary name is read
    /// through `zed_bin()` so a test always exercises that branch.
    pub(crate) fn open_in_zed(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let target = self.diff_tile_ref(id).and_then(|t| {
            let worktree = t.worktree.clone()?;
            let (rel, line) = zed_target(t.model.as_ref()?, &t.rows, t.cursor)?;
            Some((worktree, rel, line))
        });
        let Some((worktree, rel_path, line)) = target else {
            return;
        };
        let arg = zed_open_arg(&worktree, &rel_path, line);
        if let Err(e) = std::process::Command::new(zed_bin()).arg(&arg).spawn() {
            self.transient_status = Some(format!("couldn't open zed: {e}").into());
            cx.notify();
        }
    }
}

/// The `zed` binary name (spec B7). Test-only seam: under `cfg(test)` this is
/// always a deliberately-bogus name so no test can ever launch the real
/// editor (even one installed on the machine running the test) — every test
/// of `open_in_zed` therefore exercises the missing-binary/error branch
/// by construction, no environment-dependent skip needed.
fn zed_bin() -> &'static str {
    #[cfg(test)]
    {
        "yalda-test-nonexistent-zed-binary"
    }
    #[cfg(not(test))]
    {
        "zed"
    }
}
