//! `App::Diff` methods on `YaldaGpuiView`: tile lookup, bind/refresh/apply
//! (the async git → parse → join pipeline, spec § Interfaces), and the
//! per-tile key handler. The data model lives in `diff.rs`; the cached body
//! render in `diff_view.rs`. Cog node `app-diff-tile` (nd0e).

use super::*;

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
        tile.model = None;
        tile.error = None;
        tile.needs_load = false;
        tile.focus = DiffFocus::default();
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
        tile.model = None;
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

    /// Re-derive the diff for the bound tile at `id`: run `collect_raw_diff`
    /// → `parse_diff` → `resolve_git_common_dir`/`load_review_state` →
    /// `join_reviewed_flags` entirely on the background executor (spec C2 —
    /// no git subprocess or review I/O on the foreground thread), swapping the
    /// result in via `diff_apply`. No-op on an unbound tile.
    pub(crate) fn refresh_diff(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(worktree) = self.diff_tile_ref(id).and_then(|t| t.worktree.clone()) else {
            return;
        };

        let req = {
            let Some(tile) = self.diff_tile_mut(id) else {
                return;
            };
            tile.req = tile.req.wrapping_add(1);
            tile.refreshing = true;
            tile.needs_load = false;
            tile.req
        };
        cx.notify();

        cx.spawn(async move |this, cx| {
            let wt = worktree.clone();
            let outcome: Result<DiffModel, GitDiffError> = cx
                .background_executor()
                .spawn(async move {
                    let raw = collect_raw_diff(wt.clone(), None).await?;
                    let mut model =
                        parse_diff(&raw.diff_text, wt.clone(), &raw.branch, &raw.base, &raw.merge_base);
                    if let Some(common) = resolve_git_common_dir(&wt) {
                        let state = load_review_state(&common, &raw.branch);
                        join_reviewed_flags(&mut model, &state);
                    }
                    Ok(model)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.diff_apply(id, req, outcome, cx);
            });
        })
        .detach();
    }

    /// Fold a completed derive into the tile that requested it (by stable
    /// `WindowId`), discarding a superseded (stale) request (spec §
    /// Interfaces). Never panics on failure (spec B1) — an error just
    /// replaces the inline error state; the previous model (if any) is
    /// dropped only on success, per spec B3 "the previous model stays on
    /// screen until the new one lands".
    pub(crate) fn diff_apply(
        &mut self,
        id: workspace::WindowId,
        req: u64,
        result: Result<DiffModel, GitDiffError>,
        cx: &mut Context<Self>,
    ) {
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        if tile.req != req {
            return;
        }
        tile.refreshing = false;
        match result {
            Ok(model) => {
                let prev_hash = tile.focused_hunk_hash();
                tile.model = Some(model);
                tile.error = None;
                tile.model_gen = tile.model_gen.wrapping_add(1);
                tile.restore_focus_by_hash(prev_hash);
            }
            Err(e) => {
                tile.error = Some(e.to_string());
            }
        }
        cx.notify();
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

    /// Toggle the focused hunk's reviewed state (spec B5 `v`). Flips the
    /// in-memory `DiffModel`'s hunk immediately (so the cached `DiffView`
    /// re-renders off the `model_gen` bump — no new `DiffSeqs` field needed,
    /// since a mark IS a model mutation, same bump path a refresh uses) and
    /// persists the flip to `ReviewState` on the background executor (spec
    /// C2 — `ReviewState` I/O never runs on the render path; this is an event
    /// handler, not render, but the actual git/file I/O still moves off the
    /// foreground thread to match `refresh_diff`'s discipline).
    pub(crate) fn toggle_hunk_reviewed(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(hash) = self.diff_tile_ref(id).and_then(|t| t.focused_hunk_hash()) else {
            return;
        };
        let target = self
            .diff_tile_ref(id)
            .and_then(|t| t.model.as_ref())
            .and_then(|m| {
                m.files
                    .iter()
                    .flat_map(|f| f.hunks.iter())
                    .find(|h| h.hunk_hash == hash)
            })
            .map(|h| !h.reviewed);
        let Some(mark) = target else { return };
        self.set_hunks_reviewed(id, &[hash], mark, cx);
    }

    /// File-level "mark all" (spec B5, bound to `V`/shift-v): marks every
    /// hunk in the focused file reviewed. Unlike the single-hunk `v` toggle,
    /// this always marks true — a blunt "I've reviewed this whole file" act,
    /// not a per-hunk flip (a mixed file would otherwise have no well-defined
    /// toggle direction).
    pub(crate) fn mark_file_reviewed(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(hashes) = self.diff_tile_ref(id).and_then(|t| {
            let model = t.model.as_ref()?;
            let file = model.files.get(t.focus.file)?;
            Some(file.hunks.iter().map(|h| h.hunk_hash).collect::<Vec<_>>())
        }) else {
            return;
        };
        self.set_hunks_reviewed(id, &hashes, true, cx);
    }

    /// Shared core for `toggle_hunk_reviewed` / `mark_file_reviewed`: sets
    /// every hash in `hashes` to reviewed = `mark` in BOTH the in-memory
    /// `DiffModel` (so the view reflects it this frame, via a `model_gen`
    /// bump) and the persisted `ReviewState` (spec B5: "Marks persist in
    /// `ReviewState` across restarts"). The persistence half runs on the
    /// background executor — `resolve_git_common_dir` shells out to git, and
    /// `save_review_state` does file I/O, both of which must stay off the
    /// paint path (spec C2) even though this is only called from a key
    /// handler.
    fn set_hunks_reviewed(
        &mut self,
        id: workspace::WindowId,
        hashes: &[u64],
        mark: bool,
        cx: &mut Context<Self>,
    ) {
        if hashes.is_empty() {
            return;
        }
        let Some(tile) = self.diff_tile_mut(id) else {
            return;
        };
        let Some(model) = &mut tile.model else {
            return;
        };
        let hash_set: HashSet<u64> = hashes.iter().copied().collect();
        for file in &mut model.files {
            for hunk in &mut file.hunks {
                if hash_set.contains(&hunk.hunk_hash) {
                    hunk.reviewed = mark;
                }
            }
        }
        tile.model_gen = tile.model_gen.wrapping_add(1);
        let worktree = model.worktree.clone();
        let branch = model.branch.clone();
        let model_snapshot = model.clone();
        let hashes = hashes.to_vec();
        cx.notify();

        cx.background_executor()
            .spawn(async move {
                let Some(common) = resolve_git_common_dir(&worktree) else {
                    return;
                };
                let mut state = load_review_state(&common, &branch);
                for h in &hashes {
                    if mark {
                        state.mark_reviewed(*h);
                    } else {
                        state.mark_unreviewed(*h);
                    }
                }
                save_review_state(&common, &branch, &mut state, &model_snapshot);
            })
            .detach();
    }

    /// Key handler for a focused Diff tile. Unbound ⇒ the worktree picker
    /// (spec B1: `j`/`k`/arrows move, Enter binds the selection, `p` binds
    /// the folder row); bound ⇒ diff navigation.
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
        match press.key {
            Key::Char('j') | Key::Down => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.move_hunk_focus(1);
                }
                cx.notify();
            }
            Key::Char('k') | Key::Up => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.move_hunk_focus(-1);
                }
                cx.notify();
            }
            Key::Char(']') => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.jump_file(1);
                }
                cx.notify();
            }
            Key::Char('[') => {
                if let Some(tile) = self.diff_tile_mut(id) {
                    tile.jump_file(-1);
                }
                cx.notify();
            }
            Key::Char('z') => {
                let path = self.diff_tile_ref(id).and_then(|t| {
                    let m = t.model.as_ref()?;
                    m.files.get(t.focus.file).map(|f| f.path.clone())
                });
                if let Some(path) = path
                    && let Some(tile) = self.diff_tile_mut(id)
                {
                    tile.toggle_collapsed(&path);
                }
                cx.notify();
            }
            Key::Char('r') => self.refresh_diff(id, cx),
            Key::Char('v') => self.toggle_hunk_reviewed(id, cx),
            Key::Char('V') => self.mark_file_reviewed(id, cx),
            Key::Char('o') => self.open_hunk_in_zed(id, cx),
            _ => {}
        }
    }

    // ── Cog node `open-in-zed` (oc72): spec B8 ──────────────────────────────

    /// `o` on a focused hunk (spec B8 "Open in Zed"): spawn
    /// `zed <abs-path>:<first-new-line>`, fire-and-forget — the child is
    /// spawned and immediately detached (no wait, no captured output), so a
    /// slow or hung `zed` can never block the UI thread. `abs-path` is the
    /// bound worktree joined with the focused file's repo-relative path;
    /// `first-new-line` is the hunk's first added/new line
    /// (`Hunk::new_line_range().0`) — the pure composition of the two lives
    /// in `zed_open_arg` (`diff.rs`) so it's unit-tested without spawning. A
    /// missing/failing `zed` binary surfaces a `transient_status` hint
    /// instead of panicking (spec B8, DONE_WHEN #2); the binary name is read
    /// through `zed_bin()` below so a test always exercises that branch
    /// rather than depending on whether the real editor happens to be
    /// installed on the machine running the test.
    pub(crate) fn open_hunk_in_zed(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let target = self.diff_tile_ref(id).and_then(|t| {
            let worktree = t.worktree.clone()?;
            let model = t.model.as_ref()?;
            let file = model.files.get(t.focus.file)?;
            let hunk = file.hunks.get(t.focus.hunk)?;
            Some((worktree, file.path.clone(), hunk.new_line_range().0))
        });
        let Some((worktree, rel_path, first_new_line)) = target else {
            return;
        };
        let arg = zed_open_arg(&worktree, &rel_path, first_new_line);
        if let Err(e) = std::process::Command::new(zed_bin()).arg(&arg).spawn() {
            self.transient_status = Some(format!("couldn't open zed: {e}").into());
            cx.notify();
        }
    }
}

/// The `zed` binary name (spec B8). Test-only seam: under `cfg(test)` this is
/// always a deliberately-bogus name so no test can ever launch the real
/// editor (even one installed on the machine running the test) — every test
/// of `open_hunk_in_zed` therefore exercises the missing-binary/error branch
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
