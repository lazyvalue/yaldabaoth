//! Agent (Claude) tile UI + session-server wiring on YaldaGpuiView:
//! open/attach/create/close session flows, lease heartbeat, server pump
//! + notification reducer (apply_server_batch / apply_reply_events /
//! apply_agent_event), submit paths, and the Claude key handler.
//! render_agent itself lives in main.rs this pass. Extracted verbatim
//! from main.rs (split-gpui-main, stage 2).

use super::*;

/// Outcome of `bind_session_sid` (the live-view bind choke). `Bound` ⇒ the
/// placeholder now carries the sid and the caller should attach; `Focused` ⇒ the
/// sid was already owned, the orphan placeholder was dropped, and the tile was
/// pointed at the existing owner (no attach needed — it is already live).
enum BindOutcome {
    Bound,
    Focused(SessionId),
}

impl YaldaGpuiView {
    /// Open the Claude screen and attempt to attach to an ACP agent. Bound
    /// to `Ctrl-K` in the Doc and Edit views. Stashes the prior screen so
    /// `Ctrl-V` from Claude returns to it.
    ///
    /// Attach uses `YALDA_ACP_AGENT` if set, else the
    /// `claude-agent-acp` default (`AcpChannelClient::DEFAULT_AGENT_COMMAND`).
    pub(crate) fn open_agent(&mut self, _: &OpenAgent, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_agent_inner(cx);
    }

