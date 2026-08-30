//! `App::Diff` methods on `YaldaGpuiView`: tile lookup, bind/refresh/apply
//! (the async git → parse → join pipeline, spec § Interfaces), and the
//! per-tile key handler. The data model lives in `diff.rs`; the cached body
//! render in `diff_view.rs`. Cog node `app-diff-tile` (nd0e).

use super::*;

impl YaldaGpuiView {
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
            Some(tile) => DiffSeqs::of(tile, self.sessions.ids().count(), self.text_scale),
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

    /// Bind (or rebind) a Diff tile's source and kick a refresh (spec B1).
    pub(crate) fn bind_diff_source(
        &mut self,
        id: workspace::WindowId,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.source = Some(source);
            tile.model = None;
            tile.error = None;
            tile.needs_load = false;
        }
        self.refresh_diff(id, cx);
        self.save_workspace_state();
        cx.notify();
    }

    /// Return a bound tile to the selector (spec B9 "bind" verb — pick a
    /// different session/path).
    pub(crate) fn diff_unbind(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.source = None;
            tile.model = None;
            tile.error = None;
            tile.needs_load = false;
        }
        cx.notify();
    }

    /// Re-derive the diff for the tile at `id`: resolve the worktree (from
    /// the bound session's `cwd`, cheap and `cx`-only, or the explicit
    /// `Path`), then run `collect_raw_diff` → `parse_diff` →
    /// `resolve_git_common_dir`/`load_review_state` → `join_reviewed_flags`
    /// entirely on the background executor (spec C2 — no git subprocess or
    /// `ReviewState` I/O on the foreground thread, let alone the render
    /// path), swapping the result in via `diff_apply` (spec § Interfaces).
    pub(crate) fn refresh_diff(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let Some(source) = self.diff_tile_mut(id).and_then(|t| t.source.clone()) else {
            return;
        };
        let worktree = match &source {
            DiffSource::Path(p) => Some(p.clone()),
            DiffSource::Session(sid) => match self.sessions.get(*sid) {
                Some(ent) => Some(ent.read(cx).cwd.clone()),
                None => {
                    // Spec B1: the session closed — fall back to Path binding
                    // on the last-known worktree rather than closing the tile.
                    let fallback = self.diff_tile_mut(id).and_then(|t| t.worktree());
                    if let Some(wt) = fallback.clone()
                        && let Some(tile) = self.diff_tile_mut(id)
                    {
                        tile.source = Some(DiffSource::Path(wt));
                    }
                    fallback
                }
            },
        };
        let Some(worktree) = worktree else {
            if let Some(tile) = self.diff_tile_mut(id) {
                tile.error = Some(
                    "no worktree to diff (session closed with no prior binding)".to_string(),
                );
            }
            cx.notify();
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
    /// dropped only on success, per spec B3 "the tile shows the previous
    /// model until the new one lands" (a failed refresh still surfaces the
    /// error rather than silently keeping stale content, since the worktree
    /// itself may be gone).
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

    /// Tile-menu "bind" verb (spec B9) — return the focused Diff tile to its
    /// selector so a different session/path can be chosen.
    pub(crate) fn diff_bind_focused(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        if !matches!(self.workspace.focused_content(), Some(App::Diff(_))) {
            return;
        }
        self.diff_unbind(id, cx);
    }

    /// Tile-menu "merge" verb (spec B7) — the merge gate itself is a later
    /// cog node's job; this is the present, wired STUB the spec's B9 menu
    /// requires.
    pub(crate) fn diff_merge_focused(&mut self, cx: &mut Context<Self>) {
        self.transient_status = Some("merge gate not implemented yet".into());
        cx.notify();
    }

    /// Tile-menu "install hook" verb (spec B7) — the hook installer is a
    /// later cog node's job; present, wired STUB per spec B9.
    pub(crate) fn diff_install_hook_focused(&mut self, cx: &mut Context<Self>) {
        self.transient_status = Some("merge-gate hook installer not implemented yet".into());
        cx.notify();
    }

    /// Key handler for a focused Diff tile (spec B9: navigation-only — the
    /// hunk-comment compose is the tile's only insert-mode surface, and it's
    /// `None` out of this node).
    pub(crate) fn handle_diff_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        let press = keystroke_to_keypress(&ev.keystroke);
        if self.leader_intercept(&press, cx) {
            return;
        }
        let Some(id) = self.workspace.focused_window_id() else {
            return;
        };
        let unbound = matches!(self.diff_tile_ref(id), Some(t) if t.source.is_none());
        if unbound {
            match press.key {
                Key::Char('p') => {
                    let dir = self.active_workspace_cwd().unwrap_or_else(process_cwd);
                    self.bind_diff_source(id, DiffSource::Path(dir), cx);
                }
                Key::Char(d) if d.is_ascii_digit() && d != '0' => {
                    let idx = (d as u8 - b'1') as usize;
                    let candidate = self
                        .sessions
                        .iter()
                        .map(|(sid, s)| (sid, s.read(cx).cwd.clone()))
                        .filter(|(_, cwd)| looks_like_git_repo(cwd))
                        .nth(idx);
                    if let Some((sid, _)) = candidate {
                        self.bind_diff_source(id, DiffSource::Session(sid), cx);
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
            _ => {}
        }
    }
}
