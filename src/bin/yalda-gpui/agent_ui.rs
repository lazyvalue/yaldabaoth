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
    /// to `Ctrl-K` in the Doc and Edit views. Replaces the focused tile with an
    /// Agent tile; the prior buffer stays in the pool (reachable via Cmd+O).
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
        // reachable via Cmd+O).
        let mut tile = AgentTile::new();
        // Inherit the active workspace's CWD (untitled.md "Agent inherits the
        // workspace CWD"); fall back to the process dir only when unset. Seeds
        // both the session-list query and the picker's "start new" cwd.
        let base_cwd = self.agent_base_cwd();

        if self.session_server.is_some() {
            // ── Session-server path: in-tile session picker ──────────
            // Open the tile straight into the picker (unbound). The picker
            // projects the FREE sessions for the cwd + "start new" from the
            // universal roster (universal-agent-list); refresh it in case it's
            // stale since the last seed.
            tile.picker = Some(SessionPicker::new());
            self.start_server_pump(cx);
            self.set_screen(App::Agent(tile));
            self.refresh_roster(cx);
            cx.notify();
            return;
        }

        // ── Direct-spawn path (legacy): bind one session to the tile ──
        self.set_screen(App::Agent(tile));
        let persisted = load_persisted_acp_sessions(&base_cwd);
        let chosen = persisted
            .iter()
            .find(|s| s.active)
            .or(persisted.first())
            .cloned();
        let id = match chosen {
            None => {
                let label = self.next_agent_label(cx);
                let state = self.create_agent_session(None, base_cwd.clone(), cx);
                self.show_local_session(
                    AgentSession {
                        state,
                        label,
                        cwd: base_cwd.clone(),
                        resume_id: None,
                    },
                    cx,
                )
            }
            Some(slot) => {
                let slot_cwd = slot.cwd.clone().unwrap_or_else(|| base_cwd.clone());
                let mut state =
                    self.create_agent_session(Some(slot.id.clone()), slot_cwd.clone(), cx);
                // Restore placement + seed the persisted compose draft (Model C).
                state.input_surface =
                    InputSurface::with_draft(slot.mode, slot.compose_draft.as_deref().unwrap_or(""));
                // Make focus/You-block consistent with the restored placement+draft
                // (replay's finish_replay re-settles too; this covers a no-history
                // restore): restored chatbox focuses its box; a restored worksheet
                // draft shows as a tail block; empty worksheet rests in nav.
                state.settle_input_focus();
                state.tasklist_open = slot.tasklist_open;
                state.subagents_open = slot.subagents_open;
                self.show_local_session(
                    AgentSession {
                        state,
                        label: slot.label,
                        cwd: slot_cwd,
                        resume_id: Some(slot.id),
                    },
                    cx,
                )
            }
        };
        self.start_session_pump(id, cx);

        if let Some(mut c) = self.agent_mut(cx) {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Refresh the universal roster (universal-agent-list) from the server's
    /// full session list. Targets NO tile — it replaces the whole roster cache
    /// and notifies if it changed. Called at boot/connect to seed and whenever a
    /// selector opens (in case the roster is stale); the Created/Closed/Renamed
    /// broadcasts keep it live between refreshes (so this is a seed, not a poll).
    pub(crate) fn refresh_roster(&self, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result: Result<Vec<_>, String> = cx
                .background_executor()
                .spawn(async move { handle.list_sessions().map_err(|e| e.to_string()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(sessions) = result
                    && this.agent_roster.replace_all(sessions)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Project the universal roster (universal-agent-list) into a tile
    /// selector's rows for `cwd`: cwd-matched sessions partitioned into FREE
    /// (selectable rows `1..=N`) and BOUND (in use by some tile, shown
    /// read-only). This REPLACES the old per-tile async `list_sessions`
    /// round-trip + `WindowId` routing (ADR-0020): the selector is now a pure,
    /// always-current view of the shared roster — derived at render/select time,
    /// never cached on the tile — so it can't go stale and there is no per-tile
    /// async result to misroute. Sorted by label (`entries_by_label`).
    pub(crate) fn picker_projection(
        &self,
        cwd: &std::path::Path,
    ) -> (Vec<PickerSession>, Vec<PickerSession>) {
        let open_sids = self.bound_sid_set();
        let cwd_key = cwd_match_key(cwd);
        let (mut free, mut bound) = (Vec::new(), Vec::new());
        for info in self.agent_roster.entries_by_label() {
            if cwd_match_key(&info.cwd) != cwd_key {
                continue;
            }
            let ps = PickerSession {
                sid: info.session_id.clone(),
                acp_id: info.acp_session_id.clone(),
                label: info.label.clone(),
                turns: info.turns,
                connected: info.connected,
                permission_mode: info.permission_mode,
            };
            if open_sids.contains(&info.session_id) {
                bound.push(ps);
            } else {
                free.push(ps);
            }
        }
        (free, bound)
    }

    /// A unique `claude-N` label for a session about to be created. `N` is the
    /// smallest positive integer whose `claude-N` isn't already taken by a
    /// session in the store OR by a session listed in the focused tile's picker
    /// (free + bound) — so the label is unique against everything we currently
    /// know about, local or server-side. The label is sent to the server at
    /// create time (`create_session`), so deduping here keeps the persisted
    /// names distinct too. Reuses a freed number (close claude-2, create →
    /// claude-2 again).
    pub(crate) fn next_agent_label(&self, cx: &GpuiApp) -> String {
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, s) in self.sessions.iter() {
            used.insert(s.read(cx).label.clone());
        }
        // Also avoid names of every session the server knows about (the
        // universal roster, universal-agent-list) — including ones not opened
        // here — so a fresh label is unique against all of them.
        for info in self.agent_roster.entries_by_label() {
            used.insert(info.label.clone());
        }
        (1..)
            .map(|n| format!("claude-{n}"))
            .find(|l| !used.contains(l))
            .expect("infinite range always yields a free label")
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
        if self.agent_tile().and_then(|t| t.picker.as_ref()).is_none() {
            return;
        }
        // Row count = "start new" + the FREE roster rows for the active
        // workspace's LIVE cwd (same source the picker renders/creates from).
        let cwd = self.agent_base_cwd();
        let n = (1 + self.picker_projection(&cwd).0.len()) as isize;
        if let Some(picker) = self.agent_tile_mut().and_then(|t| t.picker.as_mut()) {
            if n > 0 {
                picker.selected = (picker.selected as isize + delta).rem_euclid(n) as usize;
            }
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
        // The picker's rows are PROJECTED from the universal roster at this
        // cwd is read LIVE from the active workspace (`agent_base_cwd`), never a
        // value cached when the picker opened — so "Set CWD, then Start a new
        // session" creates the agent in the dir you just set. Rows are PROJECTED
        // from the universal roster for that same cwd (the list
        // `render_agent_picker` shows), so row indices resolve against it.
        let has_picker = self.agent_tile().and_then(|t| t.picker.as_ref()).is_some();
        let choice = if !has_picker {
            None
        } else {
            let cwd = self.agent_base_cwd();
            if row == 0 {
                Some(Choice::New(cwd))
            } else {
                let free = self.picker_projection(&cwd).0;
                free.get(row - 1).map(|s| Choice::Attach {
                    cwd,
                    sid: s.sid.clone(),
                    acp_id: s.acp_id.clone(),
                    label: s.label.clone(),
                    connected: s.connected,
                    permission_mode: s.permission_mode,
                })
            }
        };
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
        let label = self.next_agent_label(cx);
        let open_token = alloc_open_token();
        if self.agent_tile_mut().is_none() {
            return;
        }
        self.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(Some(
                    "connecting to session server…".into(),
                )),
                label: label.clone(),
                cwd: cwd.clone(),
                resume_id: None,
            },
            cx,
        );
        if let Some(tile) = self.agent_tile_mut() {
            tile.pending_open_token = Some(open_token);
        }
        self.spawn_create_agent_session(open_token, label, cwd, None, cx);
        if let Some(mut c) = self.agent_mut(cx) {
            c.editor.begin_insert();
        }
        self.save_agent_ring(cx);
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
        self.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(Some("reconnecting…".into())),
                label: label.clone(),
                cwd,
                resume_id: None,
            },
            cx,
        );
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
        if let Some(mut c) = self.agent_mut(cx) {
            c.editor.begin_insert();
        }
    }

    /// Key handler for the in-tile session picker (`AgentPickerView`):
    /// j/k or ↑/↓ to move, Enter to activate.
    pub(crate) fn handle_picker_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        // Leaders first: the picker is navigation, not text entry, so `<space>`/
        // `.`/`?` open the menus instead of being swallowed.
        if self.leader_intercept(&press, cx) {
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
                if let Some(ent) = self.session_entity(id) {
                    ent.update(cx, |session, scx| {
                        Self::append_system_notice(&mut session.state, &m);
                        session.state.status = Some(m.into());
                        scx.notify();
                    });
                }
            }
            OpenResolution::Created {
                sid,
                acp_id,
                permission_mode,
            } => match self.bind_session_sid(id, &sid) {
                BindOutcome::Bound => {
                    if let Some(ent) = self.session_entity(id) {
                        ent.update(cx, |session, scx| {
                            session.resume_id = acp_id;
                            session.state.permission_mode = permission_mode;
                            session.state.status =
                                Some("attaching to ACP agent via session server…".into());
                            // The synchronous `/clear` (or open) settled the local
                            // PLACEHOLDER, but this async server round-trip is where the
                            // session is finally bound — and the reported "can't see
                            // what I type after /clear" bug is that the worksheet ends
                            // here NOT typeable. Re-settle at the bind so the fresh
                            // worksheet rests focused+Insert (typeable + inline-active ⇒
                            // keystrokes route to the compose AND repaint the block); a
                            // restored draft/history session settles to its own correct
                            // state (settle is idempotent + stale-safe). `scx.notify()`
                            // repaints the (possibly freshly-created) TranscriptView.
                            session.state.settle_input_focus();
                            crate::clear_log(&format!(
                                "apply_open_agent_resolution Created: settled id={id:?} \
                                 focus_compose={} you_block_open={} awaiting={}",
                                session.state.focus == AgentFocus::Compose,
                                session.state.you_block_open,
                                session.state.turn_phase.is_awaiting(),
                            ));
                            scx.notify();
                        });
                    }
                    bound_sids.push(sid);
                }
                BindOutcome::Focused(owner) => self.focus_existing_session(owner, cx),
            },
            OpenResolution::Attached(attached) => {
                // Strict 1:1: a tile shows exactly one session. Bind the FIRST
                // attached session to this tile; ignore extras (the server may
                // list several per cwd, but each gets its own tile via the
                // picker, never a hidden ring).
                if let Some(first) = attached.into_iter().next() {
                    match self.bind_session_sid(id, &first.sid) {
                        BindOutcome::Bound => {
                            let AttachedSlot {
                                label,
                                sid,
                                acp_id,
                                status,
                                permission_mode,
                            } = first;
                            if let Some(ent) = self.session_entity(id) {
                                ent.update(cx, |session, scx| {
                                    session.label = label;
                                    session.resume_id = acp_id;
                                    session.state.permission_mode = permission_mode;
                                    session.state.status = Some(status.into());
                                    scx.notify();
                                });
                            }
                            bound_sids.push(sid);
                        }
                        BindOutcome::Focused(owner) => self.focus_existing_session(owner, cx),
                    }
                }
            }
        }
        self.save_agent_ring(cx);
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

    /// The stable `WindowId` of the agent tile currently BOUND to session
    /// `sid` (at most one, INV-2), scanning every tab. Deliberately
    /// focus-INDEPENDENT: the async close/reconcile paths use it to address a
    /// replacement selector's list back to the bound tile (INV-PR), so it must
    /// never depend on which tile holds focus. Directly unit-tested
    /// (`session_close_shows_selector_on_bound_tile_not_focused`) so a revert to
    /// focus-based routing fails CI rather than silently passing.
    pub(crate) fn agent_tile_id_bound_to(&self, sid: SessionId) -> Option<workspace::WindowId> {
        for tab in self.workspace.tabs.iter() {
            let mut found = None;
            tab.layout.for_each_leaf(&mut |w| {
                if let App::Agent(tile) = &w.content
                    && tile.bound == Some(sid)
                {
                    found = Some(w.id);
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Jump-panel selection of an agent session (jump-panel; ADR-0021). If the
    /// session is **bound** to a tile somewhere, focus that tile in place (no new
    /// tile — preserves the 1:1 invariant). If it is **free** (no tile binds it),
    /// open it in an **ephemeral virtual workspace** whose single tile binds it;
    /// that workspace is torn down the instant the user navigates away
    /// (`Workspace::set_active_tab`), returning the session to free. No-op if the
    /// session id is no longer in the store.
    pub(crate) fn jump_to_session(&mut self, sid: SessionId, cx: &mut Context<Self>) {
        if !self.sessions.contains(sid) {
            return;
        }
        if let Some(wid) = self.agent_tile_id_bound_to(sid) {
            self.jump_to_window(wid);
        } else {
            let mut tile = AgentTile::new();
            tile.bound = Some(sid);
            self.workspace.open_ephemeral_tab(App::Agent(tile));
        }
        cx.notify();
    }

    /// Jump-panel selection dispatcher (universal-agent-list). A row may target
    /// a session already opened here (`Local`) or one that lives only in the
    /// universal roster — running on the server but never opened in this GUI
    /// (`Roster`).
    pub(crate) fn jump_to_agent(&mut self, target: JumpTarget, cx: &mut Context<Self>) {
        match target {
            JumpTarget::Local(id) => self.jump_to_session(id, cx),
            JumpTarget::Roster(sid) => self.jump_to_roster_session(sid, cx),
        }
    }

    /// Open a roster session (one not yet in this GUI's store) by its server
    /// sid. If it's already opened here, delegate to `jump_to_session` (focus
    /// its tile, or an ephemeral tab if free). Otherwise open a fresh ephemeral
    /// virtual workspace (ADR-0021) and attach the session into it, reusing the
    /// picker's bind+attach path (`picker_attach_existing`).
    pub(crate) fn jump_to_roster_session(&mut self, sid: String, cx: &mut Context<Self>) {
        if let Some(id) = self.sessions.locate(&sid) {
            self.jump_to_session(id, cx);
            return;
        }
        let Some(info) = self.agent_roster.get(&sid).cloned() else {
            return;
        };
        if self.session_server.is_none() {
            return;
        }
        // Fresh ephemeral workspace holding one unbound agent tile, now focused;
        // the attach path binds the session into that focused tile.
        self.workspace
            .open_ephemeral_tab(App::Agent(AgentTile::new()));
        self.picker_attach_existing(
            info.cwd,
            info.session_id,
            info.acp_session_id,
            info.label,
            info.connected,
            info.permission_mode,
            cx,
        );
        cx.notify();
    }

    /// Resolve an agent tile by its stable `WindowId`, scanning every tab's
    /// layout (ids are unique workspace-wide). The canonical way for an async
    /// reducer to reach the tile that originated its work — `agent_tile_mut()`
    /// (the FOCUSED tile) must never be used from a `cx.spawn` continuation,
    /// because focus can move between spawn and resolution (INV-PR / ADR-0020).
    /// Returns `None` if the id is gone or no longer holds an `App::Agent`.
    // Retained as the canonical INV-PR-safe accessor for async reducers; the
    // selector's per-tile async list (its last caller) moved to a synchronous
    // roster projection (universal-agent-list), but new reducers should reach
    // tiles through this, never `agent_tile_mut()`.
    #[allow(dead_code)]
    fn agent_tile_by_id_mut(&mut self, id: workspace::WindowId) -> Option<&mut AgentTile> {
        for tab in self.workspace.tabs.iter_mut() {
            if let Some(w) = tab.layout.find_leaf_mut(id) {
                return match &mut w.content {
                    App::Agent(tile) => Some(tile),
                    _ => None,
                };
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
                self.transcript_views.remove(&id);
                self.sessions.close(id);
                BindOutcome::Focused(owner)
            }
        }
    }

    /// Resolve an AlreadyBound conflict: the sid we tried to bind is already
    /// owned by `owner`, which some tile may already display. Strict 1:1 (a
    /// session is shown by at most ONE tile) means we must NOT bind a second
    /// tile to it — the old code did exactly that, which let the same session
    /// appear in two workspaces. Instead: if another tile already binds `owner`,
    /// return THIS tile to a fresh selector (no duplicate) and navigate to the
    /// owner. Only when no other tile binds `owner` (or it's this very tile) do
    /// we bind here.
    pub(crate) fn focus_existing_session(&mut self, owner: SessionId, cx: &mut Context<Self>) {
        let current = self.workspace.focused_window_id();
        match self.agent_tile_id_bound_to(owner) {
            Some(owner_win) if Some(owner_win) != current => {
                if let Some(tile) = self.agent_tile_mut() {
                    tile.bound = None;
                    tile.pending_open_token = None;
                    tile.picker = Some(SessionPicker::new());
                }
                // The selector projects from the roster; the owner is now
                // filtered out as bound. Refresh the roster, then reveal the
                // owner's tile (possibly in another workspace).
                self.refresh_roster(cx);
                self.transient_status =
                    Some("session already open in another workspace — switched to it".into());
                self.focus_window_for_restore(owner_win);
            }
            _ => {
                if let Some(tile) = self.agent_tile_mut() {
                    tile.bound = Some(owner);
                    tile.picker = None;
                    tile.pending_open_token = None;
                }
            }
        }
    }

    /// Attach (strict 1:1) to sessions whose slots were just bound by
    /// `apply_open_agent_resolution`, off the paint thread. Attaching here —
    /// AFTER the bind — is what closes the replay-drop race: the pump can route
    /// every replayed notification because its slot already exists. A failed
    /// attach is reconciled back into the slot status so a dead session is
    /// visible instead of silently broken.
    pub(crate) fn spawn_attach_sessions(&self, sids: Vec<String>, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let results: Vec<(String, Result<(), String>)> = cx
                .background_executor()
                .spawn(async move {
                    sids.into_iter()
                        .map(|sid| {
                            let r = attach_session(&handle, &sid);
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
                    // Per-session outcome: the status string to surface (if any).
                    let status: Option<SharedString> = match r {
                        // Attached: leave the optimistic status to be overwritten
                        // by the first real event.
                        Ok(()) => None,
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
                        && let Some(sid_id) = this.sessions.locate(&sid)
                    {
                        this.with_session(sid_id, cx, |st| {
                            st.status = Some(s);
                        });
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
                    this.save_agent_ring(cx);
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
    /// The active workspace's working directory. `None` only when there is no
    /// active tab at all (a transient pre-first-tab state); a workspace that
    /// exists always carries a cwd (the typed [`WorkspaceCwd`] makes "no cwd"
    /// unrepresentable — ADR-0023). The path is whatever `Set CWD` resolved at
    /// write time; a since-deleted dir surfaces as a spawn error
    /// (spec-agent-cwd.md §9), not here.
    pub(crate) fn active_workspace_cwd(&self) -> Option<PathBuf> {
        self.workspace.active_tab().map(|t| t.cwd().to_path_buf())
    }

    /// The CWD a new agent session inherits when the caller gives no explicit
    /// one: the active workspace's cwd. The single resolution every
    /// agent-creation entry point (open / new / bootstrap) shares, so opening an
    /// agent in workspace 2 lands in workspace 2's dir, not the app's launch
    /// dir. Total — the workspace always has a cwd; the `process_cwd` fallback
    /// only covers the degenerate no-tab state.
    pub(crate) fn agent_base_cwd(&self) -> PathBuf {
        self.active_workspace_cwd().unwrap_or_else(process_cwd)
    }

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
        let label = self.next_agent_label(cx);
        let slot_cwd = cwd.unwrap_or_else(|| self.agent_base_cwd());
        self.release_focused_session_for_rebind();

        if self.session_server.is_some() {
            // Server path: bind a "connecting…" placeholder and create the
            // session off-thread; the sid binds when the round-trip returns.
            let open_token = alloc_open_token();
            self.show_local_session(
                AgentSession {
                    state: AgentState::new_server_managed(Some(
                        "connecting to session server…".into(),
                    )),
                    label: label.clone(),
                    cwd: slot_cwd.clone(),
                    resume_id: None,
                },
                cx,
            );
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(open_token, label, slot_cwd, None, cx);
        } else {
            // Direct-spawn path.
            let state = self.create_agent_session(None, slot_cwd.clone(), cx);
            let id = self.show_local_session(
                AgentSession {
                    state,
                    label,
                    cwd: slot_cwd,
                    resume_id: None,
                },
                cx,
            );
            self.start_session_pump(id, cx);
        }
        if let Some(mut c) = self.agent_mut(cx) {
            c.editor.begin_insert();
        }
        self.save_agent_ring(cx);
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
        let slot_cwd = cwd.unwrap_or_else(|| self.agent_base_cwd());
        let label = self.next_agent_label(cx);

        if self.session_server.is_some() {
            // Server path: placeholder + create-only round-trip (NO resolve /
            // reattach — that is the whole point of "fresh").
            let open_token = alloc_open_token();
            self.show_local_session(
                AgentSession {
                    state: AgentState::new_server_managed(Some(
                        "connecting to session server…".into(),
                    )),
                    label: label.clone(),
                    cwd: slot_cwd.clone(),
                    resume_id: None,
                },
                cx,
            );
            self.start_server_pump(cx);
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            if let Some(mut c) = self.agent_mut(cx) {
                c.editor.begin_insert();
            }
            cx.notify();
            self.spawn_create_agent_session(open_token, label, slot_cwd, None, cx);
        } else {
            // Direct-spawn path: a fresh session has no resume_id.
            let state = self.create_agent_session(None, slot_cwd.clone(), cx);
            let id = self.show_local_session(
                AgentSession {
                    state,
                    label,
                    cwd: slot_cwd,
                    resume_id: None,
                },
                cx,
            );
            self.start_session_pump(id, cx);
            if let Some(mut c) = self.agent_mut(cx) {
                c.editor.begin_insert();
            }
            self.save_agent_ring(cx);
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
        // When `Some`, force this permission mode on the freshly-created session
        // instead of the server's default. Used by `/clear` to carry the cleared
        // session's mode across the close+create. Applied in the same background
        // round-trip (the new sid is known there), so the GUI badge reflects it
        // when the slot binds.
        desired_mode: Option<yalda::acp_channel::PermissionMode>,
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
                        Ok(info) => {
                            let mut permission_mode = info.permission_mode;
                            // Preserve a non-default mode across `/clear`: the
                            // create returns the server default, so push the
                            // desired mode now (we have the sid) and reflect it
                            // in the resolution so the badge is correct on bind.
                            if let Some(want) = desired_mode
                                && want != permission_mode
                            {
                                match handle.set_permission_mode(&info.session_id, want) {
                                    Ok(()) => permission_mode = want,
                                    Err(e) => eprintln!(
                                        "[yalda-gpui] clear: preserve permission mode failed: {e}"
                                    ),
                                }
                            }
                            OpenResolution::Created {
                                sid: info.session_id,
                                acp_id: info.acp_session_id,
                                permission_mode,
                            }
                        }
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

    /// Spawn a brand-new agent session that is bound to NO tile and NO
    /// workspace — a *free* session (spec-agent-session-ownership.md). Used by
    /// the global (`?`) menu's "new agent session" command.
    ///
    /// Unlike `new_agent_session` / `bootstrap_fresh_agent_session` (which place
    /// a tile and bind the new sid to it), this only issues the server
    /// `create_session` round-trip. The resulting session lands in the universal
    /// roster via the `SessionCreated` broadcast (and an explicit
    /// `refresh_roster` to make it appear immediately), so it shows up in the
    /// jump panel as an unbound, bindable row — never auto-bound here. A user can
    /// later bind it by selecting it (jump panel → `jump_to_roster_session`, or a
    /// tile selector). It is server-only: with no session server there is no
    /// roster to host a free session, so this no-ops with a status note.
    pub(crate) fn spawn_free_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            self.transient_status =
                Some("no session server — free agent sessions need one".into());
            cx.notify();
            return;
        };
        // Reuse the same label allocator and cwd resolution as the tile-bound
        // create paths so a free session is named/rooted identically.
        let label = self.next_agent_label(cx);
        let cwd = self.agent_base_cwd();
        self.transient_status = Some(format!("creating free agent session {label}…").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Result<String, String> = cx
                .background_executor()
                .spawn(async move {
                    handle
                        .create_session(cwd, label, None)
                        .map(|info| info.label)
                        .map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(label) => {
                        this.transient_status =
                            Some(format!("free agent session {label} created").into());
                        // Pull it into the roster now so the jump panel lists it
                        // without waiting on the (also-arriving) broadcast.
                        this.refresh_roster(cx);
                    }
                    Err(e) => {
                        this.transient_status =
                            Some(format!("free agent session create failed: {e}").into());
                    }
                }
                cx.notify();
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
        if let Some(ent) = self.session_entity(id) {
            ent.update(cx, |session, scx| {
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
                scx.notify();
            });
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
            if let Some(ent) = self.session_entity(id) {
                ent.update(cx, |session, scx| {
                    session.state.attach_pending = None;
                    session.state.channel = None;
                    let msg = format!(
                        "cwd → {}, connecting to fresh session…",
                        shorten_cwd_for_display(&new_cwd),
                    );
                    Self::append_system_notice(&mut session.state, &msg);
                    session.state.status = Some(msg.into());
                    scx.notify();
                });
            }
            // Stamp the focused tile (which shows this session) with the token.
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(
                open_token,
                "respawned".to_string(),
                new_cwd.clone(),
                None,
                cx,
            );
        } else {
            // Direct-spawn path: graft a throwaway AgentState's attach handle
            // into the existing session, then (re)start its pump.
            let fresh = self.create_agent_session(None, new_cwd.clone(), cx);
            if let Some(ent) = self.session_entity(id) {
                ent.update(cx, |session, scx| {
                    session.state.attach_pending = fresh.attach_pending;
                    let msg =
                        format!("cwd → {}, fresh session", shorten_cwd_for_display(&new_cwd));
                    Self::append_system_notice(&mut session.state, &msg);
                    session.state.status = Some(msg.into());
                    scx.notify();
                });
            }
            self.start_session_pump(id, cx);
        }

        self.save_agent_ring(cx);
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
                self.transcript_views.remove(&id);
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
    /// state with no usable keys. Used by close / reconcile.
    pub(crate) fn show_selector_on_focused_tile(&mut self, cx: &mut Context<Self>) {
        if let Some(tile) = self.agent_tile_mut() {
            tile.bound = None;
            tile.pending_open_token = None;
            tile.picker = Some(SessionPicker::new());
        }
        // The selector projects from the roster; refresh in case it's stale.
        self.refresh_roster(cx);
    }

    /// Close the focused session. The tile stays an Agent tile, transitioning
    /// to a LIVE unbound selector — an agent tile never falls back to a buffer.
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
        // Drop the session from the store (its channel/pump cancel on drop) and
        // land the tile in a live selector.
        self.transcript_views.remove(&id);
        self.sessions.close(id);
        self.show_selector_on_focused_tile(cx);
        // Wipe the cwd entry so reboot doesn't resurrect the closed session.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        self.save_agent_ring(cx);
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
    pub(crate) fn save_agent_ring(&self, cx: &GpuiApp) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let mut snaps: Vec<SessionSnapshot> = Vec::new();
        for tab in self.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |window| {
                if let App::Agent(tile) = &window.content
                    && let Some(id) = tile.bound
                    && let Some(ent) = self.sessions.get(id)
                {
                    let session = ent.read(cx);
                    // resume_id wins over the channel id (keep retrying the
                    // original id even when load fell back).
                    let resolved_id = session
                        .resume_id
                        .clone()
                        .or_else(|| session.state.channel.as_ref().and_then(|c| c.session_id()));
                    if let Some(rid) = resolved_id {
                        let draft = session.state.input_surface.compose().text();
                        snaps.push(SessionSnapshot {
                            id: rid,
                            label: session.label.clone(),
                            active: snaps.is_empty(),
                            mode: session.state.input_surface.mode(),
                            tasklist_open: session.state.tasklist_open,
                            subagents_open: session.state.subagents_open,
                            cwd: session.cwd.clone(),
                            compose_draft: (!draft.trim().is_empty()).then_some(draft),
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

            status: Some("attaching to ACP agent…".into()),
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            pending_reveal_cursor: false,
            user_turn_jump_mode: false,
            user_turn_jump_ord: 0,
            pending_jump_ord: None,
            pending_jump_end: false,
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::new(InputModeKind::Worksheet),
            focus: AgentFocus::Transcript,
            you_block_open: false,
            you_block_anchor: None,
            parked_you_blocks: Vec::new(),
            current_plan: None,
            agent_mode: None,
            agent_model: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            panel_col: PanelColumn::Tasklist,
            panel_sel: 0,
            panel_return_focus: AgentFocus::Compose,
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
        // The follow-output scroll handler is wired by the owning
        // `TranscriptView` (ticket 021) — the `ListState` lives in
        // `TranscriptScroll`, not on `AgentState`.
        let mut state = state;
        // A FRESH worksheet session has an empty transcript with nothing to
        // navigate — open a tail You-block so there's a visible input on first
        // open (bug: "I don't see anything" on a new session). settle handles it.
        state.settle_input_focus();
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
            let mut last_anim_fp: Option<u64> = None;
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
                    let _ = this.update(cx, |this, cx| {
                        wake_rx = this
                            .read_session(id, cx, |state| {
                                state.channel.as_ref().and_then(|ch| ch.take_wake_receiver())
                            })
                            .flatten();
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
                        // Only repaint when the whole-second indicator clock
                        // actually advanced — an unchanged fingerprint means the
                        // visible "Thinking… mm:ss" label is identical, so a
                        // notify() here would waste a full transcript rebuild.
                        // Mirrors the server pump's ~1Hz throttle above (this
                        // legacy direct-spawn path previously notified every 120ms).
                        let fp = this.awaiting_anim_fingerprint(cx);
                        if fp.is_some() && fp != last_anim_fp {
                            last_anim_fp = fp;
                            // The clock lives INSIDE the cached TranscriptView; a
                            // root notify cannot bust a cached child (facts 3/6)
                            // and no session seq moves during a stall, so tick each
                            // awaiting session's transcript directly (timer
                            // context, timing-correct, fact 4).
                            this.tick_awaiting_transcript_views(cx);
                        } else if fp.is_none() {
                            last_anim_fp = None;
                        }
                    });
                }
                let elapsed = cycle_start.elapsed();
                if elapsed < min_cycle {
                    cx.background_executor().timer(min_cycle - elapsed).await;
                }
            }
        });
        if let Some(ent) = self.session_entity(id) {
            ent.update(cx, |session, _scx| {
                session.state._pump = Some(pump);
            });
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
            if let Some(ent) = self.session_entity(id) {
                ent.update(cx, |session, scx| {
                    session.state.reset_for_replay();
                    Self::append_system_notice(&mut session.state, "reconnecting…");
                    session.state.status = Some("reconnecting…".into());
                    scx.notify();
                });
            }
            sids.push(sid);
        }

        // Re-attach off the paint thread (strict 1:1: attach is unconditional —
        // there is no owner to reclaim or contend with). The PREVIOUS
        // connection's server-side teardown races this fresh one, but a bare
        // attach simply succeeds: the server replays the full event_log on
        // attach and tears down any stale forwarder when this one registers.
        // `spawn_attach_sessions` reconciles per-slot status. (Doing blocking
        // attach round-trips inline here, as before, also froze rendering.)
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
                            let fp = this.awaiting_anim_fingerprint(cx);
                            if fp.is_some() && fp != last_anim_fp {
                                last_anim_fp = fp;
                                // The clock lives INSIDE the cached TranscriptView;
                                // a root notify cannot bust a cached child (facts
                                // 3/6) and no session seq moves during a stall, so
                                // tick each awaiting session's transcript directly
                                // (timer context, timing-correct, fact 4).
                                this.tick_awaiting_transcript_views(cx);
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
        cx: &mut Context<Self>,
        mut f: impl FnMut(&mut AgentSession),
    ) -> bool {
        let Some(id) = self.sessions.locate(sid) else {
            return false;
        };
        let Some(ent) = self.session_entity(id) else {
            return false;
        };
        // Mutation-site notify on the session entity (timing-correct, fact 4);
        // the root is also notified by the reducer's own `cx.notify()` today —
        // the per-session notify is load-bearing only after 021's observation.
        ent.update(cx, |session, scx| {
            f(session);
            scx.notify();
        });
        true
    }

    /// Reconcile a server-side close: drop the session for `sid` from the store
    /// and land whichever tile showed it in a LIVE free-session selector. The
    /// tile stays an Agent tile — an agent tile never falls back to a buffer.
    /// Returns whether anything changed.
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
        // Find the tile that showed this session (at most one, INV-2). Skip a
        // tile mid-respawn (pending_open_token in flight). The replacement
        // selector projects from the roster (no per-tile async list to
        // misroute), so there's no longer a tile id to capture here.
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
                        tile.picker = Some(SessionPicker::new());
                    }
                }
            });
        }
        if tile_was_respawning {
            // Leave the respawning session alone; its new sid will rebind.
            return false;
        }
        self.transcript_views.remove(&id);
        self.sessions.close(id);
        // The now-unbound tile's selector projects from the roster (the closed
        // session was already removed from it by the SessionClosed handler / is
        // gone after this). Refresh to be safe on non-broadcast close paths.
        let _ = tile_found;
        self.refresh_roster(cx);
        true
    }

    /// Reconcile a server-side rename: update the label on the session for
    /// `sid`. Returns whether anything changed.
    pub(crate) fn reconcile_session_renamed(
        &mut self,
        sid: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.sessions.locate(sid) else {
            return false;
        };
        let Some(ent) = self.session_entity(id) else {
            return false;
        };
        ent.update(cx, |session, scx| {
            if session.label != label {
                session.label = label.to_string();
                scx.notify();
                true
            } else {
                false
            }
        })
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
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
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
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
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
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
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
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
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
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
                        let label = acp_session_id.as_deref().unwrap_or("connected");
                        let msg = format!("attached: {label}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionDetached { session_id, reason } => {
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
                        let msg = format!("detached: {reason}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                    });
                    warn_unrouted(routed, &session_id);
                }
                ServerNotification::SessionCreated { session } => {
                    // A session was created by some connection (this GUI, a CLI,
                    // cron, another window). Fold it into the universal roster
                    // (universal-agent-list) so the jump panel + every selector
                    // surface it immediately — even though no tile here binds it.
                    self.agent_roster.upsert(session);
                }
                ServerNotification::SessionClosed { session_id } => {
                    // A session closed somewhere (this GUI, another tile, or
                    // another GUI instance). Drop it from the roster, then from
                    // the store + land its tile (if any) in a live selector.
                    self.agent_roster.remove(&session_id);
                    self.reconcile_session_closed(&session_id, cx);
                }
                ServerNotification::SessionRenamed { session_id, label } => {
                    self.agent_roster.rename(&session_id, &label);
                    self.reconcile_session_renamed(&session_id, &label, cx);
                }
                ServerNotification::PromptRejected {
                    session_id,
                    reason,
                    text,
                } => {
                    // The server refused the prompt (e.g. a send failure or a
                    // missing session). The optimistic echo is already frozen in
                    // the transcript, so without this notice the message would
                    // LOOK sent while the agent never received it. Say so in
                    // the transcript + status line, and put the text back in
                    // the chatbox (only if the user hasn't typed something
                    // new) so a resubmit is one keypress.
                    let routed = self.with_server_session_slot(&session_id, cx, |slot| {
                        let msg = format!("✗ message NOT delivered: {reason}");
                        Self::append_system_notice(&mut slot.state, &msg);
                        slot.state.status = Some(msg.into());
                        if let Some(cb) = slot.state.input_surface.chatbox_mut()
                            && cb.text().trim().is_empty()
                        {
                            let mut fresh = Compose::new();
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
        // Ticket 021: the pump-side stale-count pre-pin is gone. The
        // `ListState` + `list_item_count` now live in the owning
        // `TranscriptView`, and its render-time `reveal_tail_if_following`
        // (with the FRESH post-reconcile count) is the single authoritative
        // re-reveal — it runs for every live transcript view, focused or not,
        // so unfocused tiles stay pinned without a pump-side poke.
        let _ = &scrolled_sessions;
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

        // The session lives in its own entity; drain + apply inside its
        // `update` so the mutation-site notify on the session is timing-correct
        // (fact 4). The closure returns `None` to signal the pump task should
        // exit (session gone / disconnected), or `Some((has_events,
        // more_pending, attached_with_id))` to continue with the post-borrow
        // persistence below.
        let Some(ent) = self.session_entity(id) else {
            return false; // session gone: pump task should exit
        };
        let Some((has_events, more_pending, attached_with_id)) =
            ent.update(cx, |session, scx| {
            let claude = &mut session.state;

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
                scx.notify();
                return None;
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
                // Spec §19 auto-scroll. Ticket 021: the pump-side stale-count
                // pre-pin is gone — the `ListState` lives in the owning
                // `TranscriptView`, whose render-time `reveal_tail_if_following`
                // (with the fresh post-reconcile count) is the authoritative
                // re-reveal. The session notify below buses the transcript view
                // to re-render this same effect-flush, so the reveal lands on
                // the very frame this chunk scheduled (no stale tail).
            }

            // Mutation-site notify on the session entity (load-bearing after
            // 021; redundant with the root notify below today).
            if has_events {
                scx.notify();
            }
            Some((has_events, more_pending, attached_with_id))
        }) else {
            return false; // disconnected: pump task should exit
        };

        // Post-borrow: persist the whole ring snapshot so the just-attached
        // slot's id (or its preserved resume_id, if load failed) lands on
        // disk. Writing the snapshot (not just the one slot) is what makes
        // a stale pump from a removed slot safe — it contributes nothing
        // if its slot isn't in the ring anymore.
        if attached_with_id {
            self.save_agent_ring(cx);
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
        // Floor the splice to the top of any user worksheet draft so a notice
        // arriving mid-compose lands above it, not below (interspersed-content
        // bug — same invariant as LLM chunks and tool anchors).
        let floor = agent_tail_floor_char(&claude.editor);
        claude
            .editor
            .append_llm_chunk_floored(TurnId::System, &notice_line, floor);
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
                    let floor = agent_tail_floor_char(&claude.editor);
                    claude.editor.append_llm_chunk_floored(
                        TurnId::Llm(current_turn),
                        text.as_str(),
                        floor,
                    );
                }
                ReplyEvent::ToolCallStarted(mut tc) => {
                    cap_tool_call_payloads(&mut tc);
                    let floor = agent_tail_floor_char(&claude.editor);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor, floor);
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
                        let floor = agent_tail_floor_char(&claude.editor);
                        let anchor = anchor_for_new_tool_call(&mut claude.editor, floor);
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
                ReplyEvent::ModelChanged(model_id) => {
                    claude.agent_model = Some(model_id);
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
                // Both roles share the LLM-turn surface; floor the splice to
                // the top of any user worksheet draft (see
                // `agent_tail_floor_char`) so streamed content lands above it.
                let floor = agent_tail_floor_char(&claude.editor);
                match role {
                    ChunkRole::Message => {
                        claude.editor.append_llm_chunk_floored(
                            TurnId::Llm(turn),
                            text.as_str(),
                            floor,
                        );
                    }
                    ChunkRole::Thought => {
                        // Thought text un-parks the parked `AgentThoughtChunk`
                        // path. Until a dedicated thought style ships it shares
                        // the LLM-turn surface (tagged by the same turn) so the
                        // reasoning is still attributed to the right turn and
                        // never silently dropped.
                        claude.editor.append_llm_chunk_floored(
                            TurnId::Llm(turn),
                            text.as_str(),
                            floor,
                        );
                    }
                }
                AgentEventEffect::None
            }
            AgentEventKind::ToolCallStarted(tc) => {
                let mut tc = tc.clone();
                cap_tool_call_payloads(&mut tc);
                let floor = agent_tail_floor_char(&claude.editor);
                let anchor = anchor_for_new_tool_call(&mut claude.editor, floor);
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
                    let floor = agent_tail_floor_char(&claude.editor);
                    let anchor = anchor_for_new_tool_call(&mut claude.editor, floor);
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
            AgentEventKind::ModelChanged(model_id) => {
                claude.agent_model = Some(model_id.clone());
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

    /// Discard the focused session's conversation and start a fresh one **in
    /// place, preserving the session's identity** — name (label), working
    /// directory, and permission mode all carry across; only the transcript and
    /// the agent's context are reset. Equivalent to `/clear` in the Claude Code
    /// TUI. Use this when the model has gone off-track and you want a clean
    /// slate without restarting yalda or losing the session's setup.
    ///
    /// Mechanically this is still a close+create: the conversation lives in the
    /// agent subprocess, so there is no in-place reset — the old session is
    /// killed and a new one is spawned. The internal `SessionId` / server sid
    /// therefore change; everything the *user* sees (label, cwd, mode, the tile
    /// binding) is preserved. A no-op with a status hint if no session is bound.
    pub(crate) fn clear_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            if let Some(mut c) = self.agent_mut(cx) {
                c.status = Some("clear: no session".into());
            }
            cx.notify();
            return;
        };

        // Snapshot the identity to carry across the reset BEFORE closing.
        let Some((label, slot_cwd)) = self
            .sessions
            .get(id)
            .map(|ent| {
                let s = ent.read(cx);
                (s.label.clone(), s.cwd.clone())
            })
        else {
            return;
        };
        let desired_mode = self.read_session(id, cx, |s| s.permission_mode);

        // Forget persisted slots BEFORE re-opening so the new spawn hits
        // session/new, not session/load (a load would resume the conversation
        // we are trying to discard).
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }

        // KILL the old session (clear discards the conversation): close it on
        // the server, drop it from the store, unbind the tile.
        if let Some(sid) = self.sessions.sid_of(id).map(|s| s.to_string()) {
            self.spawn_close_session(sid, cx);
        }
        self.transcript_views.remove(&id);
        self.sessions.close(id);
        if let Some(tile) = self.agent_tile_mut() {
            tile.bound = None;
            tile.picker = None;
        }
        crate::clear_log(&format!(
            "clear_agent_session: closed old_id={id:?} server_is_some={}",
            self.session_server.is_some()
        ));

        // Re-create in place on the now-unbound focused tile, reusing the
        // snapshotted label + cwd and forcing the preserved permission mode.
        if self.session_server.is_some() || crate::force_server_clear_branch() {
            let open_token = alloc_open_token();
            // `/clear` discards the conversation → a FRESH empty worksheet. Settle so
            // it opens a typeable tail You-block immediately; without this the
            // worksheet rests in nav and post-clear keystrokes vanish into transcript
            // navigation (the "/clear then can't type" bug). (The connecting
            // placeholder has no history, so finish_replay won't re-settle it.)
            let mut state =
                AgentState::new_server_managed(Some("connecting to session server…".into()));
            state.settle_input_focus();
            self.show_local_session(
                AgentSession {
                    state,
                    label: label.clone(),
                    cwd: slot_cwd.clone(),
                    resume_id: None,
                },
                cx,
            );
            if let Some(tile) = self.agent_tile_mut() {
                tile.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(open_token, label, slot_cwd, desired_mode, cx);
        } else {
            // Direct-spawn fallback (legacy YALDA_SESSION_SERVER=0). The channel
            // attaches asynchronously, so the permission mode is not forced here
            // — this path keeps the server default. The server path above is the
            // real one.
            let state = self.create_agent_session(None, slot_cwd.clone(), cx);
            let new_id = self.show_local_session(
                AgentSession {
                    state,
                    label,
                    cwd: slot_cwd,
                    resume_id: None,
                },
                cx,
            );
            self.start_session_pump(new_id, cx);
        }

        if let Some(mut c) = self.agent_mut(cx) {
            c.editor.begin_insert();
            c.status = Some("session cleared".into());
        }
        self.save_agent_ring(cx);
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
        let snapshot = self.focused_bound_session().and_then(|id| {
            self.read_session(id, cx, |s| (s.permission_mode, s.channel.is_some()))
        });
        let (current, has_channel) = match snapshot {
            Some(v) => v,
            None => {
                if let Some(mut c) = self.agent_mut(cx) {
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
                    if let Some(mut claude) = self.agent_mut(cx) {
                        claude.permission_mode = next;
                        let msg = format!("permission mode → {}", next.short_label());
                        Self::append_system_notice(&mut claude, &msg);
                        claude.status = Some(msg.into());
                    }
                }
                Err(e) => {
                    if let Some(mut claude) = self.agent_mut(cx) {
                        claude.status = Some(format!("permission mode change failed: {e}").into());
                    }
                }
            }
        } else if has_channel {
            // Legacy direct-spawn fallback: the live channel is the authority.
            // Flip it AND keep the session-state mirror in sync.
            if let Some(mut claude) = self.agent_mut(cx) {
                if let Some(ch) = &claude.channel {
                    ch.set_permission_mode(next);
                }
                claude.permission_mode = next;
                let msg = format!("permission mode → {}", next.short_label());
                Self::append_system_notice(&mut claude, &msg);
                claude.status = Some(msg.into());
            }
        } else {
            // Neither a server session nor a local channel — nothing to drive.
            if let Some(mut claude) = self.agent_mut(cx) {
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
    ///
    /// No longer on the agent (space) menu (untitled.md removed detach/attach as
    /// user commands); kept as internal machinery for a future re-wiring.
    #[allow(dead_code)]
    pub(crate) fn detach_active_agent_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let already_detached = self
            .with_session(id, cx, |claude| {
                if claude.channel.is_none() && claude.attach_pending.is_none() {
                    claude.status = Some("session is already detached".into());
                    return true;
                }
                // Drop runs `kill_on_drop` on the subprocess; cancel any
                // in-flight attach by dropping its receiver (the spawning
                // thread's send fails silently when the connection drops).
                claude.channel = None;
                claude.attach_pending = None;
                claude.turn_phase = TurnPhase::Idle;
                Self::append_system_notice(claude, "session detached");
                claude.status = Some("session detached".into());
                false
            })
            .unwrap_or(true);
        if !already_detached {
            self.save_agent_ring(cx);
        }
        cx.notify();
    }

    /// Spawn a fresh `AcpChannelClient` for the active session. Per spec §4
    /// re-attach does NOT resume the previous conversation — the agent
    /// subprocess was killed on detach, so the session is gone. Clear
    /// `resume_id` so persistence captures the new channel's id once it
    /// binds (rather than retrying the original-load id forever).
    ///
    /// No longer on the agent (space) menu (untitled.md removed detach/attach as
    /// user commands); kept as internal machinery for a future re-wiring.
    #[allow(dead_code)]
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
            .read_session(id, cx, |s| s.server_managed)
            .unwrap_or(false);
        if has_server && server_managed {
            if let Some(mut c) = self.agent_mut(cx) {
                c.status = Some("session is server-managed — it reconnects automatically".into());
            }
            cx.notify();
            return;
        }
        let already_attached = self
            .read_session(id, cx, |c| c.channel.is_some() || c.attach_pending.is_some())
            .unwrap_or(false);
        if already_attached {
            if let Some(mut c) = self.agent_mut(cx) {
                c.status = Some("session is already attached".into());
            }
            cx.notify();
            return;
        }

        // Use the session's per-session cwd (spec-agent-cwd.md §3) so a session
        // that lives at /foo re-attaches at /foo, not at the launch directory.
        let slot_cwd = self.sessions.get(id).map(|s| s.read(cx).cwd.clone());
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

        if let Some(ent) = self.session_entity(id) {
            ent.update(cx, |session, scx| {
                session.resume_id = None;
                session.state.attach_pending = Some(attach_rx);
                Self::append_system_notice(&mut session.state, "attaching new session…");
                session.state.status = Some("attaching new session…".into());
                scx.notify();
            });
        }
        self.start_session_pump(id, cx);
        self.save_agent_ring(cx);
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
                if let Some(mut c) = self.agent_mut(cx) {
                    c.status = Some(format!("reboot failed: {e}").into());
                }
            }
        }
    }

    /// Send the user's pending draft (`extract_editable_inserts` —
    /// only the editable runs between/after frozen Claude turns) as the
    /// next ACP prompt, then lock the turn so that content can't be
    /// retroactively edited.
    /// Toggle the Tasklist (Plan) bottom panel (§24). If this closes the last
    /// open panel while it holds focus, leave panel focus (INV-UX-12: you can't
    /// be panel-focused with no panel open).
    pub(crate) fn toggle_tasklist(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx) {
            c.tasklist_open = !c.tasklist_open;
            c.reseat_panel_focus();
        }
        cx.notify();
    }

    /// Toggle the Subagents bottom panel (§28). Mirrors `toggle_tasklist`'s
    /// re-seat of panel focus when the active column closes (INV-UX-12).
    pub(crate) fn toggle_subagents(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx) {
            c.subagents_open = !c.subagents_open;
            c.reseat_panel_focus();
        }
        cx.notify();
    }

    /// Set the focused sub-agent by its stable tool-call key (§27). The
    /// main transcript swap is purely a render-time decision; this just
    /// flips the field. Keying by `ToolCallKey` (not a positional index)
    /// keeps focus pinned to the same sub-agent regardless of how the
    /// derived `subagents()` list is ordered (ADR-0006 quick win #1).
    pub(crate) fn focus_subagent(&mut self, key: ToolCallKey, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx)
            && c.tools.calls.contains_key(&key)
        {
            c.focused_subagent = Some(key);
        }
        cx.notify();
    }

    /// Return focus from a sub-agent transcript to the root agent (§27).
    pub(crate) fn unfocus_subagent(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx) {
            c.focused_subagent = None;
        }
        cx.notify();
    }

    /// Enter or leave the focused bottom-panel region (Cmd-0, INV-UX-12).
    /// Toggle: already focused → exit (restore prior focus); otherwise focus it
    /// iff at least one column has a selectable row, landing on the first such
    /// column (Plan, else Subagents) and remembering the prior focus so `Esc`
    /// can restore it. No-op (with a hint) when nothing is open to focus.
    pub(crate) fn focus_agent_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx) {
            if c.focus == AgentFocus::Panel {
                c.focus = c.panel_return_focus;
            } else {
                let cols = c.panel_open_columns();
                if cols.is_empty() {
                    c.status = Some("no panel to focus — open Plan/Subagents".into());
                } else {
                    c.panel_return_focus = c.focus;
                    c.focus = AgentFocus::Panel;
                    if !cols.contains(&c.panel_col) {
                        c.panel_col = cols[0];
                    }
                    let n = c.panel_column_rows(c.panel_col).len();
                    if c.panel_sel >= n {
                        c.panel_sel = n.saturating_sub(1);
                    }
                }
            }
        }
        cx.notify();
        // Entering the panel previews the first highlighted row — a Subagent
        // swaps into the main view (no-op when this call TOGGLED the panel off —
        // reveal requires focus).
        self.reveal_panel_selection(cx);
    }

    /// Leave panel focus, restoring the focus captured on entry (Esc). No-op if
    /// not currently panel-focused.
    pub(crate) fn exit_agent_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx)
            && c.focus == AgentFocus::Panel
        {
            c.focus = c.panel_return_focus;
        }
        cx.notify();
    }

    /// Move the selection by `delta` rows WITHIN the active column (vim `j`/`k`,
    /// arrows), clamped. No-op unless panel-focused with rows present.
    pub(crate) fn panel_move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx)
            && c.focus == AgentFocus::Panel
        {
            let n = c.panel_column_rows(c.panel_col).len();
            if n > 0 {
                let cur = c.panel_sel.min(n - 1) as isize;
                c.panel_sel = (cur + delta).clamp(0, n as isize - 1) as usize;
            }
        }
        // Live-preview: highlighting a Subagent swaps the main view to it.
        self.reveal_panel_selection(cx);
    }

    /// Switch the active column (vim `h` = left/Tasklist, `l` = right/Subagents)
    /// to the adjacent OPEN column in `dir` (-1 left / +1 right), clamping the
    /// row into the new column. No-op if there is no such column.
    pub(crate) fn panel_switch_column(&mut self, dir: isize, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx)
            && c.focus == AgentFocus::Panel
        {
            let cols = c.panel_open_columns();
            if let Some(pos) = cols.iter().position(|&col| col == c.panel_col) {
                let np = pos as isize + dir;
                if np >= 0 && (np as usize) < cols.len() {
                    c.panel_col = cols[np as usize];
                    let n = c.panel_column_rows(c.panel_col).len();
                    if c.panel_sel >= n {
                        c.panel_sel = n.saturating_sub(1);
                    }
                }
            }
        }
        // Live-preview: highlighting a Subagent swaps the main view to it.
        self.reveal_panel_selection(cx);
    }

    /// Jump the selection to the first (`g`) or last (`G`) row of the active
    /// column.
    pub(crate) fn panel_select_end(&mut self, last: bool, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx)
            && c.focus == AgentFocus::Panel
        {
            let n = c.panel_column_rows(c.panel_col).len();
            if n > 0 {
                c.panel_sel = if last { n - 1 } else { 0 };
            }
        }
        // Live-preview: highlighting a Subagent swaps the main view to it.
        self.reveal_panel_selection(cx);
    }

    /// Live-preview the currently HIGHLIGHTED panel row. Highlighting a Subagent
    /// SWAPS the main view to that subagent's context (`focused_subagent`);
    /// highlighting a Plan row clears the swap so the main transcript returns
    /// (the plan is read in the panel itself). No-op unless panel-focused with a
    /// row. Called after every highlight move.
    pub(crate) fn reveal_panel_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(mut c) = self.agent_mut(cx) {
            if c.focus != AgentFocus::Panel {
                return;
            }
            let rows = c.panel_column_rows(c.panel_col);
            if rows.is_empty() {
                return;
            }
            let Some(item) = rows.get(c.panel_sel.min(rows.len() - 1)).cloned() else {
                return;
            };
            match &item {
                PanelItem::Subagent(key) if c.tools.calls.contains_key(key) => {
                    c.focused_subagent = Some(key.clone());
                }
                _ => c.focused_subagent = None,
            }
        }
        cx.notify();
    }

    /// Activate the selected row of the active column (`Enter`): commit the
    /// live-preview (a Subagent stays swapped into the main view), then leave
    /// panel focus so it's readable. A Plan row leaves the main transcript in
    /// place (the plan is read in the panel). Back / `Esc` returns to the main
    /// view (`focused_subagent = None`).
    pub(crate) fn panel_activate_selection(&mut self, cx: &mut Context<Self>) {
        // Preview while STILL panel-focused (reveal_panel_selection requires it),
        // then exit — the swap (`focused_subagent`) persists past the exit.
        self.reveal_panel_selection(cx);
        self.exit_agent_panel(cx);
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
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        self.with_session(id, cx, |claude| {
            // Model C (`design-c.md` §4.3): toggling is purely a placement flip.
            // The compose buffer (draft text, cursor, undo) never moves — so the
            // toggle is lossless by construction, the old "move the draft into the
            // transcript / drop it" dance is gone.
            claude.input_surface.mode = match claude.input_surface.mode {
                InputModeKind::Worksheet => InputModeKind::Chatbox,
                InputModeKind::Chatbox => InputModeKind::Worksheet,
            };
            // INV-UX-9: the worksheet rests in transcript navigation (free cursor
            // in the buffer); the chatbox rests in the compose. Entering worksheet
            // with an existing draft keeps it as an open You-block; otherwise it
            // starts in pure navigation. Entering chatbox focuses the box.
            match claude.input_surface.mode {
                InputModeKind::Worksheet => {
                    // A block exists only while IDLE (rule 7); mid-turn the draft is
                    // the chatbox. Reopen a block (at a fresh legal anchor) only when
                    // idle with a non-empty draft (bug-hunt 3). Otherwise rest in nav.
                    let has_draft = !claude.input_surface.compose().text().trim().is_empty();
                    let idle = !claude.turn_phase.is_awaiting();
                    if has_draft && idle {
                        claude.you_block_open = true;
                        // Fresh anchor at the caret if legal, else the tail (None).
                        let l = claude.editor.cursor().line;
                        claude.you_block_anchor =
                            claude.you_block_anchor_is_legal(l).then_some(l);
                        claude.focus = AgentFocus::Compose;
                    } else {
                        claude.close_you_block();
                        claude.focus = AgentFocus::Transcript;
                    }
                }
                InputModeKind::Chatbox => {
                    // Leaving the worksheet: the block is no longer inline (clears the
                    // anchor so it can't be reopened stale on return — bug-hunt 1).
                    claude.close_you_block();
                    claude.focus = AgentFocus::Compose;
                }
            }
        });
        cx.notify();
    }

    /// Toggle keyboard focus between the compose draft and the read-only
    /// transcript (Model C §4.5). Entering `Transcript` lets the user navigate
    /// and select committed history (then `S` sends the selection); `Esc` (or
    /// this toggle again) returns to the compose. The transcript editor is
    /// pinned to Normal mode for read-only navigation.
    pub(crate) fn toggle_agent_focus(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        self.with_session(id, cx, |claude| {
            match claude.focus {
                AgentFocus::Compose => {
                    // → Transcript navigation. In the worksheet an EMPTY block is
                    // discarded (no phantom); a non-empty one persists (rule 4).
                    claude.mode = EditMode::Normal;
                    if !claude.input_surface.is_chatbox()
                        && claude.input_surface.compose().text().trim().is_empty()
                    {
                        claude.close_you_block();
                    }
                    claude.focus = AgentFocus::Transcript;
                }
                AgentFocus::Transcript => {
                    // → Compose. INVARIANT: in the worksheet, focus=Compose only when
                    // a block is open (else focus-into-the-void — B1). So idle ⇒ open
                    // a block; mid-turn ⇒ leave focus on the transcript (typing
                    // already routes to the bottom chatbox, and keeping Transcript
                    // means the turn can end via stop/finalize without stranding
                    // focus=Compose over a vanished box — the fuzzer-found edge). Only
                    // chatbox mode (always-visible box) focuses the compose directly.
                    if claude.input_surface.is_chatbox() {
                        claude.focus = AgentFocus::Compose;
                    } else if !claude.turn_phase.is_awaiting() {
                        claude.open_you_block_at_cursor();
                    }
                }
                AgentFocus::Panel => {
                    // Reachable only if a focus toggle races panel mode; treat it
                    // as leaving the panel back to the captured focus.
                    claude.focus = claude.panel_return_focus;
                }
            }
        });
        cx.notify();
    }

    /// Submit the user's draft to the agent. Model C: both placements share one
    /// path — the compose text is appended + frozen at the transcript EOF and
    /// sent (`submit_compose`). No per-placement dispatch.
    pub(crate) fn submit_agent(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        // Re-enable auto-scroll when the user sends a message.
        self.with_session(id, cx, |c| c.follow_output.set(true));
        self.submit_compose(cx);
    }

    /// Whether any agent slot (across all tabs/tiles) is mid-turn. Cheap
    /// traversal the pumps use to decide whether an idle animation tick is
    /// worth a re-render.
    pub(crate) fn any_agent_awaiting(&self, cx: &GpuiApp) -> bool {
        self.sessions
            .iter()
            .any(|(_, s)| s.read(cx).state.turn_phase.is_awaiting())
    }

    /// Whole-second fingerprint of the thinking-indicator clock across all
    /// awaiting agents, or `None` if nothing is awaiting. The indicator only
    /// displays `mm:ss`-granular elapsed/quiet timers, so the pump uses this to
    /// notify (and trigger the full transcript re-render) at most ~1Hz instead
    /// of every 120ms — 8x fewer O(transcript) rebuilds during a stall. We fold
    /// elapsed + quiet seconds into one value so a change in either repaints.
    pub(crate) fn awaiting_anim_fingerprint(&self, cx: &GpuiApp) -> Option<u64> {
        let mut any = false;
        let mut fp: u64 = 0;
        for (_, ent) in self.sessions.iter() {
            let s = ent.read(cx);
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
        // Only meaningful mid-turn.
        let awaiting = self
            .agent_read(cx, |c| c.turn_phase.is_awaiting())
            .unwrap_or(false);
        if !awaiting {
            if let Some(mut claude) = self.agent_mut(cx) {
                claude.status = Some("nothing to stop".into());
            }
            cx.notify();
            return;
        }

        // Second Stop while a cancel is already pending escalates to a hard
        // kill + resume — for a turn wedged on a hung upstream request the
        // cooperative `session/cancel` may never land.
        let escalate = self
            .agent_read(cx, |c| c.turn_phase.stop_requested())
            .unwrap_or(false);
        if escalate {
            // Record the escalation on the phase before the hard kill so the
            // transition stays a total function over `TurnPhase` (the marker is
            // transient — `force_restart_agent` drops to Idle immediately after).
            if let Some(mut claude) = self.agent_mut(cx) {
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
        } else {
            self.agent_read(cx, |claude| match claude.channel.as_ref() {
                Some(channel) => {
                    channel.cancel();
                    true
                }
                None => false,
            })
            .unwrap_or(false)
        };
        if let Some(mut claude) = self.agent_mut(cx) {
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
            if let Some(mut claude) = self.agent_mut(cx) {
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
            .read_session(id, cx, |s| {
                s.channel.as_ref().and_then(|ch| ch.session_id())
            })
            .flatten();
        let slot_cwd = self.sessions.get(id).map(|s| s.read(cx).cwd.clone());
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
        if let Some(ent) = self.session_entity(id) {
            ent.update(cx, |session, scx| {
                session.resume_id = resume_id;
                session.state.channel = None; // Drop → kills the wedged subprocess.
                session.state.attach_pending = Some(attach_rx);
                session.state.turn_phase = TurnPhase::Idle;
                Self::append_system_notice(
                    &mut session.state,
                    "force-restarting agent (resuming session)…",
                );
                session.state.status =
                    Some("force-restarting agent (resuming session)…".into());
                scx.notify();
            });
        }
        self.start_session_pump(id, cx);
        self.save_agent_ring(cx);
        cx.notify();
    }

    /// Submit the compose draft (Model C — unified for both placements). Reads
    /// `compose().text()`, sends, and on success appends + freezes it at the
    /// transcript EOF via `insert_user_turn` (the reconciler — dedups the server
    /// echo, single-sources turn numbering), then resets the compose preserving
    /// placement. This is correct for worksheet only because the draft now lives
    /// in a separate buffer (INV-1/2), not in the transcript — so there is no
    /// in-place per-line freeze; `submit_worksheet`/`commit_worksheet_turn` are
    /// deleted (design-c.md §4.1).
    pub(crate) fn submit_compose(&mut self, cx: &mut Context<Self>) {
        // Capture server path info before borrowing the session.
        let server_sid = self.active_server_session_id();
        let Some(id) = self.focused_bound_session() else {
            return;
        };

        // INV-UX-9 rules 5/6: an IDLE worksheet submit sends ALL You-blocks (the
        // active draft + every parked insertion point) as one combined prompt and
        // freezes each in place. MID-TURN there are no You-blocks (editing is
        // idle-only) — the compose is the steering chatbox, so fall through to the
        // single append-at-EOF steer path. The chatbox placement also uses it.
        let worksheet_idle = self
            .agent_read(cx, |c| {
                !c.input_surface.is_chatbox() && !c.turn_phase.is_awaiting()
            })
            .unwrap_or(false);
        if worksheet_idle {
            self.submit_worksheet_blocks(id, server_sid, cx);
            return;
        }

        // Read the compose draft + validate sendability inside one session
        // borrow. `Some(None)` ⇒ early-out with a status already set; `None` ⇒
        // no session (no status); `Some(Some(text))` ⇒ proceed.
        let text = match self.with_session(id, cx, |claude| {
            let text = claude.input_surface.compose().text();
            if text.trim().is_empty() {
                claude.status = Some("nothing to send".into());
                return Some(None);
            }
            if claude.channel.is_none() && server_sid.is_none() {
                claude.status = Some("no channel attached".into());
                return Some(None);
            }
            Some(Some(text))
        }) {
            Some(Some(Some(text))) => text,
            Some(Some(None)) => {
                cx.notify();
                return;
            }
            _ => return,
        };

        // Typed `/clear` is the escape hatch around the `claude-clear` command.
        // Forwarding the literal text to the agent makes the clear INVISIBLE to
        // yalda (Claude Code resets its own context with no ACP signal), so on
        // resume yalda's WAL replays + reloads the pre-`/clear` context. Route a
        // typed `/clear` to yalda's own session reset instead — it mints a NEW
        // server session, a durable boundary resume cannot cross
        // (spec-session-recall-integrity A2).
        if text.trim() == "/clear" {
            self.clear_agent_session(cx);
            return;
        }

        // STEERING (spec-turn-steering.md, INV-UX-7): a submit is delivered
        // IMMEDIATELY — even mid-turn. For agents that advertise `promptQueueing`
        // (claude-agent-acp) the worker forwards the prompt concurrently, so the
        // agent receives the steer while the current turn is still streaming and
        // processes it the instant that turn finishes. `send_prompt_to_session`
        // commits the user turn on a successful write. On send FAILURE we LEAVE
        // the draft in the compose (no clear, no queue) with a status so the user
        // can retry — the message is never moved out of sight or dropped.
        // INV-UX-9 rule 5: a worksheet reply freezes IN PLACE at the You-block's
        // anchor (between the latest turn's lines); chatbox / tail submits append
        // at EOF (anchor = None). Only meaningful when a block is open + idle.
        let anchor = self
            .agent_read(cx, |c| {
                c.inline_you_block_active()
                    .then(|| c.effective_you_block_anchor())
                    .flatten()
            })
            .flatten();
        if self.send_prompt_to_session(id, &text, anchor, cx) {
            self.with_session(id, cx, |claude| {
                // Reset the compose, PRESERVING placement (Model C §4.1).
                let mode = claude.input_surface.mode;
                claude.input_surface = InputSurface::new(mode);
                // INV-UX-9 (rule 4): a worksheet submit closes the You-block — the
                // reply was frozen by the reconciler — and rests in transcript NAV
                // (focus=Transcript). It does NOT switch to focus=Compose: a turn can
                // end via stop/force-restart (not just finalize), and focus=Compose
                // would then outlive the vanished mid-turn chatbox → invisible compose
                // (the fuzzer-found B1 edge). Mid-turn typing still reaches the
                // chatbox: the transcript is treated unfocused while awaiting, and
                // `focused_in_insert_mode` suppresses leaders mid-turn.
                if mode == InputModeKind::Worksheet {
                    claude.close_you_block();
                    claude.focus = AgentFocus::Transcript;
                }
            });
        } else if let Some(mut claude) = self.agent_mut(cx) {
            // Send failed: leave the draft intact so the user can retry, and
            // surface it instead of dropping the message into the void.
            claude.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        cx.notify();
    }

    /// Worksheet submit (INV-UX-9 rules 5/6): gather every You-block (the active
    /// draft + all parked insertion points), send their COMBINED text as one prompt,
    /// and on success freeze each block IN PLACE under one user turn, then rest in
    /// nav. On failure the drafts are kept. `/clear` as the sole content still routes
    /// to the session reset.
    fn submit_worksheet_blocks(
        &mut self,
        id: SessionId,
        server_sid: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let blocks = self
            .agent_read(cx, |c| c.collect_you_blocks())
            .unwrap_or_default();
        if blocks.is_empty() {
            if let Some(mut c) = self.agent_mut(cx) {
                c.status = Some("nothing to send".into());
            }
            cx.notify();
            return;
        }
        // `/clear` escape hatch (only when it is the sole content).
        if blocks.len() == 1 && blocks[0].1.trim() == "/clear" {
            self.clear_agent_session(cx);
            return;
        }
        let no_channel = self.agent_read(cx, |c| c.channel.is_none()).unwrap_or(true)
            && server_sid.is_none();
        if no_channel {
            if let Some(mut c) = self.agent_mut(cx) {
                c.status = Some("no channel attached".into());
            }
            cx.notify();
            return;
        }
        // Combined prompt in document order, blocks separated by a blank line so the
        // agent reads each annotation distinctly.
        let combined = blocks
            .iter()
            .map(|(_, t)| t.trim_end_matches('\n').to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
        let sent = if let Some(sid) = &server_sid {
            self.session_server
                .as_ref()
                .and_then(|s| s.prompt(sid, &combined).ok())
                .is_some()
        } else {
            self.with_session_silent(id, cx, |c| {
                c.channel
                    .as_mut()
                    .map(|ch| ch.send(&combined).is_ok())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        };
        if sent {
            self.with_session(id, cx, |c| {
                // Mint ONE turn for the whole submit; freeze each block in place
                // under it (the reconciler suppresses the combined echo).
                if let Some(k) = c.register_user_turn(
                    &combined,
                    yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
                    false,
                ) {
                    c.freeze_you_blocks(&blocks, k);
                }
                // Clear every block, rest in nav, follow the reply.
                c.input_surface = InputSurface::new(InputModeKind::Worksheet);
                c.close_you_block();
                c.focus = AgentFocus::Transcript;
                c.follow_output.set(true);
                if !matches!(c.turn_phase, TurnPhase::Awaiting { .. }) {
                    c.turn_phase = TurnPhase::begin(std::time::Instant::now());
                }
            });
        } else if let Some(mut c) = self.agent_mut(cx) {
            c.status = Some("send failed — reconnecting; press ⏎ to retry".into());
        }
        cx.notify();
    }

    /// Send `text` to session `id` as an ACP prompt and, on success, commit the
    /// optimistic user turn + begin the turn (unless one is already cleanly
    /// awaiting — a mid-turn steer rides it). Returns whether the prompt was
    /// written (server `prompt` is fire-and-forget — `Ok` means written, not
    /// accepted). Does NOT touch the compose surface, so it is safe to call for a
    /// non-focused session. (spec-turn-steering.md)
    pub(crate) fn send_prompt_to_session(
        &mut self,
        id: SessionId,
        text: &str,
        anchor: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let server_sid = self.sessions.sid_of(id).map(|s| s.to_string());
        let prompt_body = text.trim_end_matches('\n').to_string();
        let sent = if let Some(sid) = &server_sid {
            self.session_server
                .as_ref()
                .and_then(|s| s.prompt(sid, &prompt_body).ok())
                .is_some()
        } else {
            self.with_session_silent(id, cx, |claude| {
                claude
                    .channel
                    .as_mut()
                    .map(|ch| ch.send(&prompt_body).is_ok())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        };
        if sent {
            self.with_session(id, cx, |claude| {
                // `LocalSubmit` always inserts + records so the stream echo that
                // follows (server `UserPrompt` / agent `UserMessage`) is
                // suppressed. Never advances the replay boundary on a live send.
                // INV-UX-9 rule 5: a worksheet reply with a between-lines anchor
                // freezes IN PLACE; otherwise (chatbox / tail) it appends at EOF.
                match anchor {
                    Some(after_line) => claude.insert_user_turn_at(
                        after_line,
                        text,
                        yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
                        false,
                    ),
                    None => claude.insert_user_turn(
                        text,
                        yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
                        false,
                    ),
                }
                // Begin a turn unless one is ALREADY CLEANLY awaiting — a mid-turn
                // steer (promptQueueing) rides the in-flight turn, so we don't
                // reset its elapsed/quiet clocks. But `StopRequested` (a graceful
                // cancel pending) must be superseded: a fresh steer means the user
                // wants to keep going, so begin() clears the "stopping…" state
                // (matching main, which always begin()'d). Only the clean
                // `Awaiting` case is preserved.
                if !matches!(claude.turn_phase, TurnPhase::Awaiting { .. }) {
                    claude.turn_phase = TurnPhase::begin(std::time::Instant::now());
                }
            });
        }
        sent
    }

    // ── Session recap (recap-panel) ─────────────────────────────────────────
    //
    // A recap is a one-off, LLM-generated prose summary of the focused session's
    // conversation, requested manually and pinned at the top of the jump panel
    // until dismissed (INV-UX-20). Generation runs on a THROWAWAY
    // `AcpChannelClient` — a private side-channel worker fed the transcript text
    // inline — so its reply stream never routes through the visible transcript
    // reducer (`apply_reply_events`). The reply-application logic
    // (`apply_recap_event` / `finalize_recap`) is factored out of the pump so it
    // is headlessly testable with synthetic `ReplyEvent`s (the live subprocess is
    // the sole genuine gap, per dev-system § Verification harness gap 2).

    /// Cap on how much transcript we stuff into the recap prompt. A recap only
    /// needs the shape of the conversation; the tail carries the current state,
    /// so we keep the last N chars rather than blowing the context on a huge
    /// history. Trimmed at a line boundary in `build_recap_prompt`.
    const RECAP_TRANSCRIPT_BUDGET: usize = 24_000;

    /// Summon (or re-run) a recap of the focused agent session (menu
    /// `recap-session`). Snapshots the session's transcript, flips the panel to
    /// `Generating`, and kicks off the throwaway worker. Re-running while one is
    /// live bumps the run token so the prior worker's late updates are ignored
    /// and its subprocess is torn down. No-op with a clear status when there's no
    /// focused session or nothing to summarize.
    pub(crate) fn summon_recap(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_bound_session() else {
            self.transient_status = Some("no agent session focused to recap".into());
            cx.notify();
            return;
        };
        self.start_recap_for(id, cx);
    }

    /// Re-run the pinned recap against the session it already targets (the panel
    /// `⟳` button), independent of which tile currently has focus.
    pub(crate) fn rerun_recap(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.recap.as_ref().map(|r| r.session_id) {
            self.start_recap_for(id, cx);
        }
    }

    /// Shared core of summon / re-run: snapshot `id`'s transcript, flip the panel
    /// to `Generating`, and kick off the throwaway worker.
    fn start_recap_for(&mut self, id: SessionId, cx: &mut Context<Self>) {
        let Some(ent) = self.sessions.get(id).cloned() else {
            return;
        };
        let (label, cwd, transcript) = {
            let s = ent.read(cx);
            (
                s.label.clone(),
                s.cwd.clone(),
                s.state.editor.document().full_text(),
            )
        };
        if transcript.trim().is_empty() {
            self.transient_status = Some("nothing to recap yet".into());
            cx.notify();
            return;
        }

        // Bump the run token; assigning a fresh `RecapState` drops any prior one,
        // whose Drop tears down its worker + pump (superseding an in-flight run).
        let token = self.recap.as_ref().map(|r| r.token.wrapping_add(1)).unwrap_or(1);
        self.recap = Some(RecapState {
            session_label: label,
            session_id: id,
            status: RecapStatus::Generating,
            text: String::new(),
            token,
            channel: None,
            _pump: None,
        });
        // The recap lives in the jump panel — make sure it's visible so the user
        // sees the result of the command they just invoked.
        if !self.jump_panel_visible {
            self.jump_panel_visible = true;
        }
        cx.notify();
        self.spawn_recap_worker(token, cwd, transcript, cx);
    }

    /// Dismiss the pinned recap (menu `recap-dismiss`). Clears the panel and, via
    /// `RecapState`'s Drop, tears down any live worker + pump.
    pub(crate) fn dismiss_recap(&mut self, cx: &mut Context<Self>) {
        if self.recap.take().is_some() {
            cx.notify();
        }
    }

    /// Build the recap prompt: a tail-trimmed transcript wrapped with a terse
    /// instruction. Kept pure so the trimming/budget is unit-testable.
    fn build_recap_prompt(transcript: &str) -> String {
        let trimmed = if transcript.len() > Self::RECAP_TRANSCRIPT_BUDGET {
            let start = transcript.len() - Self::RECAP_TRANSCRIPT_BUDGET;
            // Advance to the next line boundary so we don't slice mid-line.
            let start = transcript[start..]
                .find('\n')
                .map(|off| start + off + 1)
                .unwrap_or(start);
            format!("…(earlier conversation elided)…\n{}", &transcript[start..])
        } else {
            transcript.to_string()
        };
        format!(
            "Write a brief recap of the coding-assistant conversation below, so the \
             user can re-orient at a glance. Use 3–6 short bullet points covering: \
             what the user is working on, the key decisions and actions taken, and \
             the current state / next step. Output ONLY the recap — no preamble, no \
             restating this instruction.\n\n<conversation>\n{trimmed}\n</conversation>"
        )
    }

    /// Spawn the throwaway recap worker in the background (blocking handshake +
    /// first prompt), then hand the live channel back to the view to start
    /// pumping. On any spawn/send error the recap flips to `Failed`.
    fn spawn_recap_worker(
        &self,
        token: u64,
        cwd: PathBuf,
        transcript: String,
        cx: &mut Context<Self>,
    ) {
        // The throwaway worker is a REAL subprocess (dev-system § Verification
        // harness gap 2). Headless tests drive the reducer (`apply_recap_event` /
        // `finalize_recap`) directly and must never fork an agent — so summon
        // leaves the panel `Generating` with no channel and the test feeds
        // synthetic `ReplyEvent`s. The live spawn→pump wiring is exercised at
        // runtime only.
        if cfg!(test) {
            return;
        }
        let prompt = Self::build_recap_prompt(&transcript);
        let cmd = std::env::var("YALDA_ACP_AGENT").unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let spawned: Result<AcpChannelClient, String> = cx
                .background_executor()
                .spawn(async move {
                    match AcpChannelClient::spawn(&cmd, Some(cwd)) {
                        Ok(mut ch) => match ch.send(&prompt) {
                            Ok(()) => Ok(ch),
                            Err(e) => Err(format!("recap send failed: {e}")),
                        },
                        Err(e) => Err(format!("recap agent unavailable: {e}")),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| match spawned {
                Ok(ch) => this.install_recap_channel(token, ch, cx),
                Err(e) => this.fail_recap(token, e, cx),
            });
        })
        .detach();
    }

    /// Adopt the freshly-spawned recap worker and start its pump — unless the run
    /// was superseded/dismissed while we were spawning (token mismatch), in which
    /// case `ch` is dropped here (killing the now-orphan subprocess).
    fn install_recap_channel(
        &mut self,
        token: u64,
        ch: AcpChannelClient,
        cx: &mut Context<Self>,
    ) {
        match self.recap.as_mut() {
            Some(r) if r.token == token => r.channel = Some(ch),
            _ => return, // superseded — drop ch
        }
        self.start_recap_pump(token, cx);
    }

    /// Drive the recap worker's reply stream. Event-driven via the channel's wake
    /// receiver (falling back to a short poll), draining into `apply_recap_event`
    /// and finalizing when the turn resolves or the worker dies.
    fn start_recap_pump(&mut self, token: u64, cx: &mut Context<Self>) {
        let wake = self
            .recap
            .as_ref()
            .and_then(|r| r.channel.as_ref())
            .and_then(|ch| ch.take_wake_receiver());
        let pump = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;
            let mut wake = wake;
            loop {
                if let Some(rx) = wake.as_mut() {
                    let timer = cx.background_executor().timer(Duration::from_millis(50));
                    futures::select_biased! {
                        _ = rx.next().fuse() => {}
                        _ = timer.fuse() => {}
                    }
                    while rx.next().now_or_never().flatten().is_some() {}
                } else {
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                }
                let keep_going = this.update(cx, |this, cx| this.drain_recap(token, cx));
                match keep_going {
                    Ok(true) => {}
                    _ => return, // done, superseded, or view gone
                }
            }
        });
        if let Some(r) = self.recap.as_mut() {
            r._pump = Some(pump);
        }
    }

    /// Drain any queued reply events into the recap, then decide whether the run
    /// is complete. Returns `true` to keep pumping, `false` when finished or
    /// superseded. Draining BEFORE reading `turn_count` guarantees every chunk
    /// enqueued before the turn boundary is applied before we finalize.
    fn drain_recap(&mut self, token: u64, cx: &mut Context<Self>) -> bool {
        // Bail if this run is no longer the current one (dismissed / re-run).
        let current = matches!(
            self.recap.as_ref(),
            Some(r) if r.token == token && r.status == RecapStatus::Generating
        );
        if !current {
            return false;
        }
        let mut events: Vec<yalda::acp_channel::ReplyEvent> = Vec::new();
        let (connected, turns) = match self.recap.as_ref().and_then(|r| r.channel.as_ref()) {
            Some(ch) => {
                while let Some(ev) = ch.try_recv() {
                    events.push(ev);
                }
                (ch.is_connected(), ch.turn_count())
            }
            None => (false, 0),
        };
        for ev in events {
            self.apply_recap_event(token, ev, cx);
        }
        // The worker increments `turn_count` only after the `session/prompt` RPC
        // resolves — i.e. after every chunk has been enqueued. So a climbed
        // counter is the authoritative "reply is complete" signal.
        if turns >= 1 {
            self.finalize_recap(token, cx);
            return false;
        }
        // Worker died before resolving a turn — finalize with whatever we have
        // (Ready if some text streamed, else Failed).
        if !connected {
            self.finalize_recap(token, cx);
            return false;
        }
        true
    }

    /// Apply one recap reply event. Text chunks accumulate into the panel;
    /// everything else (tool calls, plans, mode/model/usage) is irrelevant to a
    /// summary and ignored. Token-guarded so a stale pump can't scribble on a
    /// newer run.
    pub(crate) fn apply_recap_event(
        &mut self,
        token: u64,
        ev: yalda::acp_channel::ReplyEvent,
        cx: &mut Context<Self>,
    ) {
        use yalda::acp_channel::ReplyEvent;
        let Some(r) = self.recap.as_mut() else {
            return;
        };
        if r.token != token || r.status != RecapStatus::Generating {
            return;
        }
        if let ReplyEvent::Chunk(text) = ev {
            r.text.push_str(&text);
            cx.notify();
        }
    }

    /// Settle a recap run: `Ready` when text streamed, else `Failed`. Detaches
    /// the worker (a background drop so the join can't stall the foreground) but
    /// leaves the pump task to unwind on its own (`drain_recap` returns false).
    /// Token-guarded.
    pub(crate) fn finalize_recap(&mut self, token: u64, cx: &mut Context<Self>) {
        let Some(r) = self.recap.as_mut() else {
            return;
        };
        if r.token != token || r.status != RecapStatus::Generating {
            return;
        }
        r.status = if r.text.trim().is_empty() {
            RecapStatus::Failed("agent returned no summary".into())
        } else {
            RecapStatus::Ready
        };
        // Tear down the subprocess off-thread (Drop joins the worker).
        if let Some(ch) = r.channel.take() {
            cx.background_executor()
                .spawn(async move { drop(ch) })
                .detach();
        }
        cx.notify();
    }

    /// Flip the recap to `Failed` (spawn/send error). Token-guarded so a stale
    /// spawn can't fail a newer run.
    fn fail_recap(&mut self, token: u64, reason: String, cx: &mut Context<Self>) {
        if let Some(r) = self.recap.as_mut()
            && r.token == token
            && r.status == RecapStatus::Generating
        {
            r.status = RecapStatus::Failed(reason);
            r.channel = None;
            cx.notify();
        }
    }

    /// Send the transcript editor's current selection as a prompt
    /// (Agent local menu `S`, spec-menu-scopes.md). Mirrors `submit_chatbox`'s
    /// send-first-then-echo order, but takes the text from the worksheet
    /// selection and leaves the input surface untouched.
    pub(crate) fn send_agent_selection(&mut self, cx: &mut Context<Self>) {
        let server_sid = self.active_server_session_id();

        let text = match self
            .agent_read(cx, |c| c.editor.selection_text())
            .flatten()
        {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                if let Some(mut c) = self.agent_mut(cx) {
                    c.status = Some("no selection to send".into());
                }
                cx.notify();
                return;
            }
        };
        let no_channel = self
            .agent_read(cx, |c| c.channel.is_none())
            .unwrap_or(true);
        if no_channel && server_sid.is_none() {
            if let Some(mut c) = self.agent_mut(cx) {
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
        } else if let Some(mut claude) = self.agent_mut(cx) {
            if let Some(channel) = claude.channel.as_mut() {
                channel.send(&prompt_body).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        if sent {
            if let Some(mut claude) = self.agent_mut(cx) {
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
                // A turn just began: any open You-block must close so it can't
                // re-materialize as a phantom at a stale anchor when the turn ends
                // (bug-hunt 4, 10). The compose draft is left intact.
                claude.close_you_block();
                // Rest in transcript navigation (a turn is now in flight; mid-turn
                // typing routes to the chatbox without focus=Compose — see submit).
                claude.focus = AgentFocus::Transcript;
            }
        } else if let Some(mut claude) = self.agent_mut(cx) {
            claude.status = Some("send failed — selection not sent".into());
        }
        cx.notify();
    }

    /// Key dispatch for the agent window. Recognises the agent-window-
    /// scoped shortcuts (`Ctrl-Enter` submit, `Ctrl-Alt-Enter` mode toggle)
    /// before routing remaining keys to either the chatbox (in Chatbox mode)
    /// or the transcript editor (in Worksheet mode). See spec-agent-window.md §32.
    pub(crate) fn handle_claude_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Panel focus (Cmd-0, INV-UX-12) is MODAL: while the bottom-panel region
        // holds focus, vim keys move the selection and everything else is inert
        // (no leaders, no compose typing) until `Esc` / `Enter` leaves it. Checked
        // before the leaders so `j`/`k` navigate rows instead of opening menus.
        if self
            .agent_read(cx, |c| c.focus == AgentFocus::Panel)
            .unwrap_or(false)
        {
            if press.modifiers.is_empty() {
                match press.key {
                    Key::Esc => self.exit_agent_panel(cx),
                    Key::Enter => self.panel_activate_selection(cx),
                    Key::Down | Key::Char('j') => self.panel_move_selection(1, cx),
                    Key::Up | Key::Char('k') => self.panel_move_selection(-1, cx),
                    Key::Left | Key::Char('h') => self.panel_switch_column(-1, cx),
                    Key::Right | Key::Char('l') => self.panel_switch_column(1, cx),
                    Key::Char('g') => self.panel_select_end(false, cx),
                    Key::Char('G') => self.panel_select_end(true, cx),
                    _ => {}
                }
            }
            // Modal: consume every key (Cmd-bound actions still dispatch via
            // `on_action`, which is a separate path from this on_key_down).
            return;
        }

        // Universal leaders: when NOT in text entry (worksheet/chatbox Normal),
        // `<space>`/`.`/`?` open the menus with top priority.
        if self.leader_intercept(&press, cx) {
            return;
        }

        // Esc with a focused sub-agent: return to the parent transcript
        // (§27). Otherwise Esc falls through — the project rule is
        // "Esc never quits / never closes", so an unfocused-sub-agent
        // Esc keeps the existing per-mode behavior (toggle Normal etc.).
        if press.key == Key::Esc
            && self
                .agent_read(cx, |c| c.focused_subagent.is_some())
                .unwrap_or(false)
        {
            self.unfocus_subagent(cx);
            return;
        }

        // Esc does NOT stop an in-flight turn (runtime report: it conflicts too
        // easily with mode switching — Esc is the Insert→Normal / leave-block key in
        // the worksheet). Stop is `⌘.` only (`stop_agent`). Esc falls through to its
        // per-mode meaning (drop to Normal in a You-block, leave to nav, etc.).

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

        // Model C: input routes to the compose buffer in both placements; the
        // transcript is read-only in both (INV-1).

        // Bare `m`/`'` starts a mark chord ONLY in transcript navigation — agent
        // tiles are markable like any tile, but the chord must NOT fire in the
        // editable compose, or `m`/`'` become untypeable there (runtime report:
        // "can't type the m character in chatbox mode" — it fired whenever the
        // compose was in Normal mode). In the compose, `m`/`'` route to the editor
        // (typed in Insert; an ordinary motion in Normal).
        let in_normal = self
            .agent_read(cx, |c| {
                c.focus == AgentFocus::Transcript
                    || c.input_surface.compose().mode == EditMode::Normal
            })
            .unwrap_or(false);
        // The bare-`m`/`'` mark chord fires ONLY in genuine transcript navigation.
        // Mid-turn in the worksheet, `focus` stays on the transcript but input
        // routes to the bottom chatbox (INV-UX-9 rule 7) — that is text entry, so
        // `m` must TYPE, not start a chord. Use the SAME exclusion the leaders /
        // `transcript_focused` use. (Was `focus == Transcript` alone — the "mid-turn
        // m sets a mark" bug; the real fix lived only on the unmerged `jump-pane-nav`
        // branch. Pinned real-state by `real_midturn_worksheet_m_types_not_marks`.)
        let transcript_nav = self
            .agent_read(cx, |c| {
                c.focus == AgentFocus::Transcript
                    && !(c.turn_phase.is_awaiting() && !c.input_surface.is_chatbox())
            })
            .unwrap_or(false);
        if transcript_nav && self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
            return;
        }

        // User-turn jump mode (agent (space) menu → "jump between user turns"): when
        // on, bare `j`/`k` in Normal mode move the viewport between the user's
        // input turns (`k` = older/up, `j` = newer/down) instead of the editor
        // cursor. Normal-mode only, so Insert typing of j/k is untouched.
        if in_normal
            && press.modifiers.is_empty()
            && matches!(press.key, Key::Char('j') | Key::Char('k'))
            && self
                .agent_read(cx, |c| c.user_turn_jump_mode)
                .unwrap_or(false)
        {
            let delta = if press.key == Key::Char('j') { 1 } else { -1 };
            self.jump_user_turn(delta, false, cx);
            return;
        }

        let Some(focused_id) = self.focused_bound_session() else {
            return;
        };

        // Transcript focus (Model C §4.5): keystrokes drive READ-ONLY navigation
        // and selection over the committed transcript; `Esc` returns to the
        // compose. This is the base "workspace" capability — select history, then
        // `S` sends the selection. Edits are inert: the transcript is all frozen
        // (guards no-op them) and we pin the editor to Normal so `i`/`a` can't
        // enter Insert.
        // INV-UX-9 rule 7 (bug-hunt 6): mid-turn in the worksheet, input belongs to
        // the bottom chatbox — NOT transcript navigation. So treat the transcript as
        // unfocused while awaiting (keys fall through to the compose dispatch, which
        // edits the chatbox). Esc-interrupt / Ctrl-Enter-steer are handled earlier.
        let transcript_focused = self
            .agent_read(cx, |c| {
                c.focus == AgentFocus::Transcript
                    && !(c.turn_phase.is_awaiting() && !c.input_surface.is_chatbox())
            })
            .unwrap_or(false);
        if transcript_focused {
            // INV-UX-9: in the WORKSHEET, an Insert-entry key from transcript
            // navigation opens a **You-block** — the `Compose` becomes an inline
            // editable reply attached to the conversation (rule 2). The draft lives
            // in the separate Compose (Model C: no draft in the transcript), so
            // this only flips focus/mode; nothing is written to the transcript.
            // Stage 2: the block opens AT the caret, anchored to the cursor line,
            // so a reply lands between the latest turn's lines — gated by the
            // legal-point guard (rule 5). In a non-worksheet (chatbox) transcript
            // focus, `Esc` returns to the compose as before.
            // Rule 7: a You-block can only be opened while the agent is IDLE
            // (mid-turn input belongs to the chatbox, never an inline edit) — so
            // gate the open on `!awaiting`, keeping the transcript append-only
            // during streaming (the Model C durability property).
            let worksheet = self
                .agent_read(cx, |c| {
                    !c.input_surface.is_chatbox() && !c.turn_phase.is_awaiting()
                })
                .unwrap_or(false);
            if worksheet
                && press.modifiers.is_empty()
                && matches!(press.key, Key::Char('i' | 'a' | 'o' | 'I' | 'A' | 'O'))
            {
                // INV-UX-9 rule 6: one block at a time. If a block is ALREADY open,
                // `i` RESUMES it in place (open_you_block_at_cursor is idempotent —
                // it must NOT move the reply to the caret: the "jumps around" bug). A
                // NEW block opens only at a legal point (rule 5): within the latest
                // agent turn or the tail; an older frozen turn is refused with a hint.
                // The anchor is snapped to a rendered line inside the helper (B7).
                let open = self
                    .agent_read(cx, |c| {
                        c.you_block_open || c.you_block_anchor_is_legal(c.editor.cursor().line)
                    })
                    .unwrap_or(false);
                if open {
                    self.with_session(focused_id, cx, |c| c.open_you_block_at_cursor());
                } else if let Some(mut c) = self.agent_mut(cx) {
                    c.status = Some("can only reply within the latest turn".into());
                }
                cx.notify();
                return;
            }
            if press.modifiers.is_empty() && press.key == Key::Esc {
                // Esc from transcript navigation returns focus to the compose ONLY
                // where a compose surface is actually visible: the chatbox box, or
                // the mid-turn box. In an idle WORKSHEET there is no bottom box and
                // Esc must NOT focus an invisible compose (bug-hunt-2 B1, found by the
                // fuzzer) — nav is the resting state, so Esc is a no-op there.
                if let Some(mut c) = self.agent_mut(cx)
                    && (c.input_surface.is_chatbox() || c.turn_phase.is_awaiting())
                {
                    c.focus = AgentFocus::Compose;
                }
                cx.notify();
                return;
            }
            let Some(outcome) = self.with_session_silent(focused_id, cx, |claude| {
                claude.status = None;
                claude.mode = EditMode::Normal;
                let out = Self::dispatch_normal_core(
                    &mut claude.editor,
                    &mut claude.mode,
                    &mut claude.keybinds,
                    press,
                );
                // Never leave the read-only transcript in Insert mode.
                claude.mode = EditMode::Normal;
                out
            }) else {
                return;
            };
            if !matches!(outcome, NormalOutcome::Skipped)
                && let Some(mut c) = self.agent_mut(cx)
            {
                c.pending_reveal_cursor = true;
            }
            match outcome {
                NormalOutcome::Skipped | NormalOutcome::Handled => cx.notify(),
                NormalOutcome::Yanked => {
                    if let Some(mut c) = self.agent_mut(cx) {
                        c.status = Some("yanked".into());
                    }
                    cx.notify();
                }
                NormalOutcome::Quit => cx.quit(),
                NormalOutcome::OpenMenu => self.open_menu_inner(cx),
                // No paste into the read-only transcript.
                NormalOutcome::Paste { .. } => cx.notify(),
            }
            return;
        }

        // INV-UX-9: Esc in a worksheet You-block is LAYERED, like a vim/helix editor:
        //   1st Esc (compose in Insert) → drop to Normal IN the block, keeping focus
        //      so the user can edit the reply with motions and `i`/`a` back in.
        //   2nd Esc (compose already Normal) → LEAVE the block to transcript nav. An
        //      EMPTY block discards (rule 3, byte-identical); a non-empty one persists
        //      (rule 4), pending Submit. Parked insertion points are untouched.
        // (Chatbox Esc falls through to the dispatch — it just toggles the box Normal.)
        let ws_esc_mode = self
            .agent_read(cx, |c| {
                if !c.input_surface.is_chatbox()
                    && c.focus == AgentFocus::Compose
                    && press.key == Key::Esc
                    && press.modifiers.is_empty()
                {
                    Some(c.input_surface.compose().mode)
                } else {
                    None
                }
            })
            .flatten();
        if let Some(mode) = ws_esc_mode {
            self.with_session(focused_id, cx, |c| {
                if mode == EditMode::Insert {
                    // 1st Esc: edit-in-place (Normal), stay in the block.
                    c.input_surface.compose_mut().mode = EditMode::Normal;
                } else {
                    // 2nd Esc: leave the block to navigation.
                    if c.input_surface.compose().text().trim().is_empty() {
                        c.input_surface = InputSurface::new(InputModeKind::Worksheet);
                        c.you_block_open = false;
                        c.you_block_anchor = None;
                    }
                    c.focus = AgentFocus::Transcript;
                }
                c.pending_reveal_cursor = true;
            });
            cx.notify();
            return;
        }

        // Model C: keystrokes route to the COMPOSE buffer in both placements —
        // the transcript is read-only (INV-1) and worksheet no longer edits it
        // in place. `compose_mut()` is total, so there is no per-placement branch.
        // (Compose has its own scroll/list_state, so no `pending_reveal_cursor`
        // transcript-reveal is needed for typing.)
        let Some(outcome) = self.with_session_silent(focused_id, cx, |claude| {
            claude.status = None;
            let cb = claude.input_surface.compose_mut();
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
        }) else {
            return;
        };
        // INV-UX-9 bugfix (stale inline typing): when the compose is the INLINE
        // You-block it renders INSIDE the cached `TranscriptView`, whose
        // `cx.observe(&session)` fires only on a SESSION notify. `with_session_silent`
        // above did not notify the session, so a keystroke didn't bust the
        // transcript cache — chars appeared only "later" when an unrelated event
        // notified. Notify the session here, but ONLY for the inline block: the
        // bottom-panel chatbox renders in the root (the root `cx.notify()` below
        // suffices) and must stay O(changed) — notifying the session there would
        // re-render the whole transcript per chatbox keystroke (the perf
        // regression `transcript_021_*` guards).
        let inline_active = self
            .agent_read(cx, |c| c.inline_you_block_active())
            .unwrap_or(false);
        // TEMP unconditional diagnostic for the recurring "/clear worksheet
        // invisible" bug (→ /tmp/yalda-clear-debug.log). Captures the FULL state at
        // the keystroke so ONE real reproduction is unambiguous: the exact gate, the
        // focused session id being EDITED, whether a TranscriptView is observing
        // THAT id (a mismatch ⇒ keystroke edits one session, the displayed
        // transcript observes another), and the compose length.
        {
            let g = self.agent_read(cx, |c| {
                (
                    c.you_block_open,
                    c.turn_phase.is_awaiting(),
                    c.input_surface.is_chatbox(),
                    c.focus,
                    c.input_surface.compose().text().chars().count(),
                )
            });
            let tv_exists = self.transcript_views.contains_key(&focused_id);
            let tv_ids: Vec<_> = self.transcript_views.keys().copied().collect();
            crate::clear_log(&format!(
                "keystroke: focused_id={focused_id:?} inline_active={inline_active} \
                 gate(open,awaiting,chatbox,focus,compose_len)={g:?} \
                 transcript_view_exists_for_focused={tv_exists} all_tv_ids={tv_ids:?}"
            ));
        }
        if inline_active {
            // INV-UX-1: keep the inline block's caret in view as the reply grows.
            self.with_session_silent(focused_id, cx, |c| c.pending_reveal_cursor = true);
            if let Some(ent) = self.session_entity(focused_id) {
                ent.update(cx, |_, scx| scx.notify());
            }
        }
        match outcome {
            NormalOutcome::Skipped | NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                if let Some(mut c) = self.agent_mut(cx) {
                    c.status = Some("yanked".into());
                }
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
            NormalOutcome::Paste { before } => {
                if let Some(mut c) = self.agent_mut(cx) {
                    let cb = c.input_surface.compose_mut();
                    Self::apply_paste(&mut cb.editor, before);
                }
                cx.notify();
            }
        }
    }
}