    pub(crate) fn open_agent_inner(&mut self, cx: &mut Context<Self>) {
        // If already on an Agent tile, open the picker/rebind switcher instead.
        if matches!(
            self.workspace.focused_content().expect("no focused window"),
            App::Agent(_)
        ) {
            self.new_agent_session(None, cx);
            return;
        }

        // Replace the focused tile with an Agent tile (no buffer stash —
        // Agent and Buffer are orthogonal; the pooled file buffers stay
        // reachable via Cmd+O). Ctrl-V later converts back to a fresh picker.
        let mut tile = AgentTile::new();
        let proc_cwd = process_cwd();

        if self.session_server.is_some() {
            // ── Session-server path: in-tile session picker ──────────
            // Open the tile straight into the picker (unbound). The picker
            // lists the FREE sessions for the cwd + "start new"; the user
            // decides. The list round-trip runs off the paint thread.
            tile.picker = Some(SessionPicker::loading(proc_cwd.clone()));
            self.start_server_pump(cx);
            self.set_screen(App::Agent(tile));
            cx.notify();

            self.spawn_list_sessions_for_picker(proc_cwd, cx);
            return;
        }

        // ── Direct-spawn path (legacy): bind one session to the tile ──
        self.set_screen(App::Agent(tile));
        let persisted = load_persisted_acp_sessions(&proc_cwd);
        let chosen = persisted
            .iter()
            .find(|s| s.active)
            .or(persisted.first())
            .cloned();
        let id = match chosen {
            None => {
                let state = self.create_agent_session(None, proc_cwd.clone(), cx);
                self.show_local_session(AgentSession {
                    state,
                    label: "claude-1".into(),
                    cwd: proc_cwd.clone(),
                    resume_id: None,
                })
            }
            Some(slot) => {
                let slot_cwd = slot.cwd.clone().unwrap_or_else(|| proc_cwd.clone());
                let mut state =
                    self.create_agent_session(Some(slot.id.clone()), slot_cwd.clone(), cx);
                if slot.mode == InputModeKind::Worksheet {
                    state.input_surface = InputSurface::Worksheet;
                }
                state.tasklist_open = slot.tasklist_open;
                state.subagents_open = slot.subagents_open;
                self.show_local_session(AgentSession {
                    state,
                    label: slot.label,
                    cwd: slot_cwd,
                    resume_id: Some(slot.id),
                })
            }
        };
        self.start_session_pump(id, cx);

        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Background half of the in-tile session picker: list the server's
    /// sessions for `cwd` off the paint thread, keep those for this cwd that
    /// aren't already open in another tile, and hand the result to
    /// `apply_picker_sessions`. Mirrors the discovery half of
    /// `spawn_open_agent_server`, but stops at "list" — the user, not the
    /// code, decides what (if anything) to attach.
    pub(crate) fn spawn_list_sessions_for_picker(&self, cwd: PathBuf, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        // Snapshot the sids BOUND to a tile so the picker offers only FREE
        // sessions (spec-agent-session-ownership.md). A session in the store
        // that no tile binds is free and re-bindable.
        let open_sids = self.bound_sid_set();
        cx.spawn(async move |this, cx| {
            let cwd_for_apply = cwd.clone();
            let result: Result<Vec<PickerSession>, String> = cx
                .background_executor()
                .spawn(async move {
                    let existing = handle.list_sessions().map_err(|e| e.to_string())?;
                    let cwd_key = cwd_match_key(&cwd);
                    Ok(existing
                        .into_iter()
                        .filter(|s| cwd_match_key(&s.cwd) == cwd_key)
                        .filter(|s| !open_sids.contains(&s.session_id))
                        .map(|s| PickerSession {
                            sid: s.session_id,
                            acp_id: s.acp_session_id,
                            label: s.label,
                            turns: s.turns,
                            connected: s.connected,
                            has_owner: s.has_owner,
                            permission_mode: s.permission_mode,
                        })
                        .collect())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_picker_sessions(cwd_for_apply, result, cx);
            });
        })
        .detach();
    }

    /// Fold a completed `list_sessions` result into the focused tile's picker.
    /// No-op if the focused tile is no longer an empty Agent ring whose picker
    /// is still loading for this cwd (the user switched tabs or already picked
    /// before the list landed) — the same harmless-discard contract as
    /// `apply_open_agent_resolution`.
    pub(crate) fn apply_picker_sessions(
        &mut self,
        cwd: PathBuf,
        result: Result<Vec<PickerSession>, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(tile) = self.agent_tile_mut()
            && let Some(picker) = tile.picker.as_mut()
            && picker.sessions.is_none()
            && cwd_match_key(&picker.cwd) == cwd_match_key(&cwd)
        {
            match result {
                Ok(sessions) => picker.sessions = Some(sessions),
                Err(e) => {
                    picker.error = Some(format!("couldn't list sessions — {e}").into());
                    picker.sessions = Some(Vec::new());
                }
            }
            cx.notify();
        }
    }

    /// The set of server sids currently BOUND to some tile (across all tabs).
    /// Their `AgentSession`s exist in the store; everything else in the store
    /// or on the server is free.
    pub(crate) fn bound_sid_set(&self) -> std::collections::HashSet<String> {
        let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tab in self.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |w| {
                if let App::Agent(tile) = &w.content
                    && let Some(id) = tile.bound
                    && let Some(sid) = self.sessions.sid_of(id)
                {
                    bound.insert(sid.to_string());
                }
            });
        }
        bound
    }

    /// Move the picker highlight (j/k or ↑/↓). No-op outside picker mode.
    pub(crate) fn agent_picker_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(tile) = self.agent_tile_mut()
            && let Some(picker) = tile.picker.as_mut()
        {
            picker.move_selection(delta);
            cx.notify();
        }
    }

    /// Activate picker row `row`: row 0 starts a fresh session; rows `1..=N`
    /// attach the corresponding listed session. No-op outside picker mode or
    /// for an out-of-range / still-loading row.
    pub(crate) fn agent_picker_activate(&mut self, row: usize, cx: &mut Context<Self>) {
        // What to do, extracted before the helpers borrow `&mut self`.
        enum Choice {
            New(PathBuf),
            Attach {
                cwd: PathBuf,
                sid: String,
                acp_id: Option<String>,
                label: String,
                connected: bool,
                permission_mode: yalda::acp_channel::PermissionMode,
            },
        }
        let choice = self.agent_tile_mut().and_then(|tile| {
            let picker = tile.picker.as_ref()?;
            if row == 0 {
                Some(Choice::New(picker.cwd.clone()))
            } else {
                let s = picker.sessions.as_ref()?.get(row - 1)?;
                Some(Choice::Attach {
                    cwd: picker.cwd.clone(),
                    sid: s.sid.clone(),
                    acp_id: s.acp_id.clone(),
                    label: s.label.clone(),
                    connected: s.connected,
                    permission_mode: s.permission_mode,
                })
            }
        });
        match choice {
            Some(Choice::New(cwd)) => self.picker_start_new(cwd, cx),
            Some(Choice::Attach {
                cwd,
                sid,
                acp_id,
                label,
                connected,
                permission_mode,
            }) => {
                self.picker_attach_existing(cwd, sid, acp_id, label, connected, permission_mode, cx)
            }
            None => {}
        }
    }

    /// Picker → "start a new session": clear the picker, bind a placeholder
    /// session to this tile, and create a fresh session via the shared path.
    fn picker_start_new(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let label = "claude-1".to_string();
        let open_token = alloc_open_token();
        if self.agent_tile_mut().is_none() {
            return;
        }
        self.show_local_session(AgentSession {
            state: AgentState::new_server_managed(Some("connecting to session server…".into())),
            label: label.clone(),
            cwd: cwd.clone(),
            resume_id: None,
        });
        if let Some(tile) = self.agent_tile_mut() {
            tile.pending_open_token = Some(open_token);
        }
        self.spawn_create_agent_session(open_token, label, cwd, cx);
        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Picker → attach an existing session. The sid / acp id / permission mode
    /// all came from the `list_sessions` result, so we feed the bind+attach
    /// path directly: bind a placeholder session to this tile stamped with the
    /// open token, then synchronously run `apply_open_agent_resolution`, which
    /// binds the sid and kicks off `spawn_attach_sessions`.
    #[allow(clippy::too_many_arguments)]
    fn picker_attach_existing(
        &mut self,
        cwd: PathBuf,
        sid: String,
        acp_id: Option<String>,
        label: String,
        connected: bool,
        permission_mode: yalda::acp_channel::PermissionMode,
        cx: &mut Context<Self>,
    ) {
        let open_token = alloc_open_token();
        if self.agent_tile_mut().is_none() {
            return;
        }
        self.show_local_session(AgentSession {
            state: AgentState::new_server_managed(Some("reconnecting…".into())),
            label: label.clone(),
            cwd,
            resume_id: None,
        });
        if let Some(tile) = self.agent_tile_mut() {
            tile.pending_open_token = Some(open_token);
        }
        let status = if connected {
            "reconnecting…"
        } else {
            "reconnecting (agent spawning…)"
        };
        let resolution = OpenResolution::Attached(vec![AttachedSlot {
            label,
            sid,
            acp_id,
            status: status.to_string(),
            permission_mode,
        }]);
        self.apply_open_agent_resolution(open_token, resolution, cx);
        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
    }

    /// Key handler for the in-tile session picker (`AgentPickerView`):
    /// j/k or ↑/↓ to move, Enter to activate, Ctrl-V to back out to the
    /// underlying buffer so a mis-opened agent tile is never a dead end.
    pub(crate) fn handle_picker_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        if press.modifiers.contains(KMods::CONTROL)
            && matches!(press.key, Key::Char('v') | Key::Char('V'))
        {
            self.back_to_doc(cx);
            return;
        }
        match press.key {
            Key::Up | Key::Char('k') => self.agent_picker_move(-1, cx),
            Key::Down | Key::Char('j') => self.agent_picker_move(1, cx),
            Key::Enter => {
                if let Some(row) = self
                    .agent_tile()
                    .and_then(|t| t.picker.as_ref())
                    .map(|p| p.selected)
                {
                    self.agent_picker_activate(row, cx);
                }
            }
            _ => {}
        }
    }

    /// Apply the result of the background `open_agent` round-trips: fill the
    /// "connecting…" placeholder slot (preserving its pump) and, for the
    /// re-attach case, push any additional slots. A no-op if the placeholder
    /// is gone (screen switched / slot closed before the result returned).
    pub(crate) fn apply_open_agent_resolution(
        &mut self,
        open_token: u64,
        resolution: OpenResolution,
        cx: &mut Context<Self>,
    ) {
        // Find the placeholder TILE that started this open by its globally-
        // unique `open_token`, then bind the resolved sid into the store
        // through the single choke. If the tile is gone (screen closed, OR the
        // user rebound the tile to a different session mid-open), the placeholder
        // is orphaned — but the server may have already SPAWNED a session for
        // this resolution. Close it so it doesn't leak a running agent.
        let Some(id) = self.session_id_for_open_token(open_token) else {
            let orphan_sid = match &resolution {
                OpenResolution::Created { sid, .. } => Some(sid.clone()),
                OpenResolution::Attached(attached) => attached.first().map(|a| a.sid.clone()),
                OpenResolution::Failed(_) => None,
            };
            if let Some(sid) = orphan_sid {
                eprintln!(
                    "[yalda-gpui] open token {open_token} has no tile (rebound mid-open); \
                     closing orphaned server session {}",
                    &sid[..sid.len().min(8)]
                );
                self.spawn_close_session(sid, cx);
            }
            return;
        };
        // Consume the token regardless of outcome so a late duplicate
        // resolution can't re-bind this tile.
        self.clear_open_token(open_token);

        let mut bound_sids: Vec<String> = Vec::new();
        match resolution {
            OpenResolution::Failed(msg) => {
                let m = format!("session server error — {msg}");
                if let Some(session) = self.sessions.get_mut(id) {
                    Self::append_system_notice(&mut session.state, &m);
                    session.state.status = Some(m.into());
                }
            }
            OpenResolution::Created {
                sid,
                acp_id,
                permission_mode,
            } => match self.bind_session_sid(id, &sid) {
                BindOutcome::Bound => {
                    if let Some(session) = self.sessions.get_mut(id) {
                        session.resume_id = acp_id;
                        session.state.permission_mode = permission_mode;
                        session.state.status =
                            Some("attaching to ACP agent via session server…".into());
                    }
                    bound_sids.push(sid);
                }
                BindOutcome::Focused(owner) => self.focus_existing_session(owner),
            },
            OpenResolution::Attached(attached) => {
                // Strict 1:1: a tile shows exactly one session. Bind the FIRST
                // attached session to this tile; ignore extras (the server may
                // list several per cwd, but each gets its own tile via the
                // picker, never a hidden ring).
                if let Some(first) = attached.into_iter().next() {
                    match self.bind_session_sid(id, &first.sid) {
                        BindOutcome::Bound => {
                            if let Some(session) = self.sessions.get_mut(id) {
                                session.label = first.label;
                                session.resume_id = first.acp_id;
                                session.state.permission_mode = first.permission_mode;
                                session.state.status = Some(first.status.into());
                            }
                            bound_sids.push(first.sid);
                        }
                        BindOutcome::Focused(owner) => self.focus_existing_session(owner),
                    }
                }
            }
        }
        self.save_agent_ring();
        cx.notify();

        // Now that the session carries its sid, attach (which starts the
        // server's event replay). Routing can no longer drop the replay because
        // the session is already bound. Deferred off the paint thread.
        let targets = bound_sids;
        if !targets.is_empty() {
            self.spawn_attach_sessions(targets, cx);
        }
    }

    /// Locate the `SessionId` of the session whose tile carries `token` in its
    /// `pending_open_token` (across all tabs/tiles).
    fn session_id_for_open_token(&self, token: u64) -> Option<SessionId> {
        for tab in self.workspace.tabs.iter() {
            let mut found = None;
            tab.layout.for_each_leaf(&mut |w| {
                if let App::Agent(tile) = &w.content
                    && tile.pending_open_token == Some(token)
                {
                    found = tile.bound;
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Clear the `pending_open_token` on whichever tile carries `token`.
    fn clear_open_token(&mut self, token: u64) {
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(tile) = content
                    && tile.pending_open_token == Some(token)
                {
                    tile.pending_open_token = None;
                }
            });
        }
    }

    /// Bind `sid` to the freshly-minted placeholder session `id` through the
    /// store, with focus-on-conflict semantics (INV-1/INV-2/INV-3).
    ///
    /// - `Ok(())` ⇒ `id` now carries `sid`; the caller proceeds to attach.
    /// - `Err(AlreadyBound(owner))` ⇒ another session already owns `sid` (a
    ///   duplicate resolution — e.g. two restored tiles racing the same listed
    ///   session). We must NOT leave the orphan placeholder `id` stuck on
    ///   "attaching…": drop it from the store and point the focused tile at the
    ///   existing `owner` instead (the same AlreadyOpen/focus semantics
    ///   `show_session` implements). Returns [`BindOutcome`] so the caller can
    ///   skip the (redundant) attach for the focus case.
    fn bind_session_sid(&mut self, id: SessionId, sid: &str) -> BindOutcome {
        match self.sessions.bind_sid(id, sid.to_string()) {
            Ok(()) => BindOutcome::Bound,
            Err(AlreadyBound(owner)) => {
                eprintln!(
                    "[yalda-gpui] sid {} already owned by another session; \
                     dropping orphan placeholder and focusing the owner",
                    &sid[..sid.len().min(8)]
                );
                // Drop the orphan placeholder we just minted (it carries no sid,
                // so this only tears down its local channel/pump, never the live
                // server session).
                self.sessions.close(id);
                BindOutcome::Focused(owner)
            }
        }
    }

    /// Point the focused tile at `owner` (the existing session that already
    /// holds the sid we tried to bind) — the focus half of the AlreadyBound
    /// path. Clears the picker/token so the tile shows the live transcript.
    fn focus_existing_session(&mut self, owner: SessionId) {
        if let Some(tile) = self.agent_tile_mut() {
            tile.bound = Some(owner);
            tile.picker = None;
            tile.pending_open_token = None;
        }
    }

    /// Attach (with Owner-reclaim retry) to sessions whose slots were just
    /// bound by `apply_open_agent_resolution`, off the paint thread. Attaching
    /// here — AFTER the bind — is what closes the replay-drop race: the pump
    /// can route every replayed notification because its slot already exists.
    /// Per-session ownership outcome is reconciled back into the slot status so
    /// a read-only / failed attach is visible instead of a silently-dead session.
    pub(crate) fn spawn_attach_sessions(&self, sids: Vec<String>, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        let want_owner = !self.is_candidate;
        cx.spawn(async move |this, cx| {
            let results: Vec<(String, Result<bool, String>)> = cx
                .background_executor()
                .spawn(async move {
                    sids.into_iter()
                        .map(|sid| {
                            let r = attach_for_role(&handle, &sid, want_owner);
                            (sid, r)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Sids whose attach failed with a PERMANENT "session is gone"
                // error. These are dropped after the status pass so the dead
                // slot neither lingers as a broken tab nor gets retried next
                // launch (see below). Transient failures keep today's behavior.
                let mut dead_sids: Vec<String> = Vec::new();
                for (sid, r) in results {
                    // Per-session outcome: the status string to surface (if
                    // any). Lease/driver tracking is gone under the 1:1 model.
                    let status: Option<SharedString> = match r {
                        // Granted drive rights (Owner): leave the optimistic
                        // status to be overwritten by the first real event.
                        Ok(true) => None,
                        // Downgraded to Observer despite wanting Owner.
                        Ok(false) if want_owner => {
                            Some("read-only — another window owns this session".into())
                        }
                        Ok(false) => None,
                        Err(e) => {
                            eprintln!(
                                "[yalda-gpui] attach failed for {}: {e}",
                                &sid[..sid.len().min(8)]
                            );
                            // `no such session: <id>` is PERMANENT — the
                            // persisted id outlived the server's WAL. Drop the
                            // session rather than churn a broken one. Other
                            // errors are TRANSIENT and may recover on reconnect.
                            if is_session_gone_error(&e) {
                                dead_sids.push(sid.clone());
                                None
                            } else {
                                Some("attach failed — session may be unavailable".into())
                            }
                        }
                    };
                    if let Some(s) = status
                        && let Some(session) = this.sessions.get_by_sid_mut(&sid)
                    {
                        session.state.status = Some(s);
                    }
                }
                // Drop dead sessions via the same path the server's
                // SessionClosed broadcast uses: `reconcile_session_closed`
                // removes the session from the store and unbinds whichever tile
                // showed it (the tile transitions to the selector, never back to
                // a buffer). Then re-persist so the stale id doesn't resume.
                let mut dropped_any = false;
                for sid in &dead_sids {
                    if this.reconcile_session_closed(sid, cx) {
                        dropped_any = true;
                    }
                }
                if dropped_any {
                    this.save_agent_ring();
                }
                // Authoritatively scrub the dead ids from the persisted file by
                // id (across every cwd key). `save_agent_ring` alone misses the
                // cases that matter most here: a single-slot ring that empties
                // (the tile no longer holds an Agent ring, so the re-save never
                // touches that cwd) and a stale session in a non-active tab
                // (save_agent_ring only walks the active tab). Without this the
                // dead id would be resumed again on the next launch — the exact
                // churn this fix targets.
                if !dead_sids.is_empty() {
                    forget_persisted_acp_session_ids(&dead_sids);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Create a new session and add it to the existing ring. With `cwd =
    /// None`, the new slot inherits the process cwd (today's behavior). With
    /// `cwd = Some(path)`, that already-resolved absolute path becomes the
    /// new slot's cwd — the caller (typically the `:claude-new <path>`
    /// command handler) is responsible for running the input through
    /// `resolve_agent_cwd_arg` first.
    pub(crate) fn new_agent_session(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        // Not on an Agent tile yet — bootstrap one AND create a brand-new
        // session (never re-attach an existing per-cwd one).
        if self.agent_tile().is_none() {
            self.bootstrap_fresh_agent_session(cwd, cx);
            return;
        }
        // On an Agent tile: a tile shows exactly one session (1:1), so "new
        // session" REBINDS this tile to a fresh session. The previously-bound
        // session is freed (kept running in the store) UNLESS it is a pre-attach
        // local placeholder mid-open — that would orphan its in-flight create
        // (no tile would match its token), so close it.
        let label = "claude".to_string();
        let slot_cwd = cwd.unwrap_or_else(process_cwd);
        self.release_focused_session_for_rebind();

        if self.session_server.is_some() {
            // Server path: bind a "connecting…" placeholder and create the
            // session off-thread; the sid binds when the round-trip returns.
            let open_token = alloc_open_token();
            self.show_local_session(AgentSession {
                state: AgentState::new_server_managed(Some("connecting to session server…".into())),
                label: label.clone(),
                cwd: slot_cwd.clone(),
                resume_id: None,
            });
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(open_token, label, slot_cwd, cx);
        } else {
            // Direct-spawn path.
            let state = self.create_agent_session(None, slot_cwd.clone(), cx);
            let id = self.show_local_session(AgentSession {
                state,
                label,
                cwd: slot_cwd,
                resume_id: None,
            });
            self.start_session_pump(id, cx);
        }
        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Bootstrap the Agent screen from a non-Agent screen AND create a
    /// brand-new session — never re-attach an existing per-cwd one. This is
    /// the always-fresh counterpart to `open_agent_inner`: `open_agent_inner`
    /// resolves the cwd against the server's existing sessions and resumes a
    /// match (the "open the agent tile" semantics), whereas the "new session"
    /// command must always start a fresh conversation. The screen-stash /
    /// placeholder / server-pump setup is identical to `open_agent_inner`'s
    /// server path; the only difference is it calls `spawn_create_agent_session`
    /// (create-only) instead of `spawn_open_agent_server` (list + resolve +
    /// attach). The direct-spawn path builds a fresh `create_agent_session`
    /// with no `resume_id`.
    pub(crate) fn bootstrap_fresh_agent_session(
        &mut self,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // Replace the focused tile with a fresh Agent tile (no buffer stash —
        // Agent and Buffer are orthogonal).
        let tile = AgentTile::new();
        self.set_screen(App::Agent(tile));
        let slot_cwd = cwd.unwrap_or_else(process_cwd);
        let label = "claude-1".to_string();

        if self.session_server.is_some() {
            // Server path: placeholder + create-only round-trip (NO resolve /
            // reattach — that is the whole point of "fresh").
            let open_token = alloc_open_token();
            self.show_local_session(AgentSession {
                state: AgentState::new_server_managed(Some("connecting to session server…".into())),
                label: label.clone(),
                cwd: slot_cwd.clone(),
                resume_id: None,
            });
            self.start_server_pump(cx);
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            if let Some(c) = self.agent_mut() {
                c.editor.begin_insert();
            }
            cx.notify();
            self.spawn_create_agent_session(open_token, label, slot_cwd, cx);
        } else {
            // Direct-spawn path: a fresh session has no resume_id.
            let state = self.create_agent_session(None, slot_cwd.clone(), cx);
            let id = self.show_local_session(AgentSession {
                state,
                label,
                cwd: slot_cwd,
                resume_id: None,
            });
            self.start_session_pump(id, cx);
            if let Some(c) = self.agent_mut() {
                c.editor.begin_insert();
            }
            self.save_agent_ring();
            cx.notify();
        }
    }

    /// Background half of `new_agent_session`'s session-server path (S4).
    /// Issues the `create_session` + `attach` round-trips off the paint thread
    /// and fills the "connecting…" placeholder (by `placeholder_index`) when
    /// they return. No-op if the placeholder is gone by then.
    pub(crate) fn spawn_create_agent_session(
        &self,
        open_token: u64,
        label: String,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let resolution = cx
                .background_executor()
                .spawn(async move {
                    // Create only — attach is deferred to
                    // `apply_open_agent_resolution` (after the slot binds its
                    // `server_session_id`) so the bind-before-attach ordering
                    // is uniform across the open and new-session paths.
                    match handle.create_session(cwd, label, None) {
                        Ok(info) => OpenResolution::Created {
                            sid: info.session_id,
                            acp_id: info.acp_session_id,
                            permission_mode: info.permission_mode,
                        },
                        Err(e) => OpenResolution::Failed(format!("create failed: {e}")),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_open_agent_resolution(open_token, resolution, cx);
            });
        })
        .detach();
    }

    /// Respawn the slot identified by `slot_index` (monotonic
    /// `AgentSlot::index`) at a new working directory. Implements
    /// spec-agent-cwd.md §4 step-by-step: drop the current channel
    /// (kills subprocess), null out attach/awaiting state, swap the
    /// slot's `cwd`, drop `resume_id`, append a session-divider line
    /// to the transcript, and spawn a fresh channel. The transcript
    /// is otherwise preserved so the user can scroll back through
    /// the prior session's history above the divider.
    pub(crate) fn change_agent_cwd(
        &mut self,
        id: SessionId,
        new_cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if !self.sessions.contains(id) {
            return;
        }

        // Phase 1: tear down the existing channel + attach state.
        if let Some(session) = self.sessions.get_mut(id) {
            // Dropping `channel` kills the subprocess via kill_on_drop.
            session.state.channel = None;
            session.state.attach_pending = None;
            session.state.turn_phase = TurnPhase::Idle;
            let msg = format!("changing cwd to {}…", shorten_cwd_for_display(&new_cwd));
            Self::append_system_notice(&mut session.state, &msg);
            session.state.status = Some(msg.into());
            session.cwd = new_cwd.clone();
            // A fresh session/new is the right resume strategy for the new cwd.
            session.resume_id = None;
        }

        // Phase 2: build a fresh agent session at the new cwd.
        if self.session_server.is_some() {
            // Server path: close the old server session, then create a new one
            // and rebind THIS session's sid when the round-trip returns.
            //
            // Release the old sid from the store SYNCHRONOUSLY before firing the
            // close, so the in-flight `SessionClosed(old_sid)` broadcast can no
            // longer `locate` this session and destroy it mid-respawn (the
            // close-before-create race). The `SessionId`/payload survive — only
            // the sid binding is dropped — so the create resolution rebinds the
            // new sid onto the same live session/transcript.
            let old_sid = self.sessions.clear_sid(id);
            if let Some(old_sid) = old_sid {
                self.spawn_close_session(old_sid, cx);
            }
            let open_token = alloc_open_token();
            if let Some(session) = self.sessions.get_mut(id) {
                session.state.attach_pending = None;
                session.state.channel = None;
                let msg = format!(
                    "cwd → {}, connecting to fresh session…",
                    shorten_cwd_for_display(&new_cwd),
                );
                Self::append_system_notice(&mut session.state, &msg);
                session.state.status = Some(msg.into());
            }
            // Stamp the focused tile (which shows this session) with the token.
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(
                open_token,
                "respawned".to_string(),
                new_cwd.clone(),
                cx,
            );
        } else {
            // Direct-spawn path: graft a throwaway AgentState's attach handle
            // into the existing session, then (re)start its pump.
            let fresh = self.create_agent_session(None, new_cwd.clone(), cx);
            if let Some(session) = self.sessions.get_mut(id) {
                session.state.attach_pending = fresh.attach_pending;
                let msg = format!("cwd → {}, fresh session", shorten_cwd_for_display(&new_cwd));
                Self::append_system_notice(&mut session.state, &msg);
                session.state.status = Some(msg.into());
            }
            self.start_session_pump(id, cx);
        }

        self.save_agent_ring();
        cx.notify();
    }

    /// Prepare the focused tile to be REBOUND to a different session (rebind /
    /// new-session). Frees its current session (kept running in the store) —
    /// EXCEPT a pre-attach local placeholder whose create round-trip is still in
    /// flight (`pending_open_token` set, no sid): rebinding away would overwrite
    /// the token so the resolution can never find a tile, leaking the server
    /// session its create spawns. So we CLOSE that placeholder up front (its
    /// channel/pump cancel on drop). Always clears `bound`/`picker`/token on the
    /// tile so the caller can bind fresh.
    pub(crate) fn release_focused_session_for_rebind(&mut self) {
        let bound = self.focused_bound_session();
        let pending = self.agent_tile().and_then(|t| t.pending_open_token);
        if let Some(id) = bound {
            let sid_less = self.sessions.sid_of(id).is_none();
            if sid_less && pending.is_some() {
                // Orphaned pre-attach placeholder: kill it. If its create
                // already spawned a server session, that is closed when its
                // resolution finds no tile (see `apply_open_agent_resolution`).
                self.sessions.close(id);
            }
        }
        if let Some(tile) = self.agent_tile_mut() {
            tile.bound = None;
            tile.picker = None;
            tile.pending_open_token = None;
        }
    }

    /// Unbind the focused tile and land it in a LIVE free-session selector:
    /// install a loading `SessionPicker` and kick off the server list (server
    /// mode), so Enter/j/k are immediately usable — NOT a dead `picker == None`
    /// state that only Ctrl-V can escape. Used by close / reconcile.
    pub(crate) fn show_selector_on_focused_tile(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        if let Some(tile) = self.agent_tile_mut() {
            tile.bound = None;
            tile.pending_open_token = None;
            tile.picker = Some(SessionPicker::loading(cwd.clone()));
        }
        if self.session_server.is_some() {
            self.spawn_list_sessions_for_picker(cwd, cx);
        }
    }

    /// Close the focused session. The tile stays an Agent tile, transitioning
    /// to a LIVE unbound selector (it does NOT fall back to a buffer — only an
    /// explicit Ctrl-V / back_to_doc does that).
    pub(crate) fn close_active_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        // Fire the server close off the paint thread (it parks on a 30s
        // recv_timeout). The SessionClosed broadcast reconciles the rest.
        let server_sid = self.sessions.sid_of(id).map(|s| s.to_string());
        if let Some(sid) = server_sid {
            self.spawn_close_session(sid, cx);
        }
        let cwd = self
            .sessions
            .get(id)
            .map(|s| s.cwd.clone())
            .unwrap_or_else(process_cwd);
        // Drop the session from the store (its channel/pump cancel on drop) and
        // land the tile in a live selector.
        self.sessions.close(id);
        self.show_selector_on_focused_tile(cwd, cx);
        // Wipe the cwd entry so reboot doesn't resurrect the closed session.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        self.save_agent_ring();
        cx.notify();
        // No early `back_to_doc` — the tile stays Agent (unbound → selector).
    }

    /// Fire a `close_session` off the paint thread (S4). The local slot has
    /// already been dropped (optimistic close); this just tells the server to
    /// tear down its session. On a logical error (we're an observer, or it's
    /// already gone) it best-effort detaches. Errors are logged, not surfaced —
    /// the slot is gone and the `SessionClosed` broadcast reconciles the rest.
    pub(crate) fn spawn_close_session(&self, sid: String, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                match handle.close_session(&sid) {
                    Ok(()) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        eprintln!(
                            "[yalda-gpui] close_session({}) failed (connection): {e}",
                            &sid[..sid.len().min(8)],
                        );
                    }
                    Err(_) => {
                        // Logical error — detach best-effort so the server drops
                        // our subscription even though the session lives on.
                        let _ = handle.detach(&sid);
                    }
                }
            })
            .detach();
    }

    /// Snapshot every bound agent session to disk. Walks ALL tabs (not just the
    /// active one) so it is SYMMETRIC with restore (`restore_agent_leaves`
    /// collects agent leaves across every tab) — otherwise an agent in a
    /// background tab would be saved-but-not-restored or vice versa. The first
    /// bound session is marked active. Free sessions (no tile) are not persisted
    /// — they only live for the running process. Best-effort.
    pub(crate) fn save_agent_ring(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let mut snaps: Vec<SessionSnapshot> = Vec::new();
        for tab in self.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |window| {
                if let App::Agent(tile) = &window.content
                    && let Some(id) = tile.bound
                    && let Some(session) = self.sessions.get(id)
                {
                    // resume_id wins over the channel id (keep retrying the
                    // original id even when load fell back).
                    let resolved_id = session
                        .resume_id
                        .clone()
                        .or_else(|| session.state.channel.as_ref().and_then(|c| c.session_id()));
                    if let Some(rid) = resolved_id {
                        snaps.push(SessionSnapshot {
                            id: rid,
                            label: session.label.clone(),
                            active: snaps.is_empty(),
                            mode: session.state.input_surface.mode(),
                            tasklist_open: session.state.tasklist_open,
                            subagents_open: session.state.subagents_open,
                            cwd: session.cwd.clone(),
                        });
                    }
                }
            });
        }
        save_persisted_acp_sessions(&cwd, &snaps);
    }

    /// Build an `AgentState` with an ACP attach thread (direct-spawn path).
    /// `cwd` is the per-session working directory (spec-agent-cwd.md §3). The
    /// pump is NOT started here — the caller binds the state into the store via
    /// `show_local_session`, then calls [`start_session_pump`] with the
    /// resulting [`SessionId`] so the pump routes by that stable key (no
    /// monotonic-index fragility).
    pub(crate) fn create_agent_session(
        &mut self,
        resume_id: Option<String>,
        cwd: PathBuf,
        _cx: &mut Context<Self>,
    ) -> AgentState {
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("YALDA_ACP_AGENT").unwrap_or_default();
        let spawn_cwd = Some(cwd);
        let _ = std::thread::Builder::new()
            .name("yalda-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    spawn_cwd,
                    resume_id,
                    yalda::acp_channel::YaldaFrontend::Gpui,
                ));
            });

        let editor = Editor::new(String::new(), PathBuf::from("*claude*"));

        let state = AgentState {
            editor,
            channel: None,
            attach_pending: Some(attach_rx),
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            last_reconciled_edit_seq: 0,
            status: Some("attaching to ACP agent…".into()),
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: false,
            reconciler: yalda::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            generation: 0,
            finalized: std::collections::HashSet::new(),
            replay_prefix_finalized: false,
            agent_stream_authoritative: false,
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: None,
        };
        setup_list_follow_handler(&state.list_state, &state.follow_output);
        state
    }

    /// Spawn the per-session direct-spawn pump task, routing by the stable
    /// [`SessionId`] and storing the handle on the session so dropping the
    /// session cancels it. Call AFTER `show_local_session` binds the state.
    pub(crate) fn start_session_pump(&mut self, id: SessionId, cx: &mut Context<Self>) {
        let pump = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;
            let idle_delay = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let min_cycle = Duration::from_millis(16);
            let anim_period = Duration::from_millis(120);
            let mut last_anim = std::time::Instant::now();
            let mut wake_rx: Option<futures::channel::mpsc::UnboundedReceiver<()>> = None;
            loop {
                let cycle_start = std::time::Instant::now();
                if wake_rx.is_some() {
                    let mut rx = wake_rx.take().unwrap();
                    let timer = cx.background_executor().timer(idle_delay);
                    futures::select_biased! {
                        _ = rx.next().fuse() => {}
                        _ = timer.fuse() => {}
                    }
                    while rx.next().now_or_never().flatten().is_some() {}
                    wake_rx = Some(rx);
                } else {
                    cx.background_executor().timer(idle_delay).await;
                    let _ = this.update(cx, |this, _cx| {
                        if let Some(session) = this.sessions.get_mut(id)
                            && let Some(ch) = &session.state.channel
                        {
                            wake_rx = ch.take_wake_receiver();
                        }
                    });
                }
                loop {
                    let t_apply = perf_enabled().then(std::time::Instant::now);
                    let more = match this.update(cx, |this, cx| this.pump_session(id, cx)) {
                        Ok(more) => more,
                        Err(_) => return,
                    };
                    if let Some(t) = t_apply {
                        eprintln!(
                            "[perf] acp-pump drain+apply lock_held={:.2}ms more={more}",
                            t.elapsed().as_secs_f64() * 1e3,
                        );
                    }
                    if !more {
                        break;
                    }
                    cx.background_executor().timer(yield_delay).await;
                }
                if last_anim.elapsed() >= anim_period {
                    last_anim = std::time::Instant::now();
                    let _ = this.update(cx, |this, cx| {
                        if this.any_agent_awaiting() {
                            cx.notify();
                        }
                    });
                }
                let elapsed = cycle_start.elapsed();
                if elapsed < min_cycle {
                    cx.background_executor().timer(min_cycle - elapsed).await;
                }
            }
        });
        if let Some(session) = self.sessions.get_mut(id) {
            session.state._pump = Some(pump);
        }
    }

    /// Re-establish the session-server connection after a drop, then
    /// re-subscribe every live slot. Returns the fresh notification + wake
    /// receivers so the pump can splice them in and keep running; returns
    /// `None` when the reconnect itself failed (server still down — the pump
    /// retries on its backoff).
    ///
    /// Each slot's transcript is reset before re-attach: the server replays
    /// the full `event_log` on attach, so resetting lets that replay rebuild
    /// the on-screen transcript cleanly instead of duplicating it.
    pub(crate) fn reconnect_session_server(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(
        std::sync::mpsc::Receiver<ServerNotification>,
        futures::channel::mpsc::UnboundedReceiver<()>,
    )> {
        let (note_rx, wake_rx) = match self.session_server.as_mut()?.reconnect() {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("[yalda-gpui] session-server reconnect failed: {e}");
                return None;
            }
        };

        // Reset every server-backed session's transcript and collect the sids
        // to re-attach. Sessions are now owned centrally — one store walk.
        let mut sids: Vec<String> = Vec::new();
        let ids: Vec<SessionId> = self.sessions.ids().collect();
        for id in ids {
            let Some(sid) = self.sessions.sid_of(id).map(|s| s.to_string()) else {
                continue;
            };
            if let Some(session) = self.sessions.get_mut(id) {
                session.state.reset_for_replay();
                Self::append_system_notice(&mut session.state, "reconnecting…");
                session.state.status = Some("reconnecting…".into());
            }
            sids.push(sid);
        }

        // Re-attach off the paint thread, with the same Owner-reclaim retry the
        // open path uses. The PREVIOUS connection's server-side teardown races
        // this fresh one: the new socket can re-attach before the server has
        // processed the old socket's close and released ownership, so a bare
        // attach momentarily loses to a not-yet-cleared owner ("another GUI
        // already owns this session"). `spawn_attach_sessions` retries past
        // that window and reconciles per-slot status. (Doing blocking attach
        // round-trips inline here, as before, also froze rendering.)
        let n = sids.len();
        if !sids.is_empty() {
            self.spawn_attach_sessions(sids, cx);
        }
        eprintln!("[yalda-gpui] session-server reconnected; re-attaching {n} session(s)");
        Some((note_rx, wake_rx))
    }

    /// Unified pump task for the session server path. Drains all notifications
    /// from `SessionServerClient::try_recv()` and routes them to the correct
    /// `AgentSession` by sid through the store. Runs as a single GPUI
    /// background task per view (not per-session). The lease-heartbeat machinery
    /// is gone under the 1:1 model (spec-agent-session-ownership.md §"dormant
    /// promote") — the client no longer drives leases.
    pub(crate) fn start_server_pump(&mut self, cx: &mut Context<Self>) {
        // Singleton guard: one pump per view, alive for the view's lifetime.
        // Re-entry (every open/new/restore path calls this defensively) is a
        // no-op; the receivers stay owned by the original task.
        if self._server_pump.is_some() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;

            // Take exclusive ownership of the notification + wake receivers
            // once (Phase 2 of spec-pump-fix-synthesis.md). Channel reads
            // need no `&mut YaldaGpuiView`, so the old pattern of grabbing
            // the model lock just to call `try_recv` was pure contention with
            // keystrokes and render. After this, the loop only takes the lock
            // to *apply* a pre-drained batch.
            let (mut note_rx, mut wake_rx) = match this.update(cx, |this, _cx| {
                this.session_server
                    .as_mut()
                    .map(|s| (s.take_notification_receiver(), s.take_wake_receiver()))
            }) {
                Ok(Some((Some(rx), wake))) => (rx, wake),
                // No server, or receivers already taken — nothing to pump.
                _ => return,
            };

            // Reconnect backoff: once the wake channel closes (server gone) we
            // retry the connection on this cadence rather than hammering it.
            let reconnect_backoff = Duration::from_millis(1000);
            let mut last_reconnect: Option<std::time::Instant> = None;

            // Per-cycle cap so a runaway producer can't starve other tasks;
            // when we hit it we skip the wait and immediately drain more.
            const DRAIN_CAP: usize = 4096;
            let heartbeat = Duration::from_millis(100);
            let poll_fallback = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let anim_period = Duration::from_millis(120);
            let mut last_anim = std::time::Instant::now();
            // Last thinking-indicator second-fingerprint we repainted for. The
            // probe still runs every `anim_period` (cheap traversal), but we
            // only `cx.notify()` — which forces a full O(transcript) re-render —
            // when the displayed whole-second clock actually changes (~1Hz),
            // not on every 120ms tick.
            let mut last_anim_fp: Option<u64> = None;
            let mut more_pending = false;

            loop {
                // 1. WAIT — event-driven when we have a wake channel, else
                // poll. Skipped entirely when the last cycle hit the cap.
                if !more_pending {
                    let mut wake_closed = false;
                    if let Some(rx) = wake_rx.as_mut() {
                        let timer = cx.background_executor().timer(heartbeat);
                        futures::select_biased! {
                            v = rx.next().fuse() => {
                                if v.is_some() {
                                    // Collapse coalesced wakes; one drain covers them.
                                    while rx.next().now_or_never().flatten().is_some() {}
                                } else {
                                    wake_closed = true;
                                }
                            }
                            _ = timer.fuse() => {}
                        }
                    } else {
                        cx.background_executor().timer(poll_fallback).await;
                    }
                    // Wake channel closed (server reader thread gone): degrade
                    // to polling rather than spinning on an instant `None`.
                    if wake_closed {
                        wake_rx = None;
                    }
                }

                // RECONNECT — when the wake channel is gone the connection
                // dropped. Try to re-establish it (rate-limited) and, on
                // success, splice the fresh receivers back into the loop and
                // re-attach every slot so the durable session resumes.
                if wake_rx.is_none() {
                    let now = std::time::Instant::now();
                    let due =
                        last_reconnect.is_none_or(|t| now.duration_since(t) >= reconnect_backoff);
                    if due {
                        last_reconnect = Some(now);
                        match this.update(cx, |this, cx| this.reconnect_session_server(cx)) {
                            Ok(Some((new_note, new_wake))) => {
                                eprintln!(
                                    "[yalda-gpui] reconnected to session server \
                                     (re-attaching slots)"
                                );
                                note_rx = new_note;
                                wake_rx = Some(new_wake);
                                last_reconnect = None;
                                let _ = this.update(cx, |_t, cx| cx.notify());
                            }
                            Ok(None) => {}    // still down — retry after backoff
                            Err(_) => return, // view dropped
                        }
                    }
                }

                // 2. EXTRACT — drain the channel with no model lock held.
                let mut batch: Vec<ServerNotification> = Vec::new();
                while batch.len() < DRAIN_CAP {
                    match note_rx.try_recv() {
                        Ok(note) => batch.push(note),
                        Err(_) => break,
                    }
                }
                more_pending = batch.len() >= DRAIN_CAP;
                if batch.is_empty() {
                    more_pending = false;
                    // No events — but if a turn is in flight, tick the
                    // thinking animation so the elapsed/quiet timers stay
                    // live during a stall.
                    if last_anim.elapsed() >= anim_period {
                        last_anim = std::time::Instant::now();
                        let _ = this.update(cx, |this, cx| {
                            // Only repaint when the whole-second indicator clock
                            // changed; an unchanged fingerprint means the visible
                            // "Thinking… mm:ss" label is identical, so the full
                            // transcript rebuild a notify() triggers would be
                            // wasted (this is the dominant idle-stall cost).
                            let fp = this.awaiting_anim_fingerprint();
                            if fp.is_some() && fp != last_anim_fp {
                                last_anim_fp = fp;
                                cx.notify();
                            } else if fp.is_none() {
                                last_anim_fp = None;
                            }
                        });
                    }
                    continue;
                }

                // 3. APPLY — one model-lock acquisition for the whole cycle.
                // `apply_server_batch` notifies once if anything changed.
                let perf = perf_enabled();
                let batch_len = batch.len();
                let t_apply = perf.then(std::time::Instant::now);
                if this
                    .update(cx, |this, cx| this.apply_server_batch(batch, cx))
                    .is_err()
                {
                    return; // view dropped
                }
                if let Some(t) = t_apply {
                    eprintln!(
                        "[perf] server-pump apply events={batch_len} \
                         lock_held={:.2}ms more_pending={more_pending}",
                        t.elapsed().as_secs_f64() * 1e3,
                    );
                }

                // Yield between mega-batches so GPUI can repaint.
                if more_pending {
                    cx.background_executor().timer(yield_delay).await;
                }
            }
        });
        self._server_pump = Some(task);
    }

    /// Find an agent slot by its server session id across ALL tabs and tiles,
    /// running `f` on the first match. Returns `true` if a slot was found.
    ///
    /// A single shared `SessionServerClient` multiplexes every session's
    /// notifications onto one pump (`start_server_pump`), so routing must
    /// search the whole workspace — not just the active tab — or a session
    /// living in a background tab silently drops its streamed output. The
    /// scan is cheap: a handful of tabs × tiles × slots.
    /// Route to the single [`AgentSession`] bound to `sid` and run `f` on it,
    /// returning whether one was found (INV-4: 1:1, so zero or one match — the
    /// fan-out is gone). Replaces the old `with_server_session_slot` /
    /// `for_each_server_session_slot` pair.
    pub(crate) fn with_server_session_slot(
        &mut self,
        sid: &str,
        mut f: impl FnMut(&mut AgentSession),
    ) -> bool {
        match self.sessions.get_by_sid_mut(sid) {
            Some(session) => {
                f(session);
                true
            }
            None => false,
        }
    }

    /// Reconcile a server-side close: drop the session for `sid` from the store
    /// and land whichever tile showed it in a LIVE free-session selector. The
    /// tile stays an Agent tile (it does NOT fall back to a buffer — only an
    /// explicit Ctrl-V does that). Returns whether anything changed.
    ///
    /// A tile carrying a `pending_open_token` is mid-respawn (change_agent_cwd
    /// closed the OLD sid and is creating a new one); its in-flight
    /// `SessionClosed(old_sid)` must NOT destroy the respawning session. We
    /// guard that two ways: the respawn path `clear_sid`s before firing the
    /// close (so `locate` already misses), and here we additionally skip a tile
    /// whose token is in flight.
    pub(crate) fn reconcile_session_closed(&mut self, sid: &str, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.sessions.locate(sid) else {
            return false;
        };
        // The session's cwd seeds the replacement selector's list.
        let cwd = self
            .sessions
            .get(id)
            .map(|s| s.cwd.clone())
            .unwrap_or_else(process_cwd);
        // Find the tile that showed this session (at most one, INV-2). Skip a
        // tile mid-respawn (pending_open_token in flight).
        let mut tile_was_respawning = false;
        let mut tile_found = false;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(tile) = content
                    && tile.bound == Some(id)
                {
                    tile_found = true;
                    if tile.pending_open_token.is_some() {
                        tile_was_respawning = true;
                    } else {
                        tile.bound = None;
                        tile.picker = Some(SessionPicker::loading(cwd.clone()));
                    }
                }
            });
        }
        if tile_was_respawning {
            // Leave the respawning session alone; its new sid will rebind.
            return false;
        }
        self.sessions.close(id);
        // Kick the (now-unbound) tile's selector list off the paint thread. The
        // list reducer (`apply_picker_sessions`) guards on the focused tile
        // still loading, so a no-op for a background tile is harmless.
        if tile_found && self.session_server.is_some() {
            self.spawn_list_sessions_for_picker(cwd, cx);
        }
        true
    }

    /// Reconcile a server-side rename: update the label on the session for
    /// `sid`. Returns whether anything changed.
    pub(crate) fn reconcile_session_renamed(&mut self, sid: &str, label: &str) -> bool {
        if let Some(session) = self.sessions.get_by_sid_mut(sid) {
            if session.label != label {
                session.label = label.to_string();
                return true;
            }
        }
        false
    }

    /// Apply a pre-drained batch of server notifications to the model. Called
    /// inside a single `this.update()` so the model lock is held only for the
    /// state mutation, never for channel I/O. Returns whether anything
    /// changed and emits exactly one `cx.notify()` for the whole batch.
    pub(crate) fn apply_server_batch(
        &mut self,
        batch: Vec<ServerNotification>,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_candidate = self.is_candidate;
        // This GUI's stable lease client_id, for the LeaseChanged check below
        // (released == unleased OR held by us). Captured up front so the
        // per-note loop doesn't re-borrow `session_server`.
        let my_client_id: Option<String> = self.session_server.as_ref().map(|s| s.client_id());
        let mut ready_change: Option<bool> = None;

        let warn_unrouted = |routed: bool, sid: &str| {
            if !routed {
                eprintln!(
                    "[yalda-gpui] pump: no slot for server session {}",
                    &sid[..sid.len().min(8)],
                );
            }
        };

        // A single shared `SessionServerClient` multiplexes every session's
        // notifications onto this one pump, so each note is routed by its
        // `session_id` across the *whole* workspace (all tabs and tiles), not
        // just the active tab — otherwise a session living in a background
        // tab silently drops its streamed output.
        let did_work = !batch.is_empty();
        // Sessions that received at least one ReplyEvent in this batch. The
        // follow-scroll is hoisted out of the per-event loop and applied once
        // per affected session below: only the *final* scroll position matters,
        // and a batch can hold thousands of chunk events (DRAIN_CAP), so doing
        // the workspace walk + scroll bookkeeping per event was O(events ×
        // workspace) wasted work during fast streaming.
        let mut scrolled_sessions: Vec<String> = Vec::new();
        // Perf: a streaming batch is overwhelmingly a run of ReplyEvent chunks
        // for the SAME session. Previously each chunk re-walked every tab+tile
        // to find the slot (O(events*tiles)) and cloned the event String into a
        // throwaway `vec![event.clone()]`. Coalesce consecutive same-session
        // ReplyEvents into one slot lookup + one `apply_reply_events` call,
        // moving the events by value (no per-chunk clone). This keeps ordering
        // relative to other event kinds (we only merge adjacent ReplyEvents)
        // while collapsing routing to O(distinct_runs*tiles) and shortening the
        // model-lock hold time.
        let mut batch = batch.into_iter().peekable();
        while let Some(note) = batch.next() {
            match note {
                ServerNotification::ReplyEvent { session_id, event } => {
                    // Drain the contiguous run of ReplyEvents for this session.
                    let mut events = vec![event];
                    while let Some(ServerNotification::ReplyEvent {
                        session_id: next_sid,
                        ..
                    }) = batch.peek()
                    {
                        if *next_sid != session_id {
                            break;
                        }
                        match batch.next() {
                            Some(ServerNotification::ReplyEvent { event, .. }) => {
                                events.push(event)
                            }
                            _ => unreachable!(),
                        }
                    }
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let claude = &mut slot.state;
                        // §9 gate: once the canonical `AgentEvent` stream is the
                        // authoritative driver for this session (it has forwarded
                        // a real `TurnEnded`), the legacy `ReplyEvent` stream is
                        // INERT — applying its chunks too would double-render
                        // (the sharp double-apply risk). The reducer owns the
                        // transcript from here on.
                        if claude.agent_stream_authoritative {
                            return;
                        }
                        // The server path finalizes on its own `TurnEnded`
                        // notification, but if a `ReplayComplete` marker is
                        // forwarded in the event stream, honor it here too so
                        // the resumed transcript finalizes exactly once
                        // (Finding 13, INV-4).
                        if Self::apply_reply_events(claude, std::mem::take(&mut events)) {
                            // Idempotent finalize keyed on (generation, turn):
                            // the legacy ReplayComplete and a forwarded
                            // `AgentEvent` ReplayEnd for the same boundary
                            // finalize the transcript exactly once.
                            let gen_ = claude.generation;
                            let turn = claude.replay_turns.last_seen;
                            if claude.finalize_agent_turn_idem(gen_, turn) {
                                claude.turn_phase = TurnPhase::Idle;
                            }
                        }
                    });
                    if routed && !scrolled_sessions.iter().any(|s| s == &session_id) {
                        scrolled_sessions.push(session_id.clone());
                    }
                    warn_unrouted(routed, &session_id);
                }
                // Phase-8 Stage C (ADDITIVE, spec §9): the canonical AgentEvent
                // stream is forwarded ALONGSIDE the legacy ReplyEvent/TurnEnded/
                // UserPrompt variants. The GUI now folds it through the TOTAL
                // reducer (`apply_agent_event`) — but to avoid the sharp double-
                // apply risk (chunks applied from BOTH streams), the per-session
                // `has_forwarded_turn_ended_in_stream` gate
                // (`agent_stream_authoritative`) picks exactly ONE driver:
                //   - until the session's FIRST forwarded `TurnEnded`, the
                //     legacy `ReplyEvent`/inference path drives the transcript
                //     and the reducer runs only to OBSERVE the boundary (which
                //     flips the gate) — it makes no transcript mutation that the
                //     legacy stream also makes, except the idempotent finalize;
                //   - once authoritative, the reducer drives mutation and the
                //     legacy `ReplyEvent` arm goes inert for this session (see
                //     the guard there). The still-live inference is neutralised
                //     by the idempotent `(generation, turn)` finalize ledger.
                // NEEDS-RUNTIME: GPUI is not headlessly verifiable end-to-end.
                ServerNotification::Agent { event } => {
                    // Drain the contiguous run of Agent events for this session
                    // (same coalescing as the ReplyEvent run above): one slot
                    // lookup + one borrow for a streaming burst.
                    let session_id = event.session_id.clone();
                    let mut events = vec![event];
                    while let Some(ServerNotification::Agent { event: next }) = batch.peek() {
                        if next.session_id != session_id {
                            break;
                        }
                        match batch.next() {
                            Some(ServerNotification::Agent { event }) => events.push(event),
                            _ => unreachable!(),
                        }
                    }
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let claude = &mut slot.state;
                        for event in &events {
                            // BEFORE the gate flips, the reducer would race the
                            // legacy stream and double-apply mutating events, so
                            // we only let it OBSERVE the boundary (which sets
                            // `agent_stream_authoritative`) and finalize
                            // idempotently. AFTER it flips, the reducer owns the
                            // transcript and the legacy ReplyEvent arm is inert.
                            let authoritative_before = claude.agent_stream_authoritative;
                            let is_boundary = matches!(
                                &event.kind,
                                yalda::agent_event::AgentEventKind::TurnEnded { .. }
                            );
                            if authoritative_before || is_boundary {
                                let effect = Self::apply_agent_event(claude, event);
                                Self::settle_agent_effect(claude, effect);
                            }
                        }
                    });
                    if routed && !scrolled_sessions.iter().any(|s| s == &session_id) {
                        scrolled_sessions.push(session_id.clone());
                    }
                    warn_unrouted(routed, &session_id);
                }
                // Legacy explicit boundary. During the §9 additive rollout this
                // coexists with the forwarded `AgentEvent::TurnEnded`; both route
                // through the idempotent `(generation, turn)` finalize ledger so
                // the boundary finalizes EXACTLY ONCE regardless of which stream
                // (or both) delivers it. `generation` is now consumed: the
                // ledger key matches the reducer's, so a forwarded boundary and
                // this legacy one for the same channel/turn collapse.
                ServerNotification::TurnEnded {
                    session_id,
                    turn_count,
                    generation,
                } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let claude = &mut slot.state;
                        // Turn boundary: clear last-inserted so the next turn's
                        // user echo isn't mistaken for a duplicate of this one.
                        claude.reconciler.note_turn_progressed();
                        claude.replay_turns.last_seen = turn_count;
                        // The legacy boundary carries the channel generation; if
                        // the reducer hasn't observed a generation yet (gen 0,
                        // pre-rebaseline) use the slot's current one so the two
                        // streams' ledger keys agree.
                        let gen_ = if generation > 0 {
                            generation
                        } else {
                            claude.generation
                        };
                        // Bug 3: key the ledger on the SAME basis as the forwarded
                        // `Agent { TurnEnded }` reducer arm, which uses the 0-based
                        // envelope `turn` (the server sets `completed_turn =
                        // turns - 1` at session-server main.rs:1290). The legacy
                        // `turn_count` is the 1-based SETTLED count, so convert it
                        // to the 0-based completed-turn index before keying —
                        // otherwise the forwarded boundary (key `turns-1`) and this
                        // legacy boundary (key `turns`) are DISTINCT and BOTH
                        // finalize, defeating the §7/§9 exactly-once backstop.
                        let completed_turn = turn_count.saturating_sub(1);
                        if claude.finalize_agent_turn_idem(gen_, completed_turn) {
                            claude.turn_phase = TurnPhase::Idle;
                        }
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::UserPrompt { session_id, text } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        // Route through the single chokepoint as an `Echo`: the
                        // reconciler suppresses it when it matches our own
                        // optimistic submit (live) or a turn already inserted by
                        // a second source (replay), and inserts it otherwise.
                        // Server-managed slots never advance the replay boundary
                        // here — their boundaries arrive as replayed `TurnEnded`.
                        slot.state.insert_user_turn(
                            &text,
                            yalda::agent_transcript::UserTurnOrigin::Echo,
                            false,
                        );
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionAttached {
                    session_id,
                    acp_session_id,
                } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let label = acp_session_id.as_deref().unwrap_or("connected");
                        let msg = format!("attached: {label}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionDetached { session_id, reason } => {
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let msg = format!("detached: {reason}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::LeaseChanged { session_id, lease } => {
                    if is_candidate {
                        // Released == unleased (None) OR (defensively) already
                        // held by our OWN client_id. Either way a Promote will
                        // succeed, so the candidate may take over.
                        let released = lease
                            .as_ref()
                            .is_none_or(|l| Some(&l.client_id) == my_client_id.as_ref());
                        ready_change = Some(released);
                        self.with_server_session_slot(&session_id, |slot| {
                            let msg = if released {
                                "original released — menu → claude → take over"
                            } else {
                                "mirroring (original active) — read-only"
                            };
                            Self::append_system_notice(&mut slot.state, msg);
                            slot.state.status = Some(msg.into());
                        });
                    }
                }
                ServerNotification::SessionCreated { session } => {
                    // List-level signal that some connection created a session.
                    // The primary GUI does not auto-add it to unrelated tiles
                    // (a new session belongs to the tile that opened it, which
                    // already has its slot from the create response). Kept as a
                    // no-op hook for a future "available sessions" view / for
                    // mirror GUIs that want to surface every live session.
                    let _ = &session;
                }
                ServerNotification::SessionClosed { session_id } => {
                    // A session closed somewhere (this GUI, another tile, or
                    // another GUI instance). Drop it from the store and land its
                    // tile in a live selector.
                    self.reconcile_session_closed(&session_id, cx);
                }
                ServerNotification::SessionRenamed { session_id, label } => {
                    self.reconcile_session_renamed(&session_id, &label);
                }
                ServerNotification::PromptRejected {
                    session_id,
                    reason,
                    text,
                } => {
                    // The server refused the prompt (another window holds the
                    // lease). The optimistic echo is already frozen in the
                    // transcript, so without this notice the message would
                    // LOOK sent while the agent never received it. Say so in
                    // the transcript + status line, and put the text back in
                    // the chatbox (only if the user hasn't typed something
                    // new) so a resubmit is one keypress.
                    let routed = self.with_server_session_slot(&session_id, |slot| {
                        let msg = format!("✗ message NOT delivered: {reason}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                        if let Some(cb) = slot.state.input_surface.chatbox_mut()
                            && cb.text().trim().is_empty()
                        {
                            let mut fresh = Chatbox::new();
                            for ch in text.chars() {
                                fresh.editor.insert_char(ch);
                            }
                            *cb = fresh;
                        }
                    });
                    warn_unrouted(routed, &session_id);
                }
            }
        }
        // Single follow-scroll per session that streamed this batch, instead of
        // once per chunk event. Uses the stale `list_item_count` exactly as the
        // per-event path did (the authoritative re-scroll with the fresh count
        // happens later in render_agent after the ListState splice); this just
        // keeps unfocused tiles that miss render's scroll roughly pinned.
        for sid in &scrolled_sessions {
            self.with_server_session_slot(sid, |slot| {
                let claude = &mut slot.state;
                // Stale-count pre-pin; the authoritative reveal with the fresh
                // post-reconcile count runs in render_agent
                // (`reveal_tail_if_following`). This does NOT stamp
                // `last_scrolled_edit_seq`, so it never suppresses that
                // render-time reveal. Shares the `follow_tail` decision (F4).
                if claude.follow_tail() && claude.list_item_count > 0 {
                    claude
                        .list_state
                        .scroll_to_reveal_item(claude.list_item_count - 1);
                }
            });
        }
        // Deferred apply outside the layout borrow.
        if let Some(ready) = ready_change {
            self.candidate_promote_ready = ready;
        }
        if did_work {
            cx.notify();
        }
        did_work
    }

    /// Pump a specific session by its stable [`SessionId`]. Returns `true` if
    /// the per-tick budget was hit and more events may be queued. Returns
    /// `false` when the session is gone (pump task should exit) or the
    /// queue is drained.
    pub(crate) fn pump_session(&mut self, id: SessionId, cx: &mut Context<Self>) -> bool {
        const PUMP_EVENT_BUDGET: usize = 64;

        // The session lives in the store (disjoint from the layout tree); route
        // directly. Returns (has_events, more_pending, attached_with_id) so
        // post-borrow work (persistence) can proceed.
        let (has_events, more_pending, attached_with_id) = {
            let claude = match self.sessions.get_mut(id) {
                Some(session) => &mut session.state,
                None => return false, // session gone: pump task should exit
            };

            // 1) Resolve pending attach.
            let mut attach_resolved = false;
            let mut attached_with_id = false;
            if let Some(rx) = &claude.attach_pending {
                match rx.try_recv() {
                    Ok(Ok(client)) => {
                        let label = client.description();
                        if client.session_id().is_some() {
                            attached_with_id = true;
                        }
                        claude.channel = Some(client);
                        let msg = format!("attached: {label}");
                        Self::append_system_notice(claude, &msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                    Ok(Err(e)) => {
                        claude.channel = None;
                        let msg = format!("attach failed: {e} (set YALDA_ACP_AGENT=...?)");
                        Self::append_system_notice(claude, &msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        let msg = "attach worker died before reporting result";
                        Self::append_system_notice(claude, msg);
                        claude.status = Some(msg.into());
                        attach_resolved = true;
                    }
                }
            }
            if attach_resolved {
                claude.attach_pending = None;
            }

            // 2) Worker dropped (channel closed)?
            let stale = claude
                .channel
                .as_ref()
                .map(|c| !c.is_connected())
                .unwrap_or(false);
            if stale {
                claude.channel = None;
                // Channel gone → no turn can be in flight; drop to Idle so the
                // spinner/timer can't strand (Finding 9). The prior code cleared
                // only `turn_started`, leaving `awaiting_reply` stuck true.
                claude.turn_phase = TurnPhase::Idle;
                Self::append_system_notice(claude, "agent disconnected");
                claude.status = Some("agent disconnected".into());
                cx.notify();
                return false;
            }

            // 3) Drain up to PUMP_EVENT_BUDGET reply events.
            let mut events: Vec<yalda::acp_channel::ReplyEvent> = Vec::new();
            let mut current_turns = claude.replay_turns.last_seen;
            let mut more_pending = false;
            if let Some(client) = &claude.channel {
                while events.len() < PUMP_EVENT_BUDGET {
                    match client.try_recv() {
                        Some(ev) => events.push(ev),
                        None => break,
                    }
                }
                if events.len() == PUMP_EVENT_BUDGET {
                    more_pending = client
                        .try_recv()
                        .map(|ev| {
                            events.push(ev);
                            true
                        })
                        .unwrap_or(false);
                }
                current_turns = client.turn_count();
            }
            // A live turn ends when the agent's prompt-response settles and
            // bumps the turn counter. Replay (`session/load`) never fires a
            // prompt response — its end is the explicit `ReplayComplete`
            // marker (Finding 13, INV-4) returned by `apply_reply_events`, so
            // a transiently-empty queue between notification bursts can no
            // longer infer turn-end and finalize mid-replay.
            let turn_ended = !more_pending && current_turns > claude.replay_turns.last_seen;
            let has_events = !events.is_empty() || turn_ended;
            if has_events {
                let mut replay_complete = Self::apply_reply_events(claude, events);
                if turn_ended {
                    // Drain any straggler events that queued after the budget
                    // cut so they're applied before we finalize.
                    let mut tail: Vec<yalda::acp_channel::ReplyEvent> = Vec::new();
                    if let Some(client) = &claude.channel {
                        while let Some(ev) = client.try_recv() {
                            tail.push(ev);
                        }
                    }
                    replay_complete |= Self::apply_reply_events(claude, tail);
                    claude.replay_turns.last_seen = current_turns;
                }
                if turn_ended || replay_complete {
                    // Idempotent finalize keyed on (generation, turn): the
                    // direct-pump inference and any forwarded `AgentEvent`
                    // boundary for the same (generation, turn) collapse to one
                    // finalize. The direct (logless) path has no server-side
                    // generation, so the slot's current generation keys it.
                    let gen_ = claude.generation;
                    let turn = claude.replay_turns.last_seen;
                    if claude.finalize_agent_turn_idem(gen_, turn) {
                        claude.turn_phase = TurnPhase::Idle;
                    }
                }
                // Spec §19 auto-scroll. Shares the `follow_tail` decision (F4).
                // Stale-count pre-pin only; the authoritative reveal with the
                // fresh post-reconcile count runs in render_agent
                // (`reveal_tail_if_following`), so this does NOT stamp
                // `last_scrolled_edit_seq`.
                if claude.follow_tail() && claude.list_item_count > 0 {
                    claude
                        .list_state
                        .scroll_to_reveal_item(claude.list_item_count - 1);
                }
            }

            (has_events, more_pending, attached_with_id)
        };

        // Post-borrow: persist the whole ring snapshot so the just-attached
        // slot's id (or its preserved resume_id, if load failed) lands on
        // disk. Writing the snapshot (not just the one slot) is what makes
        // a stale pump from a removed slot safe — it contributes nothing
        // if its slot isn't in the ring anymore.
        if attached_with_id {
            self.save_agent_ring();
        }

        if has_events {
            cx.notify();
        }
        more_pending
    }

    /// Insert a lifecycle notice into the agent buffer as a frozen line.
    /// The `―` prefix distinguishes system notices from agent prose.
    /// Splice a yalda-local lifecycle notice into the transcript. Tagged
    /// `TurnId::System` — NOT `Llm(k)` — so it never masquerades as an agent
    /// turn: it carries no turn number, emits no Claude `TurnHeader`, renders
    /// a blank gutter, and is excluded from agent-turn numbering. Because the
    /// next agent chunk's `Llm(k)` lookup keys off the last `Llm`-tagged line,
    /// a `System`-tagged notice can't perturb it (Finding 5, INV-3).
    pub(crate) fn append_system_notice(claude: &mut AgentState, msg: &str) {
        // Ensure the transcript ends on a newline so the notice starts on its
        // OWN line. Otherwise the notice's leading `\n` splices onto the prior
        // (possibly in-flight `Llm(k)`) line, and `append_llm_chunk` re-tags
        // that whole line `System` — silently demoting agent prose. Mirrors
        // `freeze_as_user_turn`'s boundary guard (Finding 5, INV-3).
        let doc = claude.editor.document();
        if !doc.is_empty() && doc.last_char() != Some('\n') {
            let eof = doc.rope().len_chars();
            claude.editor.programmatic_insert(eof, "\n");
        }
        let notice_line = format!("― {msg}\n");
        claude.editor.append_llm_chunk(TurnId::System, &notice_line);
    }

    /// Apply a batch of reply events to the AgentState. Text chunks are
    /// spliced into the buffer; tool calls land in `tool_calls` and are
    /// anchored to whatever buffer line is the current end-of-frozen so
    /// the renderer can slot the tool block in between text on either
    /// side. Updates merge into existing tool calls via `ToolCall::update`.
    /// Apply a batch of events, returning `true` if a `ReplayComplete`
    /// marker was seen (Finding 13, INV-4) — the caller then finalizes the
    /// turn exactly once. Returning the signal (rather than finalizing here)
    /// keeps finalize a pump-side decision colocated with the live
    /// `turn_ended` path.
    pub(crate) fn apply_reply_events(
        claude: &mut AgentState,
        events: Vec<yalda::acp_channel::ReplyEvent>,
    ) -> bool {
        use yalda::acp_channel::ReplyEvent;
        // Any inbound activity refreshes the quiet-clock the thinking
        // indicator reads, so a streaming turn never looks stalled. A no-op
        // when idle (e.g. replay events arriving outside an awaited turn).
        if !events.is_empty() {
            claude.turn_phase.note_event(std::time::Instant::now());
        }
        let mut replay_complete = false;
        for ev in events {
            // In-progress turn for tagging streamed content, resolved per
            // event so a replayed `UserMessage` boundary mid-batch advances
            // the turn for the chunks that follow it (Finding 3, INV-3).
            // `current_turn()` is the single source of `k` (live submit and
            // replay agree): live turns read `replay_turns.last_seen + 1`;
            // during replay the boundary-advanced cursor takes over.
            let current_turn = claude.current_turn();
            match ev {
                ReplyEvent::Chunk(text) => {
                    // Spec §E3: append at the end of the last frozen line
                    // tagged with this turn (mid-line for in-progress
                    // continuation, EOF for a new turn). Editable user
                    // lines anywhere else in the document stay put.
                    if perf_enabled() {
                        eprintln!("[chunklog gui] turn={current_turn} {text:?}");
                    }
                    // Real content means a retry (if any) succeeded — drop
                    // the transient "retrying…" notice.
                    claude.status = None;
                    claude
                        .editor
                        .append_llm_chunk(TurnId::Llm(current_turn), text.as_str());
                }
                ReplyEvent::ToolCallStarted(mut tc) => {
                    cap_tool_call_payloads(&mut tc);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor);
                    // Parse the protocol id into the domain key ONCE here, at
                    // the boundary where a ToolCall enters apply_reply_events
                    // (Finding 7). All tool maps below are keyed on it.
                    let id = ToolCallKey::from_id(&tc.tool_call_id);
                    // Tag the anchor with `Tool(k)` so the gutter shows
                    // `Tk` on tool-group anchor lines (§11).
                    claude
                        .editor
                        .metadata_mut::<TurnId>()
                        .insert(anchor, TurnId::Tool(current_turn));
                    // Register through the one chokepoint so order+calls+anchor
                    // can't drift. Sub-agent classification (§25) is derived on
                    // demand from `tools` — see `AgentState::subagents()`.
                    claude.tools.register(id, tc, anchor);
                }
                ReplyEvent::ToolCallUpdated(upd) => {
                    let id = ToolCallKey::from_id(&upd.tool_call_id);
                    if let Some(existing) = claude.tools.call_mut(&id) {
                        existing.update(upd.fields);
                        cap_tool_call_payloads(existing);
                        // No sub-agent mirror to update: `subagents()`
                        // derives label + status live from the tool call we
                        // just mutated (ADR-0006 quick win #1).
                    } else {
                        // Update arrived for a tool call we never saw the
                        // start for (rare, but possible if the worker
                        // dropped an early notification). Synthesize an
                        // entry so the user still sees it.
                        let mut tc = yalda::acp_channel::ToolCall::new(
                            upd.tool_call_id.clone(),
                            String::new(),
                        );
                        tc.update(upd.fields);
                        cap_tool_call_payloads(&mut tc);
                        let anchor = anchor_for_new_tool_call(&mut claude.editor);
                        claude
                            .editor
                            .metadata_mut::<TurnId>()
                            .insert(anchor, TurnId::Tool(current_turn));
                        // Sub-agent entry (if any) is derived by `subagents()`.
                        claude.tools.register(id, tc, anchor);
                    }
                }
                ReplyEvent::PlanUpdated(plan) => {
                    // Full snapshot replaces the previous plan (§21).
                    claude.current_plan = Some(plan);
                }
                ReplyEvent::ModeChanged(mode_id) => {
                    claude.agent_mode = Some(mode_id);
                }
                ReplyEvent::UsageUpdated(snap) => {
                    claude.usage = Some(snap);
                }
                ReplyEvent::Notice(ref msg) => {
                    // Driver status (retry/failed) — show inline in the
                    // buffer and in the footer status slot.
                    Self::append_system_notice(claude, msg);
                    claude.status = Some(msg.clone().into());
                }
                ReplyEvent::UserMessage(text) => {
                    // A user-authored turn surfaced by the agent's
                    // `UserMessageChunk` (Finding 1 / defect B, INV-1, INV-6).
                    // Emitted unconditionally by the worker — both live (an
                    // echo of the prompt Submit already inserted) and on
                    // `session/load` replay (reconstructing prior prompts). The
                    // single chokepoint's reconciler suppresses the live echo
                    // by content identity (order-independent — the old
                    // suffix check double-rendered whenever a chunk streamed
                    // first) and inserts genuine replayed turns. Only the
                    // direct-channel replay path advances the replay boundary
                    // (`!server_managed`): there is no replayed `TurnEnded` to
                    // bump the live counter, so each user boundary must step
                    // the cursor — User(1),Llm(1),User(2),Llm(2)…. A suppressed
                    // echo never advances, so the live counter is safe.
                    let advance = !claude.server_managed;
                    claude.insert_user_turn(
                        &text,
                        yalda::agent_transcript::UserTurnOrigin::Echo,
                        advance,
                    );
                }
                ReplyEvent::ReplayComplete => {
                    // The agent finished re-emitting the prior conversation
                    // (Finding 13, INV-4). Fold the replay cursor back into
                    // the live counter so the next live turn continues from
                    // the right `k`, then signal the pump to finalize once —
                    // after the last replayed chunk, never mid-replay.
                    claude.finish_replay();
                    claude.reconciler.note_turn_progressed();
                    replay_complete = true;
                }
                ReplyEvent::TurnEnded { count } => {
                    // 8b additive (ADR-0006): the worker's authoritative turn
                    // boundary. INERT this stage — the pump's "queue empty +
                    // counter climbed" inference still drives finalize. Log
                    // whether the explicit signal agrees with what we inferred
                    // (last_seen), so agreement can be confirmed on real
                    // sessions before the inference is deleted. Only reaches
                    // here when YALDA_EMIT_TURN_ENDED=1.
                    eprintln!(
                        "[yalda-gpui] explicit TurnEnded count={count}; \
                         inferred last_seen={} (agree={})",
                        claude.replay_turns.last_seen,
                        count == claude.replay_turns.last_seen,
                    );
                }
            }
        }
        replay_complete
    }

    /// TOTAL reducer over the canonical [`AgentEventKind`] vocabulary (spec §7).
    ///
    /// This is the Stage C end-state of the GUI reducer: one `match` driven once
    /// per [`AgentEvent`], EXHAUSTIVE over every kind so a new variant is a
    /// compile error, with EXPLICIT arms for `Unknown` (render nothing + one
    /// diagnostic, bytes still round-trip through the durable log) and
    /// `CompactedSummary` (a deterministic "history compacted" placeholder, NOT
    /// a silent gap). Turn-numbered content is tagged by the FORWARDED
    /// `event.turn` (spec §5), not by `current_turn()` inference.
    ///
    /// ## What it does NOT do
    ///
    /// Turn FINALIZATION is a pump-side decision (spec §7) — the fold does NOT
    /// finalize or flip `turn_phase` inside itself. It returns a
    /// [`AgentEventEffect`] telling the caller what boundary it observed; the
    /// caller routes that through the idempotent `finalize_agent_turn_idem`
    /// ledger and owns the `turn_phase = Idle` flip. This keeps the reducer a
    /// pure state-fold and the finalize idempotent across the dual streams.
    ///
    /// ## Rebaseline (spec §4)
    ///
    /// The uniform generation rule lives HERE: any event whose `generation` is
    /// strictly greater than `claude.generation` runs `reset_for_replay` FIRST,
    /// then advances `claude.generation`. `ChannelOpened` guarantees the bump
    /// arrives as the channel's first event, but the rule is idempotent if a
    /// later event is first-observed (a stray older-generation event after the
    /// bump is ignored — its `generation < claude.generation`).
    ///
    /// ## Additive gate (spec §9)
    ///
    /// During rollout this is invoked ONLY for transcript mutation once the
    /// session is `agent_stream_authoritative` (it has seen a real forwarded
    /// `TurnEnded`); until then the legacy `ReplyEvent`/inference path is the
    /// sole driver and the `Agent` stream is observed diagnostically (so chunks
    /// are never double-applied from both streams). The caller enforces that
    /// gate; this method assumes it owns the stream when called.
    pub(crate) fn apply_agent_event(
        claude: &mut AgentState,
        event: &yalda::agent_event::AgentEvent,
    ) -> AgentEventEffect {
        use yalda::agent_event::{AgentEventKind, ChunkRole, TurnOutcome};

        // ── Uniform rebaseline rule (spec §4) ───────────────────────────────
        // A strictly-newer generation means a respawned channel; rebuild from
        // scratch BEFORE applying this (the channel's first) event, then adopt
        // the new generation. Idempotent: equal/older generations skip it, and
        // an older-than-current event after a bump is dropped below.
        if event.generation > claude.generation {
            claude.reset_for_replay();
            claude.generation = event.generation;
        } else if event.generation < claude.generation {
            // A late straggler from a superseded channel — ignore it so it
            // can't perturb the rebaselined transcript (spec §4 idempotency).
            return AgentEventEffect::None;
        }

        // Any inbound activity refreshes the quiet-clock the thinking indicator
        // reads, so a streaming turn never looks stalled (parity with
        // `apply_reply_events`). A no-op when idle.
        claude.turn_phase.note_event(std::time::Instant::now());

        // The authoritative turn number rides the envelope (spec §5); content
        // is tagged by it directly, NOT by `current_turn()` inference.
        let turn = event.turn as usize;

        match &event.kind {
            AgentEventKind::ChannelOpened { resumed: _ } => {
                // The rebaseline already ran above (this is the channel's first
                // event); nothing more to mutate. The status line is owned by
                // the attach/reconnect path, so this arm is a near-no-op.
                AgentEventEffect::None
            }
            AgentEventKind::Chunk { text, role } => {
                claude.status = None;
                match role {
                    ChunkRole::Message => {
                        claude
                            .editor
                            .append_llm_chunk(TurnId::Llm(turn), text.as_str());
                    }
                    ChunkRole::Thought => {
                        // Thought text un-parks the parked `AgentThoughtChunk`
                        // path. Until a dedicated thought style ships it shares
                        // the LLM-turn surface (tagged by the same turn) so the
                        // reasoning is still attributed to the right turn and
                        // never silently dropped.
                        claude
                            .editor
                            .append_llm_chunk(TurnId::Llm(turn), text.as_str());
                    }
                }
                AgentEventEffect::None
            }
            AgentEventKind::ToolCallStarted(tc) => {
                let mut tc = tc.clone();
                cap_tool_call_payloads(&mut tc);
                let anchor = anchor_for_new_tool_call(&mut claude.editor);
                let id = ToolCallKey::from_id(&tc.tool_call_id);
                claude
                    .editor
                    .metadata_mut::<TurnId>()
                    .insert(anchor, TurnId::Tool(turn));
                claude.tools.register(id, tc, anchor);
                AgentEventEffect::None
            }
            AgentEventKind::ToolCallUpdated(upd) => {
                let id = ToolCallKey::from_id(&upd.tool_call_id);
                if let Some(existing) = claude.tools.call_mut(&id) {
                    existing.update(upd.fields.clone());
                    cap_tool_call_payloads(existing);
                } else {
                    let mut tc =
                        yalda::acp_channel::ToolCall::new(upd.tool_call_id.clone(), String::new());
                    tc.update(upd.fields.clone());
                    cap_tool_call_payloads(&mut tc);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor);
                    claude
                        .editor
                        .metadata_mut::<TurnId>()
                        .insert(anchor, TurnId::Tool(turn));
                    claude.tools.register(id, tc, anchor);
                }
                AgentEventEffect::None
            }
            AgentEventKind::PlanUpdated(plan) => {
                claude.current_plan = Some(plan.clone());
                AgentEventEffect::None
            }
            AgentEventKind::ModeChanged(mode_id) => {
                claude.agent_mode = Some(mode_id.clone());
                AgentEventEffect::None
            }
            AgentEventKind::UsageUpdated(snap) => {
                claude.usage = Some(snap.clone());
                AgentEventEffect::None
            }
            AgentEventKind::Notice { kind: _, msg } => {
                // Transient status ONLY (spec §1): terminal failure is now a
                // `TurnEnded { Failed }` boundary, not a Notice. Surface inline
                // + in the footer, exactly as the legacy `Notice` arm did.
                Self::append_system_notice(claude, msg);
                claude.status = Some(msg.clone().into());
                AgentEventEffect::None
            }
            AgentEventKind::UserMessage { text } => {
                // Live submit + replay echo unified. Dedup is by identity
                // (session, generation, turn) via the reconciler, which already
                // suppresses our own optimistic insert. Server-managed slots do
                // not advance the replay boundary (their boundaries arrive as
                // forwarded `TurnEnded`); a logless direct channel does.
                let advance = !claude.server_managed;
                claude.insert_user_turn(
                    text,
                    yalda::agent_transcript::UserTurnOrigin::Echo,
                    advance,
                );
                AgentEventEffect::None
            }
            AgentEventKind::TurnEnded { outcome } => {
                // The authoritative boundary (spec §5). Mark the session as
                // agent-stream-authoritative the first time a real forwarded
                // boundary lands (the per-session §9 gate), so subsequent
                // events drive the reducer instead of the inference.
                claude.agent_stream_authoritative = true;
                claude.reconciler.note_turn_progressed();
                match outcome {
                    TurnOutcome::ReplayEnd => {
                        // Old `ReplayComplete`: fold the replay cursor back into
                        // the live counter. The marker is NOT a turn boundary —
                        // its finalize is settled by the caller through a
                        // dedicated replay-prefix idempotency, NOT the per-turn
                        // `(generation, turn)` ledger (whose key it would share
                        // with the next live turn → stuck-thinking after resume).
                        claude.finish_replay();
                        AgentEventEffect::ReplayEnded
                    }
                    TurnOutcome::Failed { msg } => {
                        // Terminal failure surfaces a status line, then ends the
                        // turn (it is a boundary, not a transient Notice).
                        claude.status = Some(msg.clone().into());
                        claude.replay_turns.last_seen = turn;
                        AgentEventEffect::TurnEnded {
                            generation: event.generation,
                            turn,
                        }
                    }
                    TurnOutcome::Completed
                    | TurnOutcome::Cancelled
                    | TurnOutcome::MaxTokens
                    | TurnOutcome::Refusal => {
                        // The live counter follows the forwarded turn so the
                        // next turn numbers correctly even with inference off.
                        claude.replay_turns.last_seen = turn;
                        AgentEventEffect::TurnEnded {
                            generation: event.generation,
                            turn,
                        }
                    }
                }
            }
            AgentEventKind::CompactedSummary {
                through_turn,
                summary,
            } => {
                // EXPLICIT arm (spec §7): the in-memory ring trimmed a prefix
                // and surfaced this marker instead of a silent gap. Render a
                // deterministic placeholder so a from-base rebuild shows
                // "history compacted" rather than starting mid-conversation.
                let note = if summary.is_empty() {
                    format!("history compacted through turn {through_turn}")
                } else {
                    format!("history compacted through turn {through_turn}: {summary}")
                };
                Self::append_system_notice(claude, &note);
                AgentEventEffect::None
            }
            AgentEventKind::Unknown { tag, .. } => {
                // EXPLICIT arm (spec §7/§8): a variant this GUI version doesn't
                // understand. Render NOTHING (so an old decoder doesn't show a
                // broken block) but emit one diagnostic — the bytes still
                // round-trip verbatim through the durable WAL for a newer node.
                // Deduped per distinct tag (the comment said "one diagnostic"
                // but it fired per-event): a resumed pre-fix session can hold
                // hundreds of legacy tool-call records that pre-date the
                // tool-kind/event-tag collision fix, which would otherwise flood
                // the log. One line per kind keeps real forward-compat signal.
                Self::log_unknown_agent_event_once(tag, event);
                AgentEventEffect::None
            }
        }
    }

    /// Apply the pump-side boundary decision an [`AgentEventEffect`] carries
    /// (spec §7): route a turn boundary through the idempotent
    /// `finalize_agent_turn_idem` ledger and flip `turn_phase` to `Idle` only
    /// when a finalize actually happened. This is the SINGLE place the
    /// `AgentEvent` reducer's finalize lands, so the dual-stream duplicate
    /// `TurnEnded` (forwarded event + lingering inference) finalizes once.
    /// Log an unrecognized AgentEvent kind at most ONCE per distinct tag for the
    /// process lifetime. A forward-compat `Unknown` is worth a single note, not a
    /// per-event flood — a resumed pre-fix session can replay hundreds of legacy
    /// records (e.g. tool calls written before the tool-kind/event-tag collision
    /// fix). The reducer runs on the GPUI foreground thread, so a thread-local
    /// set needs no lock.
    pub(crate) fn log_unknown_agent_event_once(tag: &str, event: &yalda::agent_event::AgentEvent) {
        thread_local! {
            static SEEN: RefCell<std::collections::HashSet<String>> =
                RefCell::new(std::collections::HashSet::new());
        }
        let first = SEEN.with(|s| s.borrow_mut().insert(tag.to_string()));
        if first {
            eprintln!(
                "[yalda-gpui] agent-stream: ignoring unknown event kind {tag:?} \
                 (further ones suppressed; sid={} gen={} turn={} seq={})",
                &event.session_id[..event.session_id.len().min(8)],
                event.generation,
                event.turn,
                event.seq,
            );
        }
    }

    pub(crate) fn settle_agent_effect(claude: &mut AgentState, effect: AgentEventEffect) {
        match effect {
            AgentEventEffect::None => {}
            AgentEventEffect::TurnEnded { generation, turn } => {
                if claude.finalize_agent_turn_idem(generation, turn) {
                    claude.turn_phase = TurnPhase::Idle;
                }
            }
            AgentEventEffect::ReplayEnded => {
                // Replay end settles the replayed prefix exactly once through a
                // DEDICATED idempotency that does NOT touch the per-turn
                // `(generation, turn)` ledger: the server stamps the `ReplayEnd`
                // envelope `turn` with the current settled count, which is the
                // SAME index the next live turn's `completed_turn` carries, so
                // keying the turn ledger here would pre-occupy that live turn's
                // entry and make its `TurnEnded` a no-op → "thinking" forever.
                // Flip to Idle only if no live turn is in flight (a live submit
                // during/after replay keeps `Awaiting`).
                let settled = claude.finalize_replay_prefix();
                if settled && !claude.turn_phase.is_awaiting() {
                    claude.turn_phase = TurnPhase::Idle;
                }
            }
        }
    }

    /// Wipe the local claude buffer + tool-call state, drop the saved
    /// session id for the current cwd, and reattach. Equivalent to
    /// `/clear` in the Claude Code TUI: previous turns disappear from
    /// the view *and* the agent gets a fresh `session/new` so it isn't
    /// holding on to context from the cleared conversation. Use this
    /// when the model has gone off-track and you want a clean slate
    /// without restarting yalda.
    pub(crate) fn clear_agent_session(&mut self, cx: &mut Context<Self>) {
        // Forget every persisted slot BEFORE re-opening so the new spawn
        // hits session/new instead of session/load.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        // KILL (not free) the focused session — `/clear` discards the
        // conversation, so the old session must not linger in the store still
        // running. Capture the sid first, close it on the server, then drop it
        // from the store and unbind the tile.
        if let Some(id) = self.focused_bound_session() {
            if let Some(sid) = self.sessions.sid_of(id).map(|s| s.to_string()) {
                self.spawn_close_session(sid, cx);
            }
            self.sessions.close(id);
            if let Some(tile) = self.agent_tile_mut() {
                tile.bound = None;
                tile.picker = None;
            }
        }
        // Open a fresh session in place. We're already on an Agent tile (unbound
        // now), so `new_agent_session` rebinds this tile to a brand-new session.
        self.new_agent_session(None, cx);
        if let Some(c) = self.agent_mut() {
            c.status = Some("session cleared".into());
        }
        cx.notify();
    }

    /// Cycle the ACP permission mode (read-only → auto-edit → ask-each →
    /// yolo → read-only). Surfaces the new mode in the claude footer so
    /// the user sees the change without having to find it in the header.
    pub(crate) fn cycle_claude_permission_mode(&mut self, cx: &mut Context<Self>) {
        // Read what we need WITHOUT holding a long borrow of `self`. The
        // borrow checker forbids holding `self.agent_mut()` (borrows `self`)
        // while also touching `self.session_server` (also borrows `self`), so
        // we snapshot the current mode + whether a local channel exists first,
        // then drop the borrow before talking to the server.
        let snapshot = self
            .focused_bound_session()
            .and_then(|id| self.sessions.get(id))
            .map(|s| (s.state.permission_mode, s.state.channel.is_some()));
        let (current, has_channel) = match snapshot {
            Some(v) => v,
            None => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some("permission mode: no session".into());
                }
                cx.notify();
                return;
            }
        };
        let next = current.next();
        let sid = self.active_server_session_id();

        let server_result: Option<std::io::Result<()>> = match (&sid, self.session_server.as_ref())
        {
            (Some(sid), Some(server)) => Some(server.set_permission_mode(sid, next)),
            _ => None,
        };

        if let Some(result) = server_result {
            // Server-backed session: the authoritative mode lives in the
            // server. The change was pushed over the wire above (the
            // `session_server` borrow has now ended), so it's safe to
            // re-borrow `self` mutably to mirror the result into the
            // session-state field for the badge.
            match result {
                Ok(()) => {
                    if let Some(claude) = self.agent_mut() {
                        claude.permission_mode = next;
                        let msg = format!("permission mode → {}", next.short_label());
                        Self::append_system_notice(claude, &msg);
                        claude.status = Some(msg.into());
                    }
                }
                Err(e) => {
                    if let Some(claude) = self.agent_mut() {
                        claude.status = Some(format!("permission mode change failed: {e}").into());
                    }
                }
            }
        } else if has_channel {
            // Legacy direct-spawn fallback: the live channel is the authority.
            // Flip it AND keep the session-state mirror in sync.
            if let Some(claude) = self.agent_mut() {
                if let Some(ch) = &claude.channel {
                    ch.set_permission_mode(next);
                }
                claude.permission_mode = next;
                let msg = format!("permission mode → {}", next.short_label());
                Self::append_system_notice(claude, &msg);
                claude.status = Some(msg.into());
            }
        } else {
            // Neither a server session nor a local channel — nothing to drive.
            if let Some(claude) = self.agent_mut() {
                claude.status = Some("permission mode: no session".into());
            }
        }
        cx.notify();
    }

    /// Drop the active session's `AcpChannelClient` (kills the subprocess
    /// via `kill_on_drop`) but keep the `AgentSlot` and its chat history
    /// intact. The sidebar's `[d]` suffix surfaces the detached state. The
    /// slot's `resume_id` is preserved so the next reboot still tries to
    /// `session/load` the original id (per spec §15 stability rule); fresh
    /// `claude-new` slots without a `resume_id` will silently drop from
    /// persistence on the next save (per spec: "slots without a session id
    /// are not written").
    pub(crate) fn detach_active_agent_session(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        if claude.channel.is_none() && claude.attach_pending.is_none() {
            claude.status = Some("session is already detached".into());
            cx.notify();
            return;
        }
        // Drop runs `kill_on_drop` on the subprocess; cancel any in-flight
        // attach by dropping its receiver (the spawning thread's send will
        // fail silently when the connection drops).
        claude.channel = None;
        claude.attach_pending = None;
        claude.turn_phase = TurnPhase::Idle;
        Self::append_system_notice(claude, "session detached");
        claude.status = Some("session detached".into());
        self.save_agent_ring();
        cx.notify();
    }

    /// Spawn a fresh `AcpChannelClient` for the active session. Per spec §4
    /// re-attach does NOT resume the previous conversation — the agent
    /// subprocess was killed on detach, so the session is gone. Clear
    /// `resume_id` so persistence captures the new channel's id once it
    /// binds (rather than retrying the original-load id forever).
    pub(crate) fn attach_active_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        // A server-managed session is driven by the shared server pump — never
        // graft a SECOND direct channel + per-session pump onto it, or the two
        // pumps double-apply every event ("attached ×N" class). The server path
        // re-attaches via the reconnect flow, not this direct-spawn helper.
        let has_server = self.session_server.is_some();
        let server_managed = self
            .sessions
            .get(id)
            .map(|s| s.state.server_managed)
            .unwrap_or(false);
        if has_server && server_managed {
            if let Some(c) = self.agent_mut() {
                c.status = Some("session is server-managed — it reconnects automatically".into());
            }
            cx.notify();
            return;
        }
        if let Some(c) = self.agent_mut()
            && (c.channel.is_some() || c.attach_pending.is_some())
        {
            c.status = Some("session is already attached".into());
            cx.notify();
            return;
        }

        // Use the session's per-session cwd (spec-agent-cwd.md §3) so a session
        // that lives at /foo re-attaches at /foo, not at the launch directory.
        let slot_cwd = self.sessions.get(id).map(|s| s.cwd.clone());
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("YALDA_ACP_AGENT").unwrap_or_default();
        let _ = std::thread::Builder::new()
            .name("yalda-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    None,
                    yalda::acp_channel::YaldaFrontend::Gpui,
                ));
            });

        if let Some(session) = self.sessions.get_mut(id) {
            session.resume_id = None;
            session.state.attach_pending = Some(attach_rx);
            Self::append_system_notice(&mut session.state, "attaching new session…");
            session.state.status = Some("attaching new session…".into());
        }
        self.start_session_pump(id, cx);
        self.save_agent_ring();
        cx.notify();
    }

    /// Quit-and-relaunch yalda with the auto-open-claude flag set, so the
    /// new process boots straight into the claude screen and restores every
    /// session that was in the ring at quit time via
    /// `load_persisted_acp_sessions` plus per-slot `spawn_with_resume`.
    /// Designed for "I broke something in yalda and want to keep iterating
    /// with the same Claude context" — the user's chat history (on the agent
    /// side) is preserved through `session/load`.
    ///
    /// Spawns the child detached from the parent's stdio so the new GUI
    /// stays alive after `cx.quit()` tears down the current window. Args
    /// from the original invocation (e.g. the file path) are forwarded so
    /// the file the user was editing also reappears.
    pub(crate) fn reboot_into_claude(&mut self, cx: &mut Context<Self>) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.env("YALDA_OPEN_CLAUDE", "1");
        for arg in std::env::args().skip(1) {
            cmd.arg(arg);
        }
        // Detach stdio: the child inherits its own session so the dying
        // parent doesn't drag it down. On macOS/Linux a successful spawn
        // from a GUI process already survives parent exit because launchd
        // / init reparents it, but null-ing the streams is still cheap
        // insurance against any inherited pipe getting closed.
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(_) => cx.quit(),
            Err(e) => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some(format!("reboot failed: {e}").into());
                }
            }
        }
    }

    /// Send the user's pending draft (`extract_editable_inserts` —
    /// only the editable runs between/after frozen Claude turns) as the
    /// next ACP prompt, then lock the turn so that content can't be
    /// retroactively edited.
    /// Toggle the Tasklist sidebar visibility (§24).
    pub(crate) fn toggle_tasklist(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.tasklist_open = !c.tasklist_open;
        }
        cx.notify();
    }

    /// Toggle the Subagents sidebar visibility (§28).
    pub(crate) fn toggle_subagents(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.subagents_open = !c.subagents_open;
        }
        cx.notify();
    }

    /// Set the focused sub-agent by its stable tool-call key (§27). The
    /// main transcript swap is purely a render-time decision; this just
    /// flips the field. Keying by `ToolCallKey` (not a positional index)
    /// keeps focus pinned to the same sub-agent regardless of how the
    /// derived `subagents()` list is ordered (ADR-0006 quick win #1).
    pub(crate) fn focus_subagent(&mut self, key: ToolCallKey, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut()
            && c.tools.calls.contains_key(&key)
        {
            c.focused_subagent = Some(key);
        }
        cx.notify();
    }

    /// Return focus from a sub-agent transcript to the root agent (§27).
    pub(crate) fn unfocus_subagent(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.agent_mut() {
            c.focused_subagent = None;
        }
        cx.notify();
    }

    /// Flip the agent window's input mode (§5). Data movement is
    /// asymmetric per §6/§7:
    ///
    /// * Chatbox → Worksheet: take whatever's in the chatbox, append at
    ///   EOF of the transcript as new editable user lines (one transcript
    ///   line per chatbox line), drop the chatbox. If the chatbox was
    ///   empty, nothing is added.
    /// * Worksheet → Chatbox: don't touch the transcript at all; create
    ///   a fresh empty chatbox `Editor` and route input there. Any
    ///   editable lines already in the transcript stay pending and will
    ///   be swept by the next Submit.
    ///
    /// The chatbox's undo history is per-`Editor`; closing the chatbox
    /// drops that history (§7).
    pub(crate) fn toggle_agent_input_mode(&mut self, cx: &mut Context<Self>) {
        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        match &claude.input_surface {
            // Read the draft text out (last use of `cb`) BEFORE reassigning the
            // field, so the match's shared borrow ends and the write is clean.
            InputSurface::Chatbox(cb) => {
                let text = cb.text();
                claude.input_surface = InputSurface::Worksheet;
                if !text.is_empty() {
                    // Ensure the transcript ends with a `\n` so the
                    // appended draft starts on its own line, then drop
                    // the trailing newline of `text` so we don't leave a
                    // dangling blank below the cursor.
                    let needs_nl = !claude.editor.document().full_text().ends_with('\n');
                    let eof = claude.editor.document().rope().len_chars();
                    if needs_nl {
                        claude.editor.programmatic_insert(eof, "\n");
                    }
                    let to_append = text.strip_suffix('\n').unwrap_or(&text).to_string();
                    let eof2 = claude.editor.document().rope().len_chars();
                    claude.editor.programmatic_insert(eof2, &to_append);
                    let new_eof = claude.editor.document().rope().len_chars();
                    let (cl, cc) = doc_char_to_line_col(claude.editor.document(), new_eof);
                    claude.editor.cursor_mut().line = cl;
                    claude.editor.cursor_mut().col = cc;
                }
                claude.editor.clear_selection();
            }
            InputSurface::Worksheet => {
                claude.input_surface = InputSurface::Chatbox(Chatbox::new());
            }
        }
        cx.notify();
    }

    /// Submit the user's draft to the agent. Dispatches on `input_mode`:
    /// Worksheet sweep (§12) sweeps every editable line in document order,
    /// freezes them with `TurnId::User(k)`, and sends the non-blank lines.
    /// Chatbox submit (§18) takes the chatbox text, appends + freezes it
    /// at EOF of the transcript, then sends and clears the chatbox.
    pub(crate) fn submit_agent(&mut self, cx: &mut Context<Self>) {
        if self.is_candidate {
            self.set_agent_status(
                "read-only mirror — close the original window, then menu → claude → take over",
                cx,
            );
            return;
        }
        let is_chatbox = match self.agent_mut() {
            Some(c) => {
                // Re-enable auto-scroll when the user sends a message.
                c.follow_output.set(true);
                c.input_surface.is_chatbox()
            }
            None => return,
        };
        if is_chatbox {
            self.submit_chatbox(cx);
        } else {
            self.submit_worksheet(cx);
        }
    }

    /// Whether any agent slot (across all tabs/tiles) is mid-turn. Cheap
    /// traversal the pumps use to decide whether an idle animation tick is
    /// worth a re-render.
    pub(crate) fn any_agent_awaiting(&mut self) -> bool {
        self.sessions
            .iter()
            .any(|(_, s)| s.state.turn_phase.is_awaiting())
    }

    /// Whole-second fingerprint of the thinking-indicator clock across all
    /// awaiting agents, or `None` if nothing is awaiting. The indicator only
    /// displays `mm:ss`-granular elapsed/quiet timers, so the pump uses this to
    /// notify (and trigger the full transcript re-render) at most ~1Hz instead
    /// of every 120ms — 8x fewer O(transcript) rebuilds during a stall. We fold
    /// elapsed + quiet seconds into one value so a change in either repaints.
    pub(crate) fn awaiting_anim_fingerprint(&mut self) -> Option<u64> {
        let mut any = false;
        let mut fp: u64 = 0;
        for (_, s) in self.sessions.iter() {
            if s.state.turn_phase.is_awaiting() {
                any = true;
                let elapsed = s
                    .state
                    .turn_phase
                    .turn_started()
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                let quiet = s
                    .state
                    .turn_phase
                    .last_event_at()
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                fp = fp.wrapping_add(elapsed).wrapping_mul(1_000_003) ^ quiet.wrapping_add(1);
            }
        }
        any.then_some(fp)
    }

    /// Interrupt the in-flight agent turn (ACP `session/cancel`). Routes
    /// through the active path — session server when one owns the slot,
    /// otherwise the direct `AcpChannelClient`. The agent resolves the turn
    /// with `StopReason::Cancelled`, which bumps the turn counter and clears
    /// `awaiting_reply` on the next pump tick. No-op when nothing is in
    /// flight. Bound to `StopAgent` (Cmd-.) and the footer Stop button.
    pub(crate) fn stop_agent(&mut self, _: &StopAgent, _w: &mut Window, cx: &mut Context<Self>) {
        self.stop_agent_inner(cx);
    }

    /// `cx`-only stop, callable from the menu dispatch (which has no `Window`).
    pub(crate) fn stop_agent_inner(&mut self, cx: &mut Context<Self>) {
        // Read-only mirrors can't drive the session.
        if self.is_candidate {
            return;
        }
        // Only meaningful mid-turn.
        let awaiting = self
            .agent_mut()
            .map(|c| c.turn_phase.is_awaiting())
            .unwrap_or(false);
        if !awaiting {
            if let Some(claude) = self.agent_mut() {
                claude.status = Some("nothing to stop".into());
            }
            cx.notify();
            return;
        }

        // Second Stop while a cancel is already pending escalates to a hard
        // kill + resume — for a turn wedged on a hung upstream request the
        // cooperative `session/cancel` may never land.
        let escalate = self
            .agent_mut()
            .map(|c| c.turn_phase.stop_requested())
            .unwrap_or(false);
        if escalate {
            // Record the escalation on the phase before the hard kill so the
            // transition stays a total function over `TurnPhase` (the marker is
            // transient — `force_restart_agent` drops to Idle immediately after).
            if let Some(claude) = self.agent_mut() {
                claude.turn_phase.escalate();
            }
            self.force_restart_agent(cx);
            return;
        }

        // First Stop → graceful ACP session/cancel.
        let server_sid = self.active_server_session_id();
        let sent = if let Some(sid) = &server_sid {
            self.session_server
                .as_ref()
                .and_then(|s| s.cancel(sid).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            match claude.channel.as_ref() {
                Some(channel) => {
                    channel.cancel();
                    true
                }
                None => false,
            }
        } else {
            false
        };
        if let Some(claude) = self.agent_mut() {
            claude.turn_phase.request_stop(std::time::Instant::now());
            claude.status = Some(if sent {
                "stopping… (⌘. again to force-restart)".into()
            } else {
                "nothing to stop".into()
            });
        }
        cx.notify();
    }

    /// Hard recovery for a wedged turn: kill the agent subprocess and respawn
    /// it, resuming the same ACP session so prior context survives. The
    /// escalation behind a second Stop press. Routes to the session server
    /// (which owns the subprocess) in server mode, otherwise drops and
    /// re-attaches the direct channel.
    pub(crate) fn force_restart_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(sid) = self.active_server_session_id() {
            let ok = self
                .session_server
                .as_ref()
                .and_then(|s| s.restart_session(&sid).ok())
                .is_some();
            if let Some(claude) = self.agent_mut() {
                claude.turn_phase = TurnPhase::Idle;
                claude.status = Some(if ok {
                    "force-restarting agent (resuming session)…".into()
                } else {
                    "force-restart request failed".into()
                });
            }
            cx.notify();
            return;
        }

        // Direct mode: resume the current ACP session id on a fresh
        // subprocess; dropping the old channel kills the wedged one.
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let resume_id = self
            .sessions
            .get(id)
            .and_then(|s| s.state.channel.as_ref().and_then(|ch| ch.session_id()));
        let slot_cwd = self.sessions.get(id).map(|s| s.cwd.clone());
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("YALDA_ACP_AGENT").unwrap_or_default();
        let resume_for_worker = resume_id.clone();
        let _ = std::thread::Builder::new()
            .name("yalda-acp-force-restart".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    resume_for_worker,
                    yalda::acp_channel::YaldaFrontend::Gpui,
                ));
            });
        if let Some(session) = self.sessions.get_mut(id) {
            session.resume_id = resume_id;
            session.state.channel = None; // Drop → kills the wedged subprocess.
            session.state.attach_pending = Some(attach_rx);
            session.state.turn_phase = TurnPhase::Idle;
            Self::append_system_notice(
                &mut session.state,
                "force-restarting agent (resuming session)…",
            );
            session.state.status = Some("force-restarting agent (resuming session)…".into());
        }
        self.start_session_pump(id, cx);
        self.save_agent_ring();
        cx.notify();
    }

    /// Worksheet submit per §12. Sweep every editable line in document
    /// order, build the prompt body from those with non-whitespace content
    /// (`\n`-joined), freeze every collected line — including blank
    /// spacers — and tag each with `TurnId::User(k)` so the gutter shows
    /// `Uk`. If the body is empty, no-op with a footer hint.
    pub(crate) fn submit_worksheet(&mut self, cx: &mut Context<Self>) {
        // Capture server path info before borrowing agent_mut.
        let server_sid = self.active_server_session_id();

        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        // Check sendability: either direct channel or server session.
        if claude.channel.is_none() && server_sid.is_none() {
            claude.status = Some("no channel attached".into());
            cx.notify();
            return;
        }

        // Walk every line, classify editable vs frozen.
        let line_count = claude.editor.document().line_count();
        let mut collected: Vec<(usize, String)> = Vec::new();
        for l in 0..line_count {
            if claude.editor.is_frozen_line(l) {
                continue;
            }
            let line_text = claude.editor.document().line_text(l);
            let stripped = line_text.trim_end_matches('\n').to_string();
            collected.push((l, stripped));
        }

        // Build prompt body from lines with non-whitespace content.
        let prompt_body: String = collected
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if prompt_body.is_empty() {
            claude.status = Some("nothing to send".into());
            cx.notify();
            return;
        }

        // Send FIRST, then freeze the authored lines only on success — mirroring
        // submit_chatbox. The old order computed `last_seen_turns + 1` by hand
        // and froze the lines BEFORE the send check, which (a) bypassed the
        // reconciler chokepoint so the server/agent echo of this prompt
        // re-rendered it (the double-render bug) and (b) left a phantom frozen
        // turn in place when the send failed. `collected`/`prompt_body` are
        // owned, captured above, so they survive the agent re-borrow; the send
        // is fire-and-forget over a socket and never touches the editor, so the
        // captured line indices stay valid for the post-send freeze.
        let sent = if let Some(sid) = &server_sid {
            // Server path: prompt via session server (fire-and-forget; `Ok`
            // means written, not accepted — ownership is reasserted on resume).
            self.session_server
                .as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            // Direct path: send via AcpChannelClient.
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(claude) = self.agent_mut() {
                // Derive `k` + arm dedup through the shared reconciler core and
                // freeze the authored lines in place. Registering `prompt_body`
                // as a LocalSubmit is what suppresses the echo. `None` means the
                // M3 tripwire fired — leave the lines editable rather than
                // freeze an unattributed turn.
                claude.commit_worksheet_turn(&collected, &prompt_body);
                claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
            }
        } else if let Some(claude) = self.agent_mut() {
            // Send failed: keep the authored lines editable so the user can
            // retry, and surface it rather than dropping the prompt silently.
            claude.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        if let Some(claude) = self.agent_mut() {
            claude.editor.clear_selection();
        }
        cx.notify();
    }

    /// Chatbox submit per §18. Take the full chatbox text, append it at
    /// EOF of the transcript as new lines, immediately freeze them with
    /// `TurnId::User(k)`, send via the channel, clear the chatbox. Mode
    /// stays `Chatbox`.
    pub(crate) fn submit_chatbox(&mut self, cx: &mut Context<Self>) {
        // Capture server path info before borrowing agent_mut.
        let server_sid = self.active_server_session_id();

        let claude = match self.agent_mut() {
            Some(c) => c,
            None => return,
        };
        let text = match claude.input_surface.chatbox() {
            Some(cb) => cb.text(),
            None => return,
        };
        if text.trim().is_empty() {
            claude.status = Some("nothing to send".into());
            cx.notify();
            return;
        }
        if claude.channel.is_none() && server_sid.is_none() {
            claude.status = Some("no channel attached".into());
            cx.notify();
            return;
        }

        // Send FIRST, then freeze the optimistic echo only on success. Freezing
        // before the send could leave a "phantom" user turn in the transcript
        // that was never delivered (the old order did this). The send is to a
        // local socket/pipe, so doing it first costs nothing perceptible.
        let prompt_body = text.trim_end_matches('\n').to_string();
        let sent = if let Some(sid) = &server_sid {
            // NB: server `prompt` is fire-and-forget — `Ok` means the request
            // was written, NOT that the server accepted it (a non-owner
            // rejection is invisible here). Ownership is instead guaranteed on
            // resume by the retrying attach in `spawn_attach_sessions`, so by
            // the time the user can type, this connection owns the session.
            self.session_server
                .as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(claude) = self.agent_mut() {
                // Optimistic echo + begin the turn. `LocalSubmit` always
                // inserts and records the text so the stream echo that follows
                // (server `UserPrompt` or agent `UserMessage`, in any order
                // relative to streamed content) is suppressed. Never advances
                // the replay boundary on a live submit.
                claude.insert_user_turn(
                    &text,
                    yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
                    false,
                );
                claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
                // Reset the chatbox to empty; cursor stays inside.
                claude.input_surface = InputSurface::Chatbox(Chatbox::new());
            }
        } else if let Some(claude) = self.agent_mut() {
            // Send failed: leave the chatbox text intact so the user can retry,
            // and surface it instead of dropping the message into the void.
            claude.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        cx.notify();
    }

    /// Send the transcript editor's current selection as a prompt
    /// (Agent local menu `S`, spec-menu-scopes.md). Mirrors `submit_chatbox`'s
    /// send-first-then-echo order, but takes the text from the worksheet
    /// selection and leaves the input surface untouched.
    pub(crate) fn send_agent_selection(&mut self, cx: &mut Context<Self>) {
        if self.is_candidate {
            self.set_agent_status(
                "read-only mirror — close the original window, then menu → claude → take over",
                cx,
            );
            return;
        }
        let server_sid = self.active_server_session_id();

        let text = match self.agent_mut().and_then(|c| c.editor.selection_text()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some("no selection to send".into());
                }
                cx.notify();
                return;
            }
        };
        let no_channel = self
            .agent_mut()
            .map(|c| c.channel.is_none())
            .unwrap_or(true);
        if no_channel && server_sid.is_none() {
            if let Some(c) = self.agent_mut() {
                c.status = Some("no channel attached".into());
            }
            cx.notify();
            return;
        }

        let prompt_body = text.trim_end_matches('\n').to_string();
        let sent = if let Some(sid) = &server_sid {
            self.session_server
                .as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else if let Some(claude) = self.agent_mut() {
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(claude) = self.agent_mut() {
                claude.follow_output.set(true);
                // Optimistic echo, same as a chatbox submit — LocalSubmit
                // records the text so the stream echo is suppressed.
                claude.insert_user_turn(
                    &text,
                    yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
                    false,
                );
                claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
                claude.editor.clear_selection();
            }
        } else if let Some(claude) = self.agent_mut() {
            claude.status = Some("send failed — selection not sent".into());
        }
        cx.notify();
    }

    /// Key dispatch for the agent window. Recognises the agent-window-
    /// scoped shortcuts (`Ctrl-Enter` submit, `Ctrl-Alt-Enter` mode toggle,
    /// `Ctrl-V` leave, session-cycle `Ctrl-]`/`Ctrl-[`) before routing
    /// remaining keys to either the chatbox (in Chatbox mode) or the
    /// transcript editor (in Worksheet mode). See spec-agent-window.md §32.
    pub(crate) fn handle_claude_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Session switcher overlay intercepts all keys when open.
        if self.overlay_is_session() {
            self.handle_session_switcher_key(ev, _w, cx);
            return;
        }

        // Esc with a focused sub-agent: return to the parent transcript
        // (§27). Otherwise Esc falls through — the project rule is
        // "Esc never quits / never closes", so an unfocused-sub-agent
        // Esc keeps the existing per-mode behavior (toggle Normal etc.).
        if press.key == Key::Esc
            && self
                .agent_mut()
                .map(|c| c.focused_subagent.is_some())
                .unwrap_or(false)
        {
            self.unfocus_subagent(cx);
            return;
        }

        // Mode-toggle: Ctrl-Alt-Enter (§5). Checked before Ctrl-Enter so
        // an accidental Alt-press doesn't fire a submit instead.
        if press.modifiers.contains(KMods::CONTROL)
            && press.modifiers.contains(KMods::ALT)
            && press.key == Key::Enter
        {
            self.toggle_agent_input_mode(cx);
            return;
        }

        // Submit: Ctrl-Enter (§8). Bare Enter NEVER sends — it inserts a
        // literal newline (chatbox) or a new editable line (worksheet),
        // gated by the frozen-line invariants.
        if press.modifiers.contains(KMods::CONTROL) && press.key == Key::Enter {
            self.submit_agent(cx);
            return;
        }

        // Leave the agent window with Ctrl-V; the chatbox (if any) is
        // dropped without sending — its content is recoverable by toggling
        // back into Chatbox mode (which creates a fresh chatbox) and
        // re-typing, but we don't try to preserve unsent text across the
        // jump (spec §36).
        if press.modifiers.contains(KMods::CONTROL)
            && matches!(press.key, Key::Char('v') | Key::Char('V'))
        {
            if let Some(c) = self.agent_mut() {
                c.input_surface = InputSurface::Worksheet;
            }
            self.back_to_doc(cx);
            return;
        }

        // Chatbox-mode intercept: input routes to the chatbox editor when
        // we're in Chatbox mode; the transcript is read-only (§17). In
        // Worksheet mode the transcript IS the editing surface and the
        // chatbox doesn't exist.
        let in_chatbox = self
            .agent_mut()
            .map(|c| c.input_surface.is_chatbox())
            .unwrap_or(false);

        // Bare `m`/`'` in NORMAL mode starts a mark chord — agent tiles are
        // markable/jumpable like any other tile. Insert mode is untouched so
        // typing `m`/`'` into the chatbox/worksheet still works.
        let in_normal = if in_chatbox {
            self.agent_mut()
                .and_then(|c| {
                    c.input_surface
                        .chatbox_mut()
                        .map(|cb| cb.mode == EditMode::Normal)
                })
                .unwrap_or(false)
        } else {
            self.agent_mut()
                .map(|c| c.mode == EditMode::Normal)
                .unwrap_or(false)
        };
        if in_normal && self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
            return;
        }

        // Local leader: bare `.` in NORMAL mode opens the Agent local menu
        // (spec-menu-scopes.md Behavior 3 — `.` stays a text character in
        // the compose box / worksheet insert mode).
        if in_normal && press.modifiers.is_empty() && press.key == Key::Char('.') {
            self.open_local_menu_inner(cx);
            return;
        }

        if in_chatbox {
            let outcome = {
                let claude = match self.agent_mut() {
                    Some(c) => c,
                    None => return,
                };
                claude.status = None;
                let cb = claude.input_surface.chatbox_mut().unwrap();
                match cb.mode {
                    EditMode::Insert => {
                        Self::dispatch_insert_core(&mut cb.editor, &mut cb.mode, press);
                        NormalOutcome::Handled
                    }
                    EditMode::Normal => Self::dispatch_normal_core(
                        &mut cb.editor,
                        &mut cb.mode,
                        &mut claude.keybinds,
                        press,
                    ),
                }
            };
            match outcome {
                NormalOutcome::OpenMenu => self.open_menu_inner(cx),
                NormalOutcome::Quit => cx.quit(),
                _ => cx.notify(),
            }
            return;
        }

        let outcome = {
            let claude = match self.agent_mut() {
                Some(c) => c,
                None => return,
            };
            // Any non-shortcut keystroke clears the transient status.
            claude.status = None;

            match claude.mode {
                EditMode::Insert => {
                    Self::dispatch_insert_core(&mut claude.editor, &mut claude.mode, press);
                    NormalOutcome::Handled
                }
                EditMode::Normal => Self::dispatch_normal_core(
                    &mut claude.editor,
                    &mut claude.mode,
                    &mut claude.keybinds,
                    press,
                ),
            }
        };

        // Keep the cursor's doc line in view after every key. Compute
        // the cursor's index in the virtualised list (text lines are
        // interleaved with tool blocks anchored above them) and ask
        // the ListState to scroll just enough to reveal it.
        if let Some(c) = self.agent_mut() {
            let cursor_line = c.editor.cursor().line;
            let ranges = c.block_ranges.clone();
            let line_count = c.editor.document().line_count();
            // Hoist the metadata view out of the per-line loop — same fix as
            // the S1 gutter scan: `metadata::<TurnId>()` does a by-TypeId map
            // lookup and builds a fresh view per call, so calling it per line
            // made every Worksheet keystroke O(n) view constructions.
            let gutter_tags: Vec<Option<TurnId>> = {
                let turn_meta = c.editor.metadata::<TurnId>();
                (0..line_count)
                    .map(|i| {
                        c.editor
                            .anchor_for_line_opt(i)
                            .and_then(|a| turn_meta.get(a).copied())
                    })
                    .collect()
            };
            let th_before = count_turn_headers_before(&gutter_tags, cursor_line);
            let target = cursor_visible_child_index(c, cursor_line, &ranges, th_before);
            c.list_state.scroll_to_reveal_item(target);
        }

        match outcome {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                if let Some(c) = self.agent_mut() {
                    c.status = Some("yanked".into());
                }
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
        }
    }
}
