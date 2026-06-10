//! Agent (Claude) tile UI + session-server wiring on SketchGpuiView:
//! open/attach/create/close session flows, lease heartbeat, server pump
//! + notification reducer (apply_server_batch / apply_reply_events /
//! apply_agent_event), submit paths, and the Claude key handler.
//! render_agent itself lives in main.rs this pass. Extracted verbatim
//! from main.rs (split-gpui-main, stage 2).

use super::*;

impl SketchGpuiView {
    /// Open the Claude screen and attempt to attach to an ACP agent. Bound
    /// to `Ctrl-K` in the Doc and Edit views. Stashes the prior screen so
    /// `Ctrl-V` from Claude returns to it.
    ///
    /// Attach uses `SKETCH_ACP_AGENT` if set, else the
    /// `claude-agent-acp` default (`AcpChannelClient::DEFAULT_AGENT_COMMAND`).
    pub(crate) fn open_agent(&mut self, _: &OpenAgent, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_agent_inner(cx);
    }

    pub(crate) fn open_agent_inner(&mut self, cx: &mut Context<Self>) {
        // If already on Claude screen, just add a new session to the ring.
        if matches!(
            self.workspace.focused_content().expect("no focused window"),
            App::Agent(_)
        ) {
            self.new_agent_session(None, cx);
            return;
        }

        // Stash the current screen so back_to_doc can restore it.
        let prior = self
            .workspace
            .replace_focused_content(App::Buffer(BufferApp::Viewing(DocState {
                blocks: Vec::new(),
                file_label: SharedString::new_static(""),
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                source: None,
            })))
            .expect("workspace has no focused window");

        let mut ring = AgentRing::new(Some(Box::new(prior)));
        let proc_cwd = process_cwd();

        if self.session_server.is_some() {
            // ── Session-server path (S4: non-blocking) ───────────────
            // Render IMMEDIATELY in a "connecting…" placeholder, then do the
            // (potentially slow) list_sessions / attach / create round-trips
            // on a background thread. The server pump replays each session's
            // full event_log on attach, so the transcript lands through the
            // pump — we never have to block the paint thread on an Ack. The
            // worst case the old synchronous path could hit was a ~30s freeze
            // (request `recv_timeout`) when the server stalled.
            let placeholder =
                AgentState::new_server_managed(Some("connecting to session server…".into()));
            let open_token = alloc_open_token();
            ring.push("claude-1".into(), placeholder, None, proc_cwd.clone(), None);
            // Start the unified server pump (one per view, routes by
            // session_id) and stash it on the placeholder so it lives as long
            // as the ring does — events for the soon-to-be-attached sessions
            // need it running before the attach Ack returns.
            self.start_server_pump(cx);
            if let Some(slot) = ring.slots.first_mut() {
                slot.pending_open_token = Some(open_token);
            }

            self.set_screen(App::Agent(ring));
            if let Some(c) = self.agent_mut() {
                c.editor.begin_insert();
            }
            cx.notify();

            self.spawn_open_agent_server(open_token, proc_cwd, cx);
            return;
        } else {
            // ── Direct-spawn path (legacy) ───────────────────────────
            let persisted = load_persisted_acp_sessions(&proc_cwd);

            if persisted.is_empty() {
                let slot_cwd = proc_cwd.clone();
                let session_index = ring.next_index;
                let state = self.create_agent_session(None, slot_cwd.clone(), session_index, cx);
                ring.push("claude-1".into(), state, None, slot_cwd, None);
            } else {
                let active_pos = persisted.iter().position(|s| s.active).unwrap_or(0);
                for slot in persisted {
                    let slot_cwd = slot.cwd.clone().unwrap_or_else(|| proc_cwd.clone());
                    let session_index = ring.next_index;
                    let mut state = self.create_agent_session(
                        Some(slot.id.clone()),
                        slot_cwd.clone(),
                        session_index,
                        cx,
                    );
                    if slot.mode == InputModeKind::Worksheet {
                        state.input_surface = InputSurface::Worksheet;
                    }
                    state.tasklist_open = slot.tasklist_open;
                    state.subagents_open = slot.subagents_open;
                    ring.push(slot.label, state, Some(slot.id), slot_cwd, None);
                }
                ring.active = active_pos.min(ring.slots.len().saturating_sub(1));
            }
        }

        self.set_screen(App::Agent(ring));

        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
        }
        cx.notify();
    }

    /// Background half of `open_agent_inner`'s session-server path (S4). Runs
    /// `list_sessions` and the resulting `attach`/`create_session` round-trips
    /// off the paint thread, then splices the real slot(s) into the
    /// placeholder ring via `this.update`. `placeholder_index` identifies the
    /// "connecting…" slot to fill in place (it owns the pump task, so we
    /// mutate it rather than replace it). If the window/ring is gone by the
    /// time the result lands (weak entity dropped, screen switched), every
    /// `this.update` is a no-op and the work is harmlessly discarded.
    pub(crate) fn spawn_open_agent_server(
        &self,
        open_token: u64,
        proc_cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.session_server.as_ref().map(|s| s.handle()) else {
            return;
        };
        // Snapshot the server sids already open in any tile so the background
        // thread can dedup without touching `self`. Taken now, while we're
        // still on the (single-threaded) UI thread, so it can't race a
        // concurrent ring mutation. (Attach — and thus the Owner/Observer mode
        // choice — is deferred to `spawn_attach_sessions` after the bind.)
        let mut open_sids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tab in self.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |w| {
                if let App::Agent(ring) = &w.content {
                    for slot in ring.slots.iter() {
                        if let Some(sid) = &slot.server_session_id {
                            open_sids.insert(sid.clone());
                        }
                    }
                }
            });
        }

        cx.spawn(async move |this, cx| {
            let cwd = proc_cwd.clone();
            let resolution = cx
                .background_executor()
                .spawn(async move {
                    // 1. Discover existing sessions for this cwd that aren't
                    //    already shown elsewhere.
                    let existing = match handle.list_sessions() {
                        Ok(v) => v,
                        Err(e) => return OpenResolution::Failed(format!("list failed: {e}")),
                    };
                    let cwd_key = cwd_match_key(&cwd);
                    let matching: Vec<SessionInfo> = existing
                        .into_iter()
                        .filter(|s| cwd_match_key(&s.cwd) == cwd_key)
                        .filter(|s| !open_sids.contains(&s.session_id))
                        .collect();

                    if matching.is_empty() {
                        // 2a. None — create a fresh session. The server
                        //     registers it and returns the sid immediately
                        //     (ACP subprocess spawns server-side). NOTE: we do
                        //     NOT attach here. Attaching starts the server's
                        //     event replay, and the slot's `server_session_id`
                        //     isn't bound until `apply_open_agent_resolution`
                        //     runs on the foreground — attaching first races
                        //     that bind and the pump drops the replay (the
                        //     "resumed session is wonky/empty" bug). Attach is
                        //     deferred to after the bind; see `spawn_attach_sessions`.
                        match handle.create_session(cwd, "claude-1".to_string(), None) {
                            Ok(info) => OpenResolution::Created {
                                sid: info.session_id,
                                acp_id: info.acp_session_id,
                                permission_mode: info.permission_mode,
                            },
                            Err(e) => OpenResolution::Failed(format!("create failed: {e}")),
                        }
                    } else {
                        // 2b. Resume each matching session — bind first, attach
                        //     later. Same rationale as 2a: deferring the attach
                        //     until the slot is bound closes the replay-drop
                        //     race. Owner reclaim + status come from the
                        //     deferred `spawn_attach_sessions`.
                        let attached: Vec<AttachedSlot> = matching
                            .iter()
                            .enumerate()
                            .map(|(i, info)| AttachedSlot {
                                label: if matching.len() == 1 {
                                    "claude-1".to_string()
                                } else {
                                    format!("claude-{}", i + 1)
                                },
                                sid: info.session_id.clone(),
                                acp_id: info.acp_session_id.clone(),
                                status: if info.connected {
                                    "reconnecting…".to_string()
                                } else {
                                    "reconnecting (agent spawning…)".to_string()
                                },
                                permission_mode: info.permission_mode,
                            })
                            .collect();
                        OpenResolution::Attached(attached)
                    }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.apply_open_agent_resolution(open_token, resolution, cx);
            });
        })
        .detach();
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
        // Bind back to the exact placeholder that started this open, searching
        // the WHOLE workspace (not just the focused ring) and matching the
        // globally-unique `open_token` (not the per-ring `index`, which
        // collides at 0 across rings — the cause of `pump: no slot for server
        // session`). If the placeholder is gone (screen closed before the
        // round-trip returned), this is a harmless no-op.
        // Sids whose slot we actually bound in this pass. Collected inside the
        // ring closure (which only runs if the placeholder still exists) so we
        // attach EXACTLY the sessions now routable — attaching a sid whose slot
        // is gone would resurrect the replay-drop race we are fixing.
        let bound_sids: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
        let bound_sids_c = bound_sids.clone();
        self.with_open_token_ring(open_token, move |ring| {
            let Some(pos) = ring
                .slots
                .iter()
                .position(|s| s.pending_open_token == Some(open_token))
            else {
                return;
            };
            let proc_cwd = ring.slots[pos].cwd.clone();
            // Consume the token regardless of outcome so a late duplicate
            // resolution can't re-bind this slot.
            ring.slots[pos].pending_open_token = None;

            match resolution {
                OpenResolution::Failed(msg) => {
                    let m = format!("session server error — {msg}");
                    Self::append_system_notice(&mut ring.slots[pos].state, &m);
                    ring.slots[pos].state.status = Some(m.into());
                }
                OpenResolution::Created {
                    sid,
                    acp_id,
                    permission_mode,
                } => {
                    let slot = &mut ring.slots[pos];
                    slot.server_session_id = Some(sid.clone());
                    slot.resume_id = acp_id;
                    slot.state.permission_mode = permission_mode;
                    slot.state.status = Some("attaching to ACP agent via session server…".into());
                    bound_sids_c.borrow_mut().push(sid);
                }
                OpenResolution::Attached(attached) => {
                    let mut iter = attached.into_iter();
                    // First attached session fills the placeholder in place.
                    if let Some(first) = iter.next() {
                        let slot = &mut ring.slots[pos];
                        slot.label = first.label;
                        slot.server_session_id = Some(first.sid.clone());
                        slot.resume_id = first.acp_id;
                        slot.state.permission_mode = first.permission_mode;
                        slot.state.status = Some(first.status.into());
                        bound_sids_c.borrow_mut().push(first.sid);
                    }
                    // Remaining sessions get their own slots in the same ring.
                    for a in iter {
                        let mut state = AgentState::new_server_managed(Some(a.status.into()));
                        state.permission_mode = a.permission_mode;
                        ring.push(
                            a.label,
                            state,
                            a.acp_id,
                            proc_cwd.clone(),
                            Some(a.sid.clone()),
                        );
                        bound_sids_c.borrow_mut().push(a.sid);
                    }
                    // Land the user on the placeholder slot, not the last push.
                    ring.active = pos;
                }
            }
        });
        self.save_agent_ring();
        cx.notify();

        // Now that the slots carry their `server_session_id`, attach (which
        // starts the server's event replay). Routing can no longer drop the
        // replay because every target is already bound. Deferred off the paint
        // thread; surfaces ownership/attach failures into the slot status.
        let targets = std::rc::Rc::try_unwrap(bound_sids)
            .map(|c| c.into_inner())
            .unwrap_or_default();
        if !targets.is_empty() {
            self.spawn_attach_sessions(targets, cx);
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
                    // Per-slot outcome: the status string to surface (if any) and
                    // whether THIS window now drives the session. `is_driver` is
                    // the attach response's `driver` flag — the single source of
                    // truth the lease heartbeat (beat only drivers) and the
                    // no-poll-acquire rule (observers don't re-attach Owner on a
                    // heartbeat error) both read.
                    let (status, is_driver): (Option<SharedString>, bool) = match r {
                        // Granted drive rights (Owner): leave the optimistic
                        // "reconnecting…"/"attaching…" status to be overwritten
                        // by the first real event / SessionAttached notice.
                        Ok(true) => (None, true),
                        // Downgraded to Observer despite wanting Owner: a
                        // different live client holds the lease. Surface
                        // read-only and DO NOT drive.
                        Ok(false) if want_owner => (
                            Some("read-only — another window owns this session".into()),
                            false,
                        ),
                        // Observer by design (candidate / explicit observe).
                        Ok(false) => (None, false),
                        Err(e) => {
                            eprintln!(
                                "[sketch-gpui] attach failed for {}: {e}",
                                &sid[..sid.len().min(8)]
                            );
                            // The server answers `no such session: <id>` for a
                            // lookup miss (sketch-session-server actor) — the
                            // persisted id outlived the server's WAL. That's
                            // PERMANENT: drop the dead slot rather than churn a
                            // broken one. Anything else (disconnected, write/
                            // read failure) is TRANSIENT and may recover on
                            // reconnect, so keep the status and the slot.
                            if is_session_gone_error(&e) {
                                dead_sids.push(sid.clone());
                                (None, false)
                            } else {
                                (
                                    Some("attach failed — session may be unavailable".into()),
                                    false,
                                )
                            }
                        }
                    };
                    this.for_each_server_session_slot(&sid, |slot| {
                        slot.is_driver = is_driver;
                        if let Some(s) = status.clone() {
                            slot.state.status = Some(s);
                        }
                    });
                }
                // Drop dead slots via the same path the server's SessionClosed
                // broadcast uses: `reconcile_session_closed` finds the slot in
                // any tab/tile, removes it with `close_at` (which fixes the
                // ring's active index and never panics on the last slot), and
                // restores the underlying screen if the ring empties — so no
                // tile is ever left holding an empty ring. After removal,
                // re-save every tile's ring (keyed by the process cwd, exactly
                // like close_active_agent_session) so the stale id doesn't
                // return on the next launch.
                let mut dropped_any = false;
                for sid in &dead_sids {
                    if this.reconcile_session_closed(sid) {
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

    /// Run `f` on the agent ring holding the placeholder slot stamped with
    /// `token` (see `AgentSlot::pending_open_token`), searching every tab and
    /// tile. Returns whether a match was found. Lets an async server
    /// open/create bind back to its originating slot regardless of which
    /// window happens to be focused when the round-trip returns.
    pub(crate) fn with_open_token_ring(
        &mut self,
        token: u64,
        f: impl FnOnce(&mut AgentRing),
    ) -> bool {
        let mut f = Some(f);
        for tab in self.workspace.tabs.iter_mut() {
            let found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content
                    && ring
                        .slots
                        .iter()
                        .any(|s| s.pending_open_token == Some(token))
                {
                    if let Some(f) = f.take() {
                        f(ring);
                    }
                    return Some(());
                }
                None
            });
            if found.is_some() {
                return true;
            }
        }
        false
    }

    /// Create a new session and add it to the existing ring. With `cwd =
    /// None`, the new slot inherits the process cwd (today's behavior). With
    /// `cwd = Some(path)`, that already-resolved absolute path becomes the
    /// new slot's cwd — the caller (typically the `:claude-new <path>`
    /// command handler) is responsible for running the input through
    /// `resolve_agent_cwd_arg` first.
    pub(crate) fn new_agent_session(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        let (label, session_index) = match self.agent_ring() {
            Some(r) => (format!("claude-{}", r.next_index + 1), r.next_index),
            None => {
                // Not on the Agent screen yet — bootstrap it AND create a
                // brand-new session. We deliberately do NOT route through
                // `open_agent_inner` here: its server path runs
                // `spawn_open_agent_server`, which `list_sessions` and
                // re-attaches an existing per-cwd session instead of creating
                // a fresh one. That made the very first "new session" resume
                // the prior session and only the *second* invocation create
                // fresh (the bug). `bootstrap_fresh_agent_session` mirrors the
                // screen setup but always creates (server path) / always
                // spawns fresh (direct path).
                self.bootstrap_fresh_agent_session(cwd, cx);
                return;
            }
        };
        let slot_cwd = cwd.unwrap_or_else(process_cwd);

        if self.session_server.is_some() {
            // Session-server path (S4: non-blocking). Push a "connecting…"
            // placeholder immediately and create the session off-thread; the
            // sid is spliced in when the round-trip returns.
            let placeholder =
                AgentState::new_server_managed(Some("connecting to session server…".into()));
            let open_token = alloc_open_token();
            let ring = self.agent_ring_mut().unwrap();
            ring.push(label.clone(), placeholder, None, slot_cwd.clone(), None);
            if let Some(slot) = ring.slots.last_mut() {
                slot.pending_open_token = Some(open_token);
            }
            self.spawn_create_agent_session(open_token, label, slot_cwd, cx);
        } else {
            // Direct-spawn path.
            let state = self.create_agent_session(None, slot_cwd.clone(), session_index, cx);
            let ring = self.agent_ring_mut().unwrap();
            ring.push(label, state, None, slot_cwd, None);
        }
        // §18 soft cap: at 6+ slots, surface a one-shot footer warning so
        // the user notices the per-slot ~100MB subprocess cost. Advisory
        // only — no enforcement.
        let count = self.agent_ring().map(|r| r.len()).unwrap_or(0);
        if let Some(c) = self.agent_mut() {
            c.editor.begin_insert();
            if count >= 6 {
                c.status = Some(format!("{count} sessions active — each uses ~100MB").into());
            }
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
        // Stash the current screen so back_to_doc can restore it (mirrors
        // open_agent_inner).
        let prior = self
            .workspace
            .replace_focused_content(App::Buffer(BufferApp::Viewing(DocState {
                blocks: Vec::new(),
                file_label: SharedString::new_static(""),
                cursor_block: 0,
                list_state: DocState::new_list_state(0),
                list_item_count: std::cell::Cell::new(0),
                blocks_seq: 0,
                blocks_snapshot: RefCell::new(None),
                last_cursor_block: std::cell::Cell::new(None),
                source: None,
            })))
            .expect("workspace has no focused window");

        let mut ring = AgentRing::new(Some(Box::new(prior)));
        let slot_cwd = cwd.unwrap_or_else(process_cwd);
        let label = "claude-1".to_string();

        if self.session_server.is_some() {
            // Server path: placeholder + create-only round-trip (NO resolve /
            // reattach — that is the whole point of "fresh").
            let placeholder =
                AgentState::new_server_managed(Some("connecting to session server…".into()));
            let open_token = alloc_open_token();
            ring.push(label.clone(), placeholder, None, slot_cwd.clone(), None);
            self.start_server_pump(cx);
            if let Some(slot) = ring.slots.first_mut() {
                slot.pending_open_token = Some(open_token);
            }
            self.set_screen(App::Agent(ring));
            if let Some(c) = self.agent_mut() {
                c.editor.begin_insert();
            }
            cx.notify();
            self.spawn_create_agent_session(open_token, label, slot_cwd, cx);
        } else {
            // Direct-spawn path: a fresh session has no resume_id.
            let session_index = ring.next_index;
            let state = self.create_agent_session(None, slot_cwd.clone(), session_index, cx);
            ring.push(label, state, None, slot_cwd, None);
            self.set_screen(App::Agent(ring));
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
        slot_index: usize,
        new_cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        // Resolve slot position once; the index is monotonic so it
        // doesn't shift unless the slot was closed.
        let pos = match self.agent_ring().and_then(|r| r.slot_by_index(slot_index)) {
            Some(p) => p,
            None => return,
        };

        // Phase 1: tear down the existing channel + attach state. The
        // borrow ends before we cross-call create_agent_session.
        let prev_cwd = {
            let ring = self.agent_ring_mut().unwrap();
            let slot = &mut ring.slots[pos];
            let prev = slot.cwd.clone();
            // Dropping `channel` kills the subprocess via kill_on_drop.
            slot.state.channel = None;
            slot.state.attach_pending = None;
            slot.state.turn_phase = TurnPhase::Idle;
            let msg = format!("changing cwd to {}…", shorten_cwd_for_display(&new_cwd),);
            Self::append_system_notice(&mut slot.state, &msg);
            slot.state.status = Some(msg.into());
            slot.cwd = new_cwd.clone();
            // The agent-side session was bound to the old cwd; a fresh
            // session/new is the right resume strategy.
            slot.resume_id = None;
            prev
        };

        // Phase 2: build a fresh agent session at the new cwd.
        if self.session_server.is_some() {
            // Server path (S4: non-blocking): take the old sid, fire its close
            // off-thread, mark the slot "connecting…", and create the new
            // session off-thread. `spawn_create_agent_session` splices the new
            // sid into this slot (by its monotonic `slot_index`) when ready.
            let old_sid = {
                if let Some(ring) = self.agent_ring_mut() {
                    ring.slots
                        .get_mut(pos)
                        .and_then(|s| s.server_session_id.take())
                } else {
                    None
                }
            };
            if let Some(old_sid) = old_sid {
                self.spawn_close_session(old_sid, cx);
            }
            let open_token = alloc_open_token();
            if let Some(ring) = self.agent_ring_mut()
                && let Some(slot) = ring.slots.get_mut(pos)
            {
                slot.state.attach_pending = None;
                slot.state.channel = None;
                slot.pending_open_token = Some(open_token);
                let msg = format!(
                    "cwd → {}, connecting to fresh session…",
                    shorten_cwd_for_display(&new_cwd),
                );
                Self::append_system_notice(&mut slot.state, &msg);
                slot.state.status = Some(msg.into());
            }
            self.spawn_create_agent_session(
                open_token,
                "respawned".to_string(),
                new_cwd.clone(),
                cx,
            );
        } else {
            // Direct-spawn path: graft a throwaway AgentState's
            // channel + pump into the existing slot.
            let fresh = self.create_agent_session(None, new_cwd.clone(), slot_index, cx);
            if let Some(ring) = self.agent_ring_mut()
                && let Some(slot) = ring.slots.get_mut(pos)
            {
                slot.state.attach_pending = fresh.attach_pending;
                slot.state._pump = fresh._pump;
                let msg = format!("cwd → {}, fresh session", shorten_cwd_for_display(&new_cwd),);
                Self::append_system_notice(&mut slot.state, &msg);
                slot.state.status = Some(msg.into());
            }
        }

        let _ = prev_cwd;
        self.save_agent_ring();
        cx.notify();
    }

    /// Switch to the next (+1) or previous (-1) session in the ring.
    pub(crate) fn switch_agent_session(&mut self, direction: i32, cx: &mut Context<Self>) {
        if let Some(ring) = self.agent_ring_mut() {
            if direction > 0 {
                ring.next();
            } else {
                ring.prev();
            }
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Close the active session. If the ring is now empty, exit Claude.
    pub(crate) fn close_active_agent_session(&mut self, cx: &mut Context<Self>) {
        // For server sessions: drop the slot locally NOW (optimistic) and fire
        // the close round-trip off the paint thread (S4). `close_session`
        // parks on a 30s `recv_timeout`, so doing it synchronously froze the
        // window when the server stalled. The server broadcasts `SessionClosed`
        // on success, which `reconcile_session_closed` already folds into every
        // tile — so the worst case of an off-thread close that ends up not
        // landing is a stale entry that the next open's dedup/reconnect path
        // cleans up, not a frozen UI.
        let server_sid = self
            .agent_ring()
            .filter(|r| !r.is_empty())
            .and_then(|r| r.active().server_session_id.clone());

        if let Some(sid) = server_sid {
            self.spawn_close_session(sid, cx);
        }

        let is_empty = {
            let ring = match self.agent_ring_mut() {
                Some(r) => r,
                None => return,
            };
            let _dropped = ring.close_active(); // AgentSlot drops → pump task cancelled
            ring.is_empty()
        };
        if is_empty {
            // Last slot closed: wipe the cwd entry so reboot doesn't
            // resurrect anything, then drop the Claude screen.
            if let Ok(cwd) = std::env::current_dir() {
                forget_persisted_acp_sessions(&cwd);
            }
            self.back_to_doc(cx);
        } else {
            self.save_agent_ring();
            cx.notify();
        }
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
                            "[sketch-gpui] close_session({}) failed (connection): {e}",
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

    /// Snapshot the current ring to disk. Called after every ring mutation
    /// (new/close/switch) and from the pump after a slot's attach resolves.
    /// Best-effort: any failure to write is silently ignored.
    pub(crate) fn save_agent_ring(&self) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        // Save agent rings from ALL tiles, not just the focused one.
        if let Some(tab) = self.workspace.active_tab() {
            tab.layout.for_each_leaf(&mut |window| {
                if let App::Agent(ring) = &window.content {
                    save_persisted_acp_sessions(&cwd, ring);
                }
            });
        }
    }

    /// Build a `AgentState` with ACP attach thread and pump task. The
    /// returned state is ready to be pushed into a `AgentRing`. `cwd` is
    /// the per-session working directory (spec-agent-cwd.md §3) — both the
    /// `NewSessionRequest` payload and the OS-level subprocess cwd come
    /// from this single argument. `session_index` is the monotonic
    /// `AgentSlot::index` the pump task will use to find this slot every
    /// tick; callers MUST pass the value that `AgentRing::push` will (or
    /// did) assign to this slot. Passing the wrong value silently strands
    /// the slot's attach (the pump drains some other slot's
    /// `attach_pending` and this slot's channel stays `None` forever).
    pub(crate) fn create_agent_session(
        &mut self,
        resume_id: Option<String>,
        cwd: PathBuf,
        session_index: usize,
        cx: &mut Context<Self>,
    ) -> AgentState {
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let spawn_cwd = Some(cwd);
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    spawn_cwd,
                    resume_id,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        let editor = Editor::new(String::new(), PathBuf::from("*claude*"));

        let pump = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;
            let idle_delay = Duration::from_millis(16);
            let yield_delay = Duration::from_millis(1);
            let min_cycle = Duration::from_millis(16);
            // Local throttle for the thinking-indicator animation: while a
            // turn is in flight we re-render at ~8fps even without events so
            // the elapsed/quiet timers stay live through a stall. Kept local
            // so the idle path doesn't grab the model lock every 16ms.
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
                        if let Some(ring) = this.agent_ring_mut()
                            && let Some(slot) = ring.slot_by_index_mut(session_index)
                            && let Some(ch) = &slot.state.channel
                        {
                            wake_rx = ch.take_wake_receiver();
                        }
                    });
                }
                loop {
                    let t_apply = perf_enabled().then(std::time::Instant::now);
                    let more =
                        match this.update(cx, |this, cx| this.pump_session(session_index, cx)) {
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
                // Animation heartbeat: keep the thinking timer ticking even
                // when no events arrived this cycle.
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

        let state = AgentState {
            editor,
            channel: None,
            attach_pending: Some(attach_rx),
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            status: Some("attaching to ACP agent…".into()),
            turn_phase: TurnPhase::Idle,
            replay_turns: sketch::acp_channel::ReplayTurns::default(),
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
            permission_mode: sketch::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: false,
            reconciler: sketch::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            generation: 0,
            finalized: std::collections::HashSet::new(),
            replay_prefix_finalized: false,
            agent_stream_authoritative: false,
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: Some(pump),
        };
        setup_list_follow_handler(&state.list_state, &state.follow_output);
        state
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
                eprintln!("[sketch-gpui] session-server reconnect failed: {e}");
                return None;
            }
        };

        // Reset every server-backed slot's transcript and collect the sids to
        // re-attach. (Borrow of `session_server` above has ended.)
        let mut sids: Vec<String> = Vec::new();
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content {
                    for slot in ring.slots.iter_mut() {
                        if let Some(sid) = slot.server_session_id.clone() {
                            slot.state.reset_for_replay();
                            Self::append_system_notice(&mut slot.state, "reconnecting…");
                            slot.state.status = Some("reconnecting…".into());
                            sids.push(sid);
                        }
                    }
                }
            });
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
        eprintln!("[sketch-gpui] session-server reconnected; re-attaching {n} session(s)");
        Some((note_rx, wake_rx))
    }

    /// Unified pump task for the session server path. Drains all
    /// notifications from `SessionServerClient::try_recv()` and routes
    /// them to the correct `AgentSlot` by `server_session_id`. Runs as a
    /// single GPUI background task per view (not per-slot).
    /// Long-lived lease-heartbeat driver (spec phase 4). Every
    /// `HEARTBEAT_INTERVAL` it collects the server sessions THIS GUI currently
    /// drives (a candidate/observer drives none) and sends `Heartbeat` for each
    /// so the server pushes the lease expiry forward. On a Heartbeat `Err`
    /// (lease lost — expired and re-taken, or demoted) it re-attaches that sid
    /// (resumes-or-observes), which also refreshes the slot's role. The task
    /// self-cancels when the client disconnects or the view is dropped.
    ///
    /// SINGLETON per view: the beater is stored in `self._lease_heartbeat` and
    /// spawned at most ONCE for the window's lifetime. `start_server_pump` runs
    /// at every "open a fresh Claude screen" site, but only the first call (the
    /// one that finds the field `None`) spawns a beater; later calls are
    /// no-ops. One beater is correct and sufficient because the loop depends on
    /// NO per-open state — it self-gates per-tick on `slot.is_driver` and so
    /// covers every driven session this window has, including ones opened after
    /// it started. (A non-singleton, detached-per-call design would leave K
    /// concurrent beaters after K opens, and a single lease-loss would fan out
    /// into K redundant same-client_id Owner re-attaches.) Cancelled when the
    /// view (and thus the stored `Task`) is dropped.
    pub(crate) fn start_lease_heartbeat(&mut self, cx: &mut Context<Self>) {
        // Singleton guard: a beater already rides this window's lifetime, so the
        // 2nd+ `start_server_pump` call (re-opening the Claude screen) must not
        // spawn another. The existing beater already covers any newly-opened
        // driven session via its per-tick `is_driver` rescan.
        if self._lease_heartbeat.is_some() {
            return;
        }
        // Spawned UNCONDITIONALLY (no `is_candidate` early-return). The loop
        // self-gates per-iteration: each tick it collects ONLY the sessions
        // this window actually drives (`slot.is_driver`). A candidate drives
        // nothing, so it simply beats no one until it promotes — and once
        // `candidate_take_over` flips `is_driver=true` on its slots, the very
        // next tick begins beating them automatically, with no need to restart
        // the beater. This closes the owner-gap-after-promote race where a
        // freshly-promoted owner held the lease but never heartbeat it.
        const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
        let beater = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(HEARTBEAT_INTERVAL).await;

                // Collect the handle + the sids this GUI DRIVES this tick. Only
                // driver slots (`is_driver`) are beaten: a window downgraded to
                // Observer at attach must never beat (its heartbeat would hit
                // the server's non-holder branch and churn re-attaches). Bail if
                // there's no server (view dropped / server disabled).
                let collected = this.update(cx, |this, _cx| {
                    let handle = this.session_server.as_ref().map(|s| s.handle());
                    let mut sids: Vec<String> = Vec::new();
                    for tab in this.workspace.tabs.iter_mut() {
                        tab.layout.for_each_leaf_content_mut(&mut |content| {
                            if let App::Agent(ring) = content {
                                for slot in ring.slots.iter() {
                                    if !slot.is_driver {
                                        continue;
                                    }
                                    if let Some(sid) = slot.server_session_id.clone() {
                                        sids.push(sid);
                                    }
                                }
                            }
                        });
                    }
                    (handle, sids)
                });
                let (handle, sids) = match collected {
                    Ok((Some(handle), sids)) if !sids.is_empty() => (handle, sids),
                    Ok(_) => continue, // no server or nothing to beat
                    Err(_) => break,   // view dropped: stop the beater
                };
                if !handle.is_connected() {
                    continue; // pump's reconnect path will re-attach
                }

                // Beat each driven session off the paint thread. Collect the
                // ones whose lease was lost so we can re-attach them.
                let lost: Vec<String> = cx
                    .background_executor()
                    .spawn(async move {
                        sids.into_iter()
                            .filter(|sid| handle.heartbeat(sid).is_err())
                            .collect()
                    })
                    .await;

                if !lost.is_empty() {
                    // Only GENUINE drivers reach this branch (the collect above
                    // beats `is_driver` slots exclusively), so a lost sid was a
                    // real owner whose own lease lapsed — re-attaching as Owner
                    // is a same-client_id DETERMINISTIC reclaim, not a poll-
                    // acquire steal: the server grants it only if the lease is
                    // free or already ours. A window downgraded to Observer at
                    // attach never beats, so it can never reach here and never
                    // re-attaches Owner from a heartbeat error; it regains
                    // ownership only via an explicit LeaseChanged{None} promote.
                    // spawn_attach_sessions re-stamps `is_driver` from the new
                    // outcome, so if the reclaim downgrades us we stop beating.
                    let _ = this.update(cx, |this, cx| {
                        this.spawn_attach_sessions(lost, cx);
                    });
                }
            }
        });
        // Store the singleton beater on the view so it lives for the window's
        // lifetime and is cancelled (dropped) with the view.
        self._lease_heartbeat = Some(beater);
    }

    pub(crate) fn start_server_pump(&mut self, cx: &mut Context<Self>) {
        // Singleton guard, same as the heartbeat: one pump per view, alive
        // for the view's lifetime. Re-entry (every open/new/restore path
        // calls this defensively) is a no-op; the receivers stay owned by
        // the original task.
        if self._server_pump.is_some() {
            return;
        }
        // Phase 4: a single lease-heartbeat beater rides alongside the pump so a
        // live owner's lease never falsely expires.
        self.start_lease_heartbeat(cx);
        let task = cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            use futures::stream::StreamExt;

            // Take exclusive ownership of the notification + wake receivers
            // once (Phase 2 of spec-pump-fix-synthesis.md). Channel reads
            // need no `&mut SketchGpuiView`, so the old pattern of grabbing
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
                                    "[sketch-gpui] reconnected to session server \
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
    pub(crate) fn with_server_session_slot(
        &mut self,
        sid: &str,
        mut f: impl FnMut(&mut AgentSlot),
    ) -> bool {
        for tab in self.workspace.tabs.iter_mut() {
            let found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content
                    && let Some(slot) = ring.slot_by_server_session_id_mut(sid)
                {
                    f(slot);
                    return Some(());
                }
                None
            });
            if found.is_some() {
                return true;
            }
        }
        false
    }

    /// Run `f` on the slot for `sid` in *every* tile that has one (unlike
    /// [`with_server_session_slot`], which stops at the first match). A session
    /// observed in multiple tiles must fan its events out to all of them.
    /// Returns the number of slots visited.
    pub(crate) fn for_each_server_session_slot(
        &mut self,
        sid: &str,
        mut f: impl FnMut(&mut AgentSlot),
    ) -> usize {
        let mut count = 0;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content
                    && let Some(slot) = ring.slot_by_server_session_id_mut(sid)
                {
                    f(slot);
                    count += 1;
                }
            });
        }
        count
    }

    /// Reconcile a server-side close into the local model: drop the slot for
    /// `sid` from every tile's ring. A ring left empty is replaced in place
    /// with its stashed underlying screen (or a fresh browser) so no tile is
    /// ever left holding an empty `AgentRing`, which would panic on render.
    /// Returns whether anything changed.
    pub(crate) fn reconcile_session_closed(&mut self, sid: &str) -> bool {
        let mut changed = false;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                // Compute the replacement (if the ring empties) *before*
                // reassigning, so the `ring` borrow ends first.
                let restore: Option<Option<App>> =
                    if let App::Agent(ring) = content {
                        if let Some(pos) = ring.position_by_server_session_id(sid) {
                            ring.close_at(pos);
                            changed = true;
                            if ring.is_empty() {
                                Some(ring.underlying.take().map(|b| *b))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                if let Some(under) = restore {
                    *content = under.unwrap_or_else(|| {
                        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        )))
                    });
                }
            });
        }
        changed
    }

    /// Reconcile a server-side rename: update the label on the matching slot in
    /// every tile. Returns whether anything changed.
    pub(crate) fn reconcile_session_renamed(&mut self, sid: &str, label: &str) -> bool {
        let mut changed = false;
        self.for_each_server_session_slot(sid, |slot| {
            if slot.label != label {
                slot.label = label.to_string();
                changed = true;
            }
        });
        changed
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
                    "[sketch-gpui] pump: no slot for server session {}",
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
                                sketch::agent_event::AgentEventKind::TurnEnded { .. }
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
                            sketch::agent_transcript::UserTurnOrigin::Echo,
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
                    // another GUI instance). Drop its slot from every tile so
                    // the lists stay consistent.
                    self.reconcile_session_closed(&session_id);
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

    /// Pump a specific session by its monotonic index. Returns `true` if
    /// the per-tick budget was hit and more events may be queued. Returns
    /// `false` when the session is gone (pump task should exit) or the
    /// queue is drained.
    pub(crate) fn pump_session(&mut self, session_index: usize, cx: &mut Context<Self>) -> bool {
        const PUMP_EVENT_BUDGET: usize = 64;

        // Scoped borrow: all mutable access to the ring/slot/claude happens
        // inside this block. Returns (has_events, more_pending, attached_with_id)
        // so post-borrow work (persistence) can proceed.
        //
        // Search ALL tiles (not just the focused one) so that agent sessions
        // in unfocused split tiles keep pumping events.
        let (has_events, more_pending, attached_with_id) = {
            // Find the slot across every tile in EVERY tab (not just the
            // active tab) so agent sessions in background tabs and unfocused
            // split tiles keep pumping events.
            let mut found = None;
            for tab in self.workspace.tabs.iter_mut() {
                found = tab.layout.find_map_leaf_content_mut(&mut |content| {
                    if let App::Agent(ring) = content
                        && let Some(slot) = ring.slot_by_index_mut(session_index)
                    {
                        // SAFETY: pointer is valid for the scoped-borrow
                        // block below — we don't structurally mutate the
                        // layout.
                        let ptr = &mut slot.state as *mut AgentState;
                        return Some(ptr);
                    }
                    None
                });
                if found.is_some() {
                    break;
                }
            }
            let state_ptr = match found {
                Some(f) => f,
                None => return false,
            };
            // SAFETY: the layout isn't mutated during this block; the
            // pointer remains valid until the scoped borrow ends.
            let claude = unsafe { &mut *state_ptr };

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
                        let msg = format!("attach failed: {e} (set SKETCH_ACP_AGENT=...?)");
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
            let mut events: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
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
                    let mut tail: Vec<sketch::acp_channel::ReplyEvent> = Vec::new();
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
    /// Splice a sketch-local lifecycle notice into the transcript. Tagged
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
        events: Vec<sketch::acp_channel::ReplyEvent>,
    ) -> bool {
        use sketch::acp_channel::ReplyEvent;
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
                    if let Some(existing) = claude.tools.calls.get_mut(&id) {
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
                        let mut tc = sketch::acp_channel::ToolCall::new(
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
                        sketch::agent_transcript::UserTurnOrigin::Echo,
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
                    // here when SKETCH_EMIT_TURN_ENDED=1.
                    eprintln!(
                        "[sketch-gpui] explicit TurnEnded count={count}; \
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
        event: &sketch::agent_event::AgentEvent,
    ) -> AgentEventEffect {
        use sketch::agent_event::{AgentEventKind, ChunkRole, TurnOutcome};

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
                if let Some(existing) = claude.tools.calls.get_mut(&id) {
                    existing.update(upd.fields.clone());
                    cap_tool_call_payloads(existing);
                } else {
                    let mut tc =
                        sketch::acp_channel::ToolCall::new(upd.tool_call_id.clone(), String::new());
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
                    sketch::agent_transcript::UserTurnOrigin::Echo,
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
    pub(crate) fn log_unknown_agent_event_once(tag: &str, event: &sketch::agent_event::AgentEvent) {
        thread_local! {
            static SEEN: RefCell<std::collections::HashSet<String>> =
                RefCell::new(std::collections::HashSet::new());
        }
        let first = SEEN.with(|s| s.borrow_mut().insert(tag.to_string()));
        if first {
            eprintln!(
                "[sketch-gpui] agent-stream: ignoring unknown event kind {tag:?} \
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
    /// without restarting sketch.
    pub(crate) fn clear_agent_session(&mut self, cx: &mut Context<Self>) {
        // Forget every persisted slot BEFORE re-opening so the new spawn
        // hits session/new instead of session/load. Done first so even
        // if open_agent_inner panics partway through, the next manual
        // attach won't accidentally resume any cleared session.
        if let Ok(cwd) = std::env::current_dir() {
            forget_persisted_acp_sessions(&cwd);
        }
        // Drop the current claude screen entirely; open_agent_inner
        // builds a new one. We don't try to surgically reset fields on
        // the existing AgentState because the underlying screen
        // (browser/doc) is also stashed there — preserving it is the
        // job of open_agent_inner via the prior-screen swap dance.
        if matches!(
            self.workspace.focused_content().expect("no focused window"),
            App::Agent(_)
        ) {
            // Restore underlying first so open_agent_inner can capture
            // it as the new "prior" screen. Otherwise we'd lose the
            // file/browser the user was viewing before they opened
            // claude.
            self.back_to_doc(cx);
        }
        self.open_agent_inner(cx);
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
            .agent_ring()
            .and_then(|r| r.slots.get(r.active))
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
        if let Some(c) = self.agent_mut()
            && (c.channel.is_some() || c.attach_pending.is_some())
        {
            c.status = Some("session is already attached".into());
            cx.notify();
            return;
        }

        // Use the active slot's per-session cwd (spec-agent-cwd.md §3)
        // rather than the process cwd, so a slot that lives at /foo
        // re-attaches at /foo and not at sketch's launch directory.
        let slot_cwd = match self.agent_ring() {
            Some(r) => Some(r.active().cwd.clone()),
            None => return,
        };
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-attach".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    None,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });

        if let Some(ring) = self.agent_ring_mut() {
            ring.active_mut().resume_id = None;
            let claude = &mut ring.active_mut().state;
            claude.attach_pending = Some(attach_rx);
            Self::append_system_notice(claude, "attaching new session…");
            claude.status = Some("attaching new session…".into());
        }
        self.save_agent_ring();
        cx.notify();
    }

    /// Quit-and-relaunch sketch with the auto-open-claude flag set, so the
    /// new process boots straight into the claude screen and restores every
    /// session that was in the ring at quit time via
    /// `load_persisted_acp_sessions` plus per-slot `spawn_with_resume`.
    /// Designed for "I broke something in sketch and want to keep iterating
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
        cmd.env("SKETCH_OPEN_CLAUDE", "1");
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
        let mut awaiting = false;
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content
                    && ring.slots.iter().any(|s| s.state.turn_phase.is_awaiting())
                {
                    awaiting = true;
                }
            });
        }
        awaiting
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
        for tab in self.workspace.tabs.iter_mut() {
            tab.layout.for_each_leaf_content_mut(&mut |content| {
                if let App::Agent(ring) = content {
                    for s in ring.slots.iter() {
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
                            // Combine without losing either's transitions.
                            fp = fp.wrapping_add(elapsed).wrapping_mul(1_000_003)
                                ^ quiet.wrapping_add(1);
                        }
                    }
                }
            });
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
        let resume_id = self
            .agent_mut()
            .and_then(|c| c.channel.as_ref().and_then(|ch| ch.session_id()));
        let slot_cwd = self.agent_ring().map(|r| r.active().cwd.clone());
        let (attach_tx, attach_rx) =
            std::sync::mpsc::channel::<std::io::Result<AcpChannelClient>>();
        let cmd = std::env::var("SKETCH_ACP_AGENT").unwrap_or_default();
        let resume_for_worker = resume_id.clone();
        let _ = std::thread::Builder::new()
            .name("sketch-acp-force-restart".into())
            .spawn(move || {
                let _ = attach_tx.send(AcpChannelClient::spawn_with_resume_in(
                    &cmd,
                    slot_cwd,
                    resume_for_worker,
                    sketch::acp_channel::SketchFrontend::Gpui,
                ));
            });
        if let Some(ring) = self.agent_ring_mut() {
            ring.active_mut().resume_id = resume_id;
            let claude = &mut ring.active_mut().state;
            claude.channel = None; // Drop → kills the wedged subprocess.
            claude.attach_pending = Some(attach_rx);
            claude.turn_phase = TurnPhase::Idle;
            Self::append_system_notice(claude, "force-restarting agent (resuming session)…");
            claude.status = Some("force-restarting agent (resuming session)…".into());
        }
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
                    sketch::agent_transcript::UserTurnOrigin::LocalSubmit,
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
                    sketch::agent_transcript::UserTurnOrigin::LocalSubmit,
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

        // Session switching: Ctrl-] next, Ctrl-[ prev.
        if press.modifiers.contains(KMods::CONTROL) {
            if press.key == Key::Char(']') {
                self.switch_agent_session(1, cx);
                return;
            }
            if press.key == Key::Char('[') {
                self.switch_agent_session(-1, cx);
                return;
            }
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
