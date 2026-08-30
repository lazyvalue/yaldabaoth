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

    /// Every window id holding a Diff tile whose `DiffSource` is
    /// `Session(id)` — spec B3 triggers (a)/(b) fan out a re-derive to
    /// however many tiles (across however many workspaces) are watching one
    /// session's worktree. Mirrors the `for_each_attached_window` walk
    /// `reconcile_session_closed` uses to find a session's `App::Agent` tile,
    /// generalized to `App::Diff` and to "however many", not "at most one" —
    /// unlike an agent tile's 1:1 binding, several Diff tiles may legitimately
    /// watch the same session's worktree at once.
    fn diff_windows_bound_to_session(&self, id: SessionId) -> Vec<workspace::WindowId> {
        let mut out = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.for_each_attached_window(&mut |window| {
                if let App::Diff(tile) = &window.content
                    && tile.source == Some(DiffSource::Session(id))
                {
                    out.push(window.id());
                }
            });
        }
        out
    }

    /// Re-derive every Diff tile bound to session `id` (spec B3 triggers
    /// (a)/(b)). A no-op when nothing is watching this session — the common
    /// case, since most sessions never have a Diff tile open on them.
    pub(crate) fn refresh_diff_tiles_for_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        for wid in self.diff_windows_bound_to_session(id) {
            self.refresh_diff(wid, cx);
        }
    }

    /// Cog node `refresh-triggers` (7ods), spec B3(a): "when a bound session's
    /// turn completes". `finalize_agent_turn_idem` is the one chokepoint every
    /// turn-completion path (forwarded `AgentEvent`, legacy inference, the
    /// direct-channel path) funnels through, so arming `diff_turn_completed_due`
    /// there and draining it HERE — exactly where `drain_autoname_requests`
    /// already runs, for the same "no `cx` at the finalize site" reason —
    /// covers every completion path by construction. Not debounced: a turn
    /// completion is a single, infrequent event (unlike a burst of tool
    /// calls), so an immediate re-derive is correct as-is.
    pub(crate) fn drain_diff_refresh_requests(&mut self, cx: &mut Context<Self>) {
        let due: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, ent)| ent.read(cx).state.diff_turn_completed_due)
            .map(|(id, _)| id)
            .collect();
        for id in due {
            if let Some(ent) = self.sessions.get(id) {
                ent.update(cx, |s, _| s.state.diff_turn_completed_due = false);
            }
            self.refresh_diff_tiles_for_session(id, cx);
        }
    }

    /// Debounce window for spec B3(b) — long enough to coalesce a multi-file
    /// edit turn's tool-call completions (which land back-to-back) into one
    /// re-derive, short enough that the diff still feels live.
    pub(crate) const DIFF_FILE_CHANGE_DEBOUNCE: std::time::Duration =
        std::time::Duration::from_millis(600);

    /// Cog node `refresh-triggers` (7ods), spec B3(b): "debounced after any
    /// tool-call completion... that reports file changes". The reducer arms
    /// (`apply_reply_events` / `apply_agent_event`) bump
    /// `diff_file_change_gen` on every completed file-mutating tool call —
    /// cheap, `cx`-free bookkeeping at the point of detection. This drain
    /// (called from the same two pump chokepoints as
    /// `drain_diff_refresh_requests`) notices a session whose generation has
    /// moved since the last schedule, marks it seen, and schedules ONE
    /// debounce task carrying that generation.
    pub(crate) fn drain_diff_file_change_requests(&mut self, cx: &mut Context<Self>) {
        let due: Vec<(SessionId, u64)> = self
            .sessions
            .iter()
            .filter_map(|(id, ent)| {
                let s = ent.read(cx);
                let gen_ = s.state.diff_file_change_gen;
                if gen_ != s.state.diff_file_change_seen_gen {
                    Some((id, gen_))
                } else {
                    None
                }
            })
            .collect();
        for (id, gen_) in due {
            if let Some(ent) = self.sessions.get(id) {
                ent.update(cx, |s, _| s.state.diff_file_change_seen_gen = gen_);
            }
            self.schedule_debounced_diff_refresh(id, gen_, cx);
        }
    }

    /// Wait out the debounce window, then re-derive ONLY if `gen_` is still
    /// the LATEST generation for this session — a later bump during the
    /// window means a newer scheduled task (carrying that later `gen_`) owns
    /// the eventual refresh instead, so this stale one no-ops. This trailing-
    /// edge pattern collapses a whole burst of file-touching tool calls to
    /// exactly one re-derive: the caller marks each generation "seen" at
    /// SCHEDULE time (`drain_diff_file_change_requests`), so a burst inside
    /// one window spawns several of these tasks, but every task except the
    /// one whose `gen_` survives untouched through its own wait exits without
    /// deriving.
    fn schedule_debounced_diff_refresh(&mut self, id: SessionId, gen_: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Self::DIFF_FILE_CHANGE_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                let current = this
                    .sessions
                    .get(id)
                    .map(|ent| ent.read(cx).state.diff_file_change_gen);
                if current == Some(gen_) {
                    this.refresh_diff_tiles_for_session(id, cx);
                }
            });
        })
        .detach();
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

    /// Key handler for a focused Diff tile (spec B9): navigation by default —
    /// the hunk-comment compose (spec B4) is the tile's only insert-mode
    /// surface, checked FIRST (before the platform/control bail and
    /// `leader_intercept`) so Ctrl-Enter can reach `submit_hunk_comment` while
    /// composing. `focused_in_insert_mode` (`main.rs`) already keys off
    /// `tile.compose.is_some()`, so leaders are suppressed correctly while
    /// composing regardless — this early branch is what makes the compose's
    /// OWN keys (Esc/Ctrl-Enter/typing) actually reach it.
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
            self.handle_diff_comment_key(id, press, cx);
            return;
        }

        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        if self.leader_intercept(&press, cx) {
            return;
        }
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
            Key::Char('v') => self.toggle_hunk_reviewed(id, cx),
            Key::Char('V') => self.mark_file_reviewed(id, cx),
            Key::Char('c') => self.open_hunk_comment(id, cx),
            _ => {}
        }
    }

    // ── Cog node `comment-steering` (hk81): spec B4 comment → steering ──────

    /// `c` on a focused hunk (spec B4): open the hunk-comment compose,
    /// snapshotting the focused hunk's path/line-range/patch into
    /// `comment_target` so a later background refresh (spec B3) can't move
    /// the ground the comment was written against out from under it. A
    /// `Path`-bound tile has NO comment affordance (spec B4/C4: comment→
    /// steering needs a session to steer) — `c` is a silent no-op there, and
    /// likewise a no-op with no model/no hunks to anchor to.
    pub(crate) fn open_hunk_comment(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let target = self.diff_tile_ref(id).and_then(|t| {
            if !matches!(t.source, Some(DiffSource::Session(_))) {
                return None;
            }
            let model = t.model.as_ref()?;
            let file = model.files.get(t.focus.file)?;
            let hunk = file.hunks.get(t.focus.hunk)?;
            Some(CommentTarget {
                path: file.path.clone(),
                line_range: hunk.new_line_range(),
                patch: hunk.patch_text(),
            })
        });
        let Some(target) = target else {
            return;
        };
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.comment_target = Some(target);
            tile.compose = Some(Compose::new());
        }
        cx.notify();
    }

    /// Esc while composing (spec B4/B9): cancel — drop the compose AND its
    /// target, back to hunk-nav. Unlike the worksheet's layered Esc
    /// (Insert→Normal, then leave), this compose has exactly one level: it is
    /// a short-lived, single-purpose surface, not an editable transcript
    /// block.
    pub(crate) fn cancel_hunk_comment(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        if let Some(tile) = self.diff_tile_mut(id) {
            tile.compose = None;
            tile.comment_target = None;
        }
        cx.notify();
    }

    /// Ctrl-Enter while composing (spec B4): build the prefixed prompt
    /// (`build_hunk_comment_prompt`, `diff.rs`) and send it to the bound
    /// session via the SAME `send_prompt_to_session` core the agent compose
    /// uses (`agent_ui.rs`) — mid-turn it steers, idle it prompts, no new
    /// transport. `steer_codex` is computed exactly like `submit_compose`'s:
    /// Claude keeps its unconditional promptQueueing path; only a
    /// cleanly-awaiting Codex session asks the transport to steer. On success
    /// the compose clears; on FAILURE it is left intact (spec B4: "the draft
    /// stays in the compose") with a status hint so the user can retry.
    pub(crate) fn submit_hunk_comment(&mut self, id: workspace::WindowId, cx: &mut Context<Self>) {
        let target_and_session = self.diff_tile_ref(id).and_then(|t| {
            let Some(DiffSource::Session(sid)) = t.source else {
                return None;
            };
            let target = t.comment_target.clone()?;
            let comment = t.compose.as_ref()?.text();
            Some((sid, target, comment))
        });
        let Some((sid, target, comment)) = target_and_session else {
            return;
        };
        let prompt =
            build_hunk_comment_prompt(&target.path, target.line_range, &target.patch, &comment);
        let steer_codex = self
            .read_session(sid, cx, |c| {
                c.provider == AgentProvider::Codex && matches!(c.turn_phase, TurnPhase::Awaiting { .. })
            })
            .unwrap_or(false);
        let sent = self.send_prompt_to_session(sid, &prompt, &[], None, steer_codex, cx);
        if sent {
            if let Some(tile) = self.diff_tile_mut(id) {
                tile.compose = None;
                tile.comment_target = None;
            }
            self.transient_status = Some("comment sent".into());
        } else {
            // Spec B4: on send FAILURE the draft stays in `tile.compose` — no
            // silent drop. Nothing else to undo: we never touched it.
            self.transient_status =
                Some("send failed — reconnecting; press ctrl-enter to retry".into());
        }
        cx.notify();
    }

    /// Key dispatch while the hunk-comment compose is open (spec B4/B9). Esc
    /// cancels; Ctrl-Enter submits (mirrors the agent compose's Ctrl-Enter
    /// submit key — `handle_claude_key`, `agent_ui.rs` — so the same chord
    /// submits every compose in the app); everything else is ordinary typing,
    /// dispatched through the SAME insert-mode core the agent compose and the
    /// Edit view share (`dispatch_insert_core`). The compose never leaves
    /// Insert mode on its own — `dispatch_insert_core`'s internal Esc arm
    /// (which would drop to Normal) never fires because Esc is handled here
    /// first, so there is no Normal-mode dispatch to wire.
    fn handle_diff_comment_key(&mut self, id: workspace::WindowId, press: KeyPress, cx: &mut Context<Self>) {
        if press.key == Key::Esc && press.modifiers.is_empty() {
            self.cancel_hunk_comment(id, cx);
            return;
        }
        if press.key == Key::Enter && press.modifiers.contains(KMods::CONTROL) {
            self.submit_hunk_comment(id, cx);
            return;
        }
        if let Some(tile) = self.diff_tile_mut(id)
            && let Some(compose) = tile.compose.as_mut()
        {
            Self::dispatch_insert_core(&mut compose.editor, &mut compose.mode, press);
        }
        cx.notify();
    }
}
