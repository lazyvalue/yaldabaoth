//! Cog explorer App methods on `YaldaGpuiView`: open the tile, fetch the Topic
//! browser / a graph bundle off the paint thread, fold the result into the tile's
//! cached [`CogView`], drive left selection + right-pane scroll, and the
//! per-tile key handler. The body render lives in `cog_view.rs`; the subprocess
//! client + data model in `cog.rs`.

use super::*;

impl YaldaGpuiView {
    /// Open a Cog explorer tile. Replaces the focused tile's content with a
    /// fresh `App::Cog` and kicks off the Topic-browser fetch. No-op if already
    /// on a Cog tile.
    pub(crate) fn open_cog(&mut self, _: &OpenCog, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_cog_inner(cx);
    }

    pub(crate) fn open_cog_inner(&mut self, cx: &mut Context<Self>) {
        if self.install_cog_tile(cx) {
            self.cog_load_graphs(cx);
        }
    }

    /// Swap the focused tile for a fresh Cog explorer. Returns `false` (no-op) if
    /// already on a Cog tile. Does NOT fetch — the caller kicks the graph load.
    /// Split out so tests can install a tile without the live `cog` subprocess.
    pub(crate) fn install_cog_tile(&mut self, cx: &mut Context<Self>) -> bool {
        if matches!(self.workspace.focused_content(), Some(App::Cog(_))) {
            return false;
        }
        self.set_screen(App::Cog(CogTile::new()));
        cx.notify();
        true
    }

    /// Kick a graph-list load for any Cog tile that still needs one (restored
    /// from disk, never opened). Run every frame from the root reconcile; the
    /// `needs_load` flag dedups so each tile loads exactly once.
    pub(crate) fn cog_reconcile_loads(&mut self, cx: &mut Context<Self>) {
        let mut targets: Vec<workspace::WindowId> = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.for_each_attached_window(&mut |w| {
                if let App::Cog(tile) = &w.content
                    && tile.needs_load
                {
                    targets.push(w.id());
                }
            });
        }
        // Runs during the root render, so do NOT load (and notify) inline — clear
        // the flag (mutation only) and spawn the load, whose notifies then land
        // outside the draw. Mirrors `reconcile_diagrams`' discipline.
        for target in targets {
            if let Some(tile) = self.cog_tile_by_id_mut(target) {
                tile.needs_load = false;
            }
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |v, cx| v.cog_load_graphs_into(target, cx));
            })
            .detach();
        }
    }

    /// Fetch the Topic hierarchy + address directory into the focused Cog tile.
    pub(crate) fn cog_load_graphs(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        self.cog_load_graphs_into(target, cx);
    }

    /// Fetch the primary Cog browser into a SPECIFIC Cog tile (by window id) —
    /// used by the reconcile kick, which must target the restored tile, not
    /// whatever is focused.
    pub(crate) fn cog_load_graphs_into(
        &mut self,
        target: workspace::WindowId,
        cx: &mut Context<Self>,
    ) {
        let view = self.ensure_cog_view(target, cx);
        let req = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.req += 1;
            tile.title = "Cog".into();
            tile.needs_load = false;
            tile.req
        };
        if let Some(v) = &view {
            v.update(cx, |cv, vcx| {
                cv.set_state(CogViewState::Loading("loading topics…".into()));
                vcx.notify();
            });
        }
        cx.notify();

        // Never spawn the live subprocess under test (hermetic — gap #2); the
        // reducer `cog_apply` is driven directly by tests.
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cursor = cog::event_cursor().ok();
                    cog::load_home().map(|home| CogFetch::HomeAt {
                        home: Box::new(home),
                        cursor,
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cog_apply(target, req, result, cx);
            });
        })
        .detach();
    }

    /// Open the graph highlighted in the focused tile's explorer: bump the
    /// request id, set the title + loading, and load the bundle off the paint
    /// thread.
    pub(crate) fn cog_open_selected_graph(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        let sel = self.cog_focused_tile_view().and_then(|v| {
            let cv = v.read(cx);
            cv.selected_graph_id()
                .map(|id| (id, cv.selected_graph_label()))
        });
        let Some((id, label)) = sel else {
            return;
        };
        self.cog_open_graph(target, id, label, cx);
    }

    /// Open a specific graph into the tile at `target` (the shared open path for
    /// keyboard Enter and a graph-row click): bump the request id, set the title
    /// + loading, and load the bundle off the paint thread.
    pub(crate) fn cog_open_graph(
        &mut self,
        target: workspace::WindowId,
        id: String,
        label: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // Preserve the Home backstack while presenting loading. Stable graph
        // identity is recorded before the fetch so a reboot during loading can
        // resume the same graph.
        if let Some(view) = self
            .cog_tile_by_id_mut(target)
            .and_then(|tile| tile.view.clone())
        {
            let status = format!("loading {id}…");
            view.update(cx, |cv, vcx| {
                cv.begin_graph_load(id.clone(), label.clone(), status);
                vcx.notify();
            });
        }
        self.cog_fetch_graph(target, id, label, cx);
    }

    /// Bump the tile's request id, denormalize the title, and load the graph
    /// bundle off the paint thread — WITHOUT touching the view state (the caller
    /// owns the loading state, so this is safe to call from inside a CogView
    /// click handler, where re-updating the view would double-borrow it).
    pub(crate) fn cog_fetch_graph(
        &mut self,
        target: workspace::WindowId,
        id: String,
        label: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let req = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.req += 1;
            if let Some(l) = &label {
                tile.title = l.clone();
            }
            tile.req
        };
        self.save_workspace_state();
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { cog::load_graph(&id).map(|b| CogFetch::Graph(Box::new(b))) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cog_apply(target, req, result, cx);
            });
        })
        .detach();
    }

    /// Open a graph (`id`/`label` already resolved by the clicked [`CogView`])
    /// into the tile that owns `view` — the graph-row click path. A click doesn't
    /// route through focus, so we resolve the target tile by matching the view
    /// entity (id comparison only — we never read `view`, which is still mutably
    /// borrowed by the click handler).
    pub(crate) fn cog_open_graph_for(
        &mut self,
        view: Entity<CogView>,
        id: String,
        label: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let vid = view.entity_id();
        let mut target = None;
        for wsp in self.workspace.workspaces.iter() {
            wsp.for_each_attached_window(&mut |w| {
                if let App::Cog(tile) = &w.content
                    && tile.view.as_ref().map(|v| v.entity_id()) == Some(vid)
                {
                    target = Some(w.id());
                }
            });
            if target.is_some() {
                break;
            }
        }
        let Some(target) = target else {
            return;
        };
        // The clicked view already set its own Loading state — only fetch here,
        // so we never re-update the still-borrowed CogView.
        self.cog_fetch_graph(target, id, label, cx);
    }

    /// Load a Topic target selected by a mouse row without re-borrowing the
    /// currently-mutating cached view.
    pub(crate) fn cog_fetch_topic_for(
        &mut self,
        view: Entity<CogView>,
        binding: CogTopicBinding,
        cx: &mut Context<Self>,
    ) {
        let vid = view.entity_id();
        let mut target = None;
        for workspace in self.workspace.workspaces.iter() {
            workspace.for_each_attached_window(&mut |window| {
                if let App::Cog(tile) = &window.content
                    && tile.view.as_ref().map(|item| item.entity_id()) == Some(vid)
                {
                    target = Some(window.id());
                }
            });
            if target.is_some() {
                break;
            }
        }
        if let Some(target) = target {
            self.cog_fetch_topic(target, binding, cx);
        }
    }

    fn cog_fetch_topic(
        &mut self,
        target: workspace::WindowId,
        binding: CogTopicBinding,
        cx: &mut Context<Self>,
    ) {
        let req = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.req += 1;
            tile.req
        };
        if cfg!(test) {
            return;
        }
        let address = binding.address.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { cog::load_topic_detail(&binding) })
                .await;
            let fetch = CogFetch::TopicDetail { address, result };
            let _ = this.update(cx, |this, cx| {
                this.cog_apply(target, req, Ok(fetch), cx);
            });
        })
        .detach();
    }

    /// Load an address selected by a mouse row without re-borrowing the
    /// currently-mutating cached view.
    pub(crate) fn cog_fetch_agent_for(
        &mut self,
        view: Entity<CogView>,
        address: CogAgentAddress,
        cx: &mut Context<Self>,
    ) {
        let vid = view.entity_id();
        let mut target = None;
        for workspace in self.workspace.workspaces.iter() {
            workspace.for_each_attached_window(&mut |window| {
                if let App::Cog(tile) = &window.content
                    && tile.view.as_ref().map(|item| item.entity_id()) == Some(vid)
                {
                    target = Some(window.id());
                }
            });
            if target.is_some() {
                break;
            }
        }
        if let Some(target) = target {
            self.cog_fetch_agent(target, address, cx);
        }
    }

    fn cog_fetch_agent(
        &mut self,
        target: workspace::WindowId,
        address: CogAgentAddress,
        cx: &mut Context<Self>,
    ) {
        let req = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.req += 1;
            tile.req
        };
        if cfg!(test) {
            return;
        }
        let address_id = address.id.clone();
        cx.spawn(async move |this, cx| {
            let detail = cx
                .background_executor()
                .spawn(async move { cog::load_agent_detail(address) })
                .await;
            let fetch = CogFetch::AgentDetail {
                address: address_id,
                detail: Box::new(detail),
            };
            let _ = this.update(cx, |this, cx| {
                this.cog_apply(target, req, Ok(fetch), cx);
            });
        })
        .detach();
    }

    fn cog_return_home(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.workspace.focused_window_id() else {
            return false;
        };
        let Some(view) = self.cog_focused_tile_view() else {
            return false;
        };
        let returned = view.update(cx, |cv, vcx| {
            let returned = cv.return_home();
            if returned {
                vcx.notify();
            }
            returned
        });
        if returned {
            self.cog_stop_watch(target);
            if let Some(tile) = self.cog_tile_by_id_mut(target) {
                tile.title = "Cog".into();
            }
            self.cog_live_tick(target, cx);
            self.save_workspace_state();
            cx.notify();
        }
        returned
    }

    /// Fold a completed fetch into the tile that requested it (by stable
    /// `WindowId`). Discards if the tile is gone, isn't a Cog tile, or has
    /// issued a newer request.
    pub(crate) fn cog_apply(
        &mut self,
        target: workspace::WindowId,
        req: u64,
        result: Result<CogFetch, String>,
        cx: &mut Context<Self>,
    ) {
        match self.cog_tile_by_id_mut(target) {
            Some(tile) if tile.req == req => {}
            _ => return,
        }
        match result {
            Ok(CogFetch::Graph(bundle)) => {
                // Set the view (clears any prior events), then start watching this
                // graph's live event stream into the fresh events pane.
                let id = bundle.graph.id.clone();
                let view = self
                    .cog_tile_by_id_mut(target)
                    .and_then(|tile| tile.view.clone());
                if let Some(view) = view {
                    view.update(cx, |cv, vcx| {
                        cv.enter_graph(bundle);
                        vcx.notify();
                    });
                }
                self.cog_start_watch(target, id, cx);
                self.cog_start_live_revalidation(target, cx);
                self.save_workspace_state();
            }
            Ok(CogFetch::Home(home)) => {
                self.cog_stop_watch(target);
                let view = self
                    .cog_tile_by_id_mut(target)
                    .and_then(|tile| tile.view.clone());
                let resume = view.as_ref().and_then(|view| {
                    view.update(cx, |cv, vcx| {
                        cv.install_home(*home);
                        vcx.notify();
                        cv.resume()
                    })
                });
                if let Some(tile) = self.cog_tile_by_id_mut(target) {
                    tile.title = "Cog".into();
                }
                self.cog_start_live_revalidation(target, cx);
                match resume {
                    Some(CogResume::Graph { id, label }) => {
                        self.cog_open_graph(target, id, label, cx)
                    }
                    Some(CogResume::Topic(binding)) => {
                        if let Some(view) = view {
                            view.update(cx, |cv, vcx| {
                                cv.set_topic_loading(binding.address.clone());
                                vcx.notify();
                            });
                        }
                        self.cog_fetch_topic(target, binding, cx);
                    }
                    Some(CogResume::Agent(address)) => {
                        if let Some(view) = view {
                            view.update(cx, |cv, vcx| {
                                cv.set_agent_loading(address.id.clone());
                                vcx.notify();
                            });
                        }
                        self.cog_fetch_agent(target, address, cx);
                    }
                    None => {}
                }
                self.save_workspace_state();
                cx.notify();
            }
            Ok(CogFetch::HomeAt { home, cursor }) => {
                if let Some(tile) = self.cog_tile_by_id_mut(target) {
                    tile.events_cursor = match (tile.events_cursor, cursor) {
                        (Some(old), Some(captured)) => Some(old.max(captured)),
                        (old @ Some(_), None) => old,
                        (None, captured) => captured,
                    };
                }
                self.cog_apply(target, req, Ok(CogFetch::Home(home)), cx);
            }
            Ok(CogFetch::Graphs(graphs)) => {
                self.cog_stop_watch(target);
                self.cog_set_view(
                    target,
                    CogViewState::Graphs {
                        graphs,
                        selected: 0,
                    },
                    cx,
                );
            }
            Ok(CogFetch::TopicDetail { address, result }) => {
                let view = self
                    .cog_tile_by_id_mut(target)
                    .and_then(|tile| tile.view.clone());
                if let Some(view) = view {
                    view.update(cx, |cv, vcx| {
                        cv.apply_topic_detail(address, result);
                        vcx.notify();
                    });
                }
            }
            Ok(CogFetch::AgentDetail { address, detail }) => {
                let view = self
                    .cog_tile_by_id_mut(target)
                    .and_then(|tile| tile.view.clone());
                if let Some(view) = view {
                    view.update(cx, |cv, vcx| {
                        cv.apply_agent_detail(address, *detail);
                        vcx.notify();
                    });
                }
            }
            Err(e) => {
                self.cog_stop_watch(target);
                self.cog_set_view(target, CogViewState::Error(e), cx);
                self.cog_start_live_revalidation(target, cx);
            }
        }
        let trailing = self
            .cog_tile_by_id_mut(target)
            .is_some_and(|tile| std::mem::take(&mut tile.events_pending));
        if trailing {
            self.cog_invalidate_current(target, cx);
        }
    }

    /// Start (or restart) the live `cog graph watch` stream for `target`'s graph.
    /// Kills any prior watcher first; events are folded in via `cog_push_event`,
    /// tagged with a generation so a killed watcher's late events are dropped.
    pub(crate) fn cog_start_watch(
        &mut self,
        target: workspace::WindowId,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.cog_stop_watch(target);
        let generation = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.watch_gen += 1;
            tile.watch_gen
        };
        // Never spawn the live subprocess under test (hermetic — gap #2).
        if cfg!(test) {
            return;
        }
        match cog::spawn_watch(&id) {
            Ok((child, mut rx)) => {
                if let Some(tile) = self.cog_tile_by_id_mut(target) {
                    tile.watch = Some(child);
                }
                cx.spawn(async move |this, cx| {
                    use futures::StreamExt;
                    while let Some(line) = rx.next().await {
                        if this
                            .update(cx, |v, cx| v.cog_push_event(target, generation, line, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                    let _ = this.update(cx, |v, cx| v.cog_watch_ended(target, generation, cx));
                })
                .detach();
            }
            Err(_) => {} // no stream — the events pane just shows "waiting"
        }
    }

    /// Stop the live watcher for `target` (kill the child, bump the generation so
    /// in-flight events are dropped).
    pub(crate) fn cog_stop_watch(&mut self, target: workspace::WindowId) {
        if let Some(tile) = self.cog_tile_by_id_mut(target) {
            tile.watch_gen += 1;
            if let Some(mut child) = tile.watch.take() {
                let _ = child.kill();
            }
        }
    }

    /// Mark a naturally-ended watcher absent. The bounded reconciliation loop
    /// then becomes the graph fallback until the next graph open restarts SSE.
    fn cog_watch_ended(
        &mut self,
        target: workspace::WindowId,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let ended = self
            .cog_tile_by_id_mut(target)
            .filter(|tile| tile.watch_gen == generation)
            .map(|tile| {
                if let Some(mut child) = tile.watch.take() {
                    let _ = child.try_wait();
                }
            })
            .is_some();
        if ended {
            self.cog_refresh_bundle(target, cx);
        }
    }

    /// Start one lifecycle-bound global subscriber and freshness supervisor per
    /// Cog tile. The event stream gives immediate invalidation; a disconnected
    /// installation revalidates once per second, and a connected stream gets a
    /// 30-second safety read. Dropping the tile cancels the task and child.
    fn cog_start_live_revalidation(&mut self, target: workspace::WindowId, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let token = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            if tile.live_task.is_some() {
                return;
            }
            tile.live_token
        };
        self.cog_start_events(target, cx);
        let task = cx.spawn(async move |this, cx| {
            let mut ticks = 0_u64;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                ticks += 1;
                let current = this
                    .update(cx, |view, cx| {
                        let state = view
                            .cog_tile_by_id_mut(target)
                            .map(|tile| (tile.live_token == token, tile.events_connected));
                        let Some((current, connected)) = state else {
                            return false;
                        };
                        if !current {
                            return false;
                        }
                        if !connected {
                            view.cog_start_events(target, cx);
                            view.cog_invalidate_current(target, cx);
                        } else if ticks % 30 == 0 {
                            view.cog_invalidate_current(target, cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !current {
                    break;
                }
            }
        });
        if let Some(tile) = self.cog_tile_by_id_mut(target) {
            tile.live_task = Some(task);
        }
    }

    /// Start/restart `cog events --follow` from the last applied global cursor.
    /// Reader output is generation tagged so a killed prior process cannot
    /// mutate a reused tile.
    fn cog_start_events(&mut self, target: workspace::WindowId, cx: &mut Context<Self>) {
        let (generation, cursor) = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            if tile.events_watch.is_some() {
                return;
            }
            tile.events_gen += 1;
            (tile.events_gen, tile.events_cursor)
        };
        if cfg!(test) {
            return;
        }
        match cog::spawn_events(cursor) {
            Ok((child, mut rx)) => {
                if let Some(tile) = self.cog_tile_by_id_mut(target) {
                    tile.events_watch = Some(child);
                    tile.events_connected = true;
                }
                cx.spawn(async move |this, cx| {
                    use futures::StreamExt;
                    while let Some(line) = rx.next().await {
                        if this
                            .update(cx, |view, cx| {
                                view.cog_push_global_event(target, generation, line, cx)
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    let _ = this.update(cx, |view, _| {
                        view.cog_event_stream_ended(target, generation)
                    });
                })
                .detach();
            }
            Err(_) => self.cog_event_stream_ended(target, generation),
        }
    }

    pub(crate) fn cog_push_global_event(
        &mut self,
        target: workspace::WindowId,
        generation: u64,
        line: String,
        cx: &mut Context<Self>,
    ) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            return;
        };
        let Some(event_id) = event.get("event_id").and_then(serde_json::Value::as_i64) else {
            return;
        };
        let fresh = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            if tile.events_gen != generation
                || tile.events_cursor.is_some_and(|cursor| event_id <= cursor)
            {
                false
            } else {
                tile.events_cursor = Some(event_id);
                true
            }
        };
        if fresh {
            // Scope metadata is an optimization hint, never a correctness
            // dependency: every new entity kind still refreshes this tile's
            // current projection.
            self.cog_invalidate_current(target, cx);
        }
    }

    pub(crate) fn cog_event_stream_ended(
        &mut self,
        target: workspace::WindowId,
        generation: u64,
    ) {
        let Some(tile) = self.cog_tile_by_id_mut(target) else {
            return;
        };
        if tile.events_gen != generation {
            return;
        }
        tile.events_connected = false;
        if let Some(mut child) = tile.events_watch.take() {
            let _ = child.kill();
        }
    }

    /// Invalidate whatever the tile currently projects. Home uses the existing
    /// exact-selection atomic snapshot; Graph uses its existing burst coalescer;
    /// loading/error retries the cursor-before-read startup.
    pub(crate) fn cog_invalidate_current(
        &mut self,
        target: workspace::WindowId,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .cog_tile_by_id_mut(target)
            .and_then(|tile| tile.view.clone())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.current_graph_id().is_some(),
                    view.live_home_key().is_some(),
                    view.is_loading(),
                )
            });
        match state {
            Some((true, _, _)) => self.cog_refresh_bundle(target, cx),
            Some((_, true, _)) => self.cog_live_tick(target, cx),
            Some((_, _, true)) => {
                if let Some(tile) = self.cog_tile_by_id_mut(target) {
                    tile.events_pending = true;
                }
            }
            Some(_) => self.cog_load_graphs_into(target, cx),
            None => {}
        }
    }

    /// Run one freshness cycle. Graphs keep their event-driven watcher and use
    /// the cycle only as a fallback if that process is absent; browser surfaces
    /// load their visible directory + detail as one atomic snapshot.
    pub(crate) fn cog_live_tick(&mut self, target: workspace::WindowId, cx: &mut Context<Self>) {
        let graph_fallback = self
            .cog_tile_by_id_mut(target)
            .and_then(|tile| tile.view.clone().map(|view| (tile.watch.is_none(), view)))
            .is_some_and(|(no_watch, view)| no_watch && view.read(cx).current_graph_id().is_some());
        if graph_fallback {
            self.cog_refresh_bundle(target, cx);
            return;
        }

        let Some(key) = self
            .cog_tile_by_id_mut(target)
            .and_then(|tile| tile.view.clone())
            .and_then(|view| view.read(cx).live_home_key())
        else {
            return;
        };
        let token = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            if tile.live_refreshing {
                tile.live_pending = true;
                return;
            }
            tile.live_refreshing = true;
            tile.live_token
        };
        // Hermetic tests drive the real apply seam directly after asserting the
        // in-flight/coalescing state.
        if cfg!(test) {
            return;
        }
        let request_key = key.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { cog::load_live_home(request_key) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.cog_apply_live_home(target, token, key, result, cx)
            });
        })
        .detach();
    }

    pub(crate) fn cog_apply_live_home(
        &mut self,
        target: workspace::WindowId,
        token: u64,
        key: CogLiveHomeKey,
        result: Result<CogLiveHome, String>,
        cx: &mut Context<Self>,
    ) {
        let pending = {
            let Some(tile) = self
                .cog_tile_by_id_mut(target)
                .filter(|tile| tile.live_token == token)
            else {
                return;
            };
            tile.live_refreshing = false;
            std::mem::take(&mut tile.live_pending)
        };
        if let Ok(live) = result {
            let view = self
                .cog_tile_by_id_mut(target)
                .and_then(|tile| tile.view.clone());
            if let Some(view) = view {
                let changed = view.update(cx, |cv, _| cv.apply_live_home(&key, live));
                if changed {
                    view.update(cx, |_cv, vcx| vcx.notify());
                    self.save_workspace_state();
                }
            }
        }
        if pending {
            self.cog_live_tick(target, cx);
        }
    }

    /// Fold one live event line into the tile's events pane (generation-guarded).
    pub(crate) fn cog_push_event(
        &mut self,
        target: workspace::WindowId,
        generation: u64,
        line: String,
        cx: &mut Context<Self>,
    ) {
        let fresh = self
            .cog_tile_by_id_mut(target)
            .map(|t| t.watch_gen == generation)
            .unwrap_or(false);
        if !fresh {
            return;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else {
            return;
        };
        let view = self.cog_tile_by_id_mut(target).and_then(|t| t.view.clone());
        if let Some(v) = view {
            v.update(cx, |cv, vcx| {
                cv.push_event(val);
                vcx.notify();
            });
        }
        // A live event means the graph moved — auto-refresh its data (coalesced).
        self.cog_refresh_bundle(target, cx);
    }

    /// Refresh the open graph's bundle IN PLACE (keeping the events feed) — the
    /// auto-refresh path fired by a live event and by manual `r`. Coalesces a
    /// burst into one in-flight reload plus one queued (`refresh_pending`), and
    /// leaves the live watcher untouched.
    pub(crate) fn cog_refresh_bundle(
        &mut self,
        target: workspace::WindowId,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .cog_tile_by_id_mut(target)
            .and_then(|t| t.view.clone())
            .and_then(|v| v.read(cx).current_graph_id())
        else {
            return;
        };
        {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            if tile.refreshing {
                tile.refresh_pending = true;
                return;
            }
            tile.refreshing = true;
        }
        // Don't spawn the live subprocess reload under test — the `refreshing`
        // flag is set (testable) and `cog_apply_refresh` is driven directly.
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { cog::load_graph(&id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cog_apply_refresh(target, result, cx);
            });
        })
        .detach();
    }

    /// Fold a refresh reload into the tile IN PLACE (via `update_bundle`, so the
    /// events feed + selection survive), then fire a queued refresh if one landed
    /// while this was in flight.
    pub(crate) fn cog_apply_refresh(
        &mut self,
        target: workspace::WindowId,
        result: Result<CogBundle, String>,
        cx: &mut Context<Self>,
    ) {
        let pending = {
            let Some(tile) = self.cog_tile_by_id_mut(target) else {
                return;
            };
            tile.refreshing = false;
            std::mem::take(&mut tile.refresh_pending)
        };
        if let Ok(bundle) = result {
            let view = self.cog_tile_by_id_mut(target).and_then(|t| t.view.clone());
            if let Some(v) = view {
                v.update(cx, |cv, vcx| {
                    if cv.update_bundle(Box::new(bundle)) {
                        vcx.notify();
                    }
                });
            }
        }
        if pending {
            self.cog_refresh_bundle(target, cx);
        }
    }

    /// Push a new body state onto the tile's cached view (mutation-site notify),
    /// then notify the root for the workspace-strip title.
    fn cog_set_view(
        &mut self,
        target: workspace::WindowId,
        state: CogViewState,
        cx: &mut Context<Self>,
    ) {
        let view = self.cog_tile_by_id_mut(target).and_then(|t| t.view.clone());
        if let Some(v) = view {
            v.update(cx, |cv, vcx| {
                cv.set_state(state);
                vcx.notify();
            });
        }
        cx.notify();
    }

    /// Move the left selection in the focused Cog tile.
    pub(crate) fn cog_select(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                cv.select_move(delta);
                vcx.notify();
            });
            self.save_workspace_state();
            let selected = v.read(cx).selected_topic_binding();
            if let Some(binding) = selected {
                v.update(cx, |cv, vcx| {
                    cv.set_topic_loading(binding.address.clone());
                    vcx.notify();
                });
                self.cog_fetch_topic(target, binding, cx);
                return;
            }
            let selected = v.read(cx).selected_agent();
            if let Some(address) = selected {
                v.update(cx, |cv, vcx| {
                    cv.set_agent_loading(address.id.clone());
                    vcx.notify();
                });
                self.cog_fetch_agent(target, address, cx);
            }
        }
    }

    /// Run a graph-explorer search (`/`) mutation on the focused view.
    pub(crate) fn cog_filter_op(&mut self, cx: &mut Context<Self>, op: impl FnOnce(&mut CogView)) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                op(cv);
                vcx.notify();
            });
        }
    }

    /// Scroll the focused Cog tile's right detail pane.
    pub(crate) fn cog_scroll(&mut self, down: f32, cx: &mut Context<Self>) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                cv.scroll_right(down);
                vcx.notify();
            });
        }
    }

    /// Scroll the focused Cog tile's live-events pane.
    pub(crate) fn cog_scroll_events(&mut self, down: f32, cx: &mut Context<Self>) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                cv.scroll_events(down);
                vcx.notify();
            });
        }
    }

    /// Toggle the focused Cog tile's live-events strip hidden/shown (the
    /// `cog-toggle-events` tile menu command).
    pub(crate) fn cog_toggle_events(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| cv.toggle_events(vcx));
            self.save_workspace_state();
        }
    }

    /// Scroll whichever scroll pane is focused — the events pane when
    /// `to_events`, else the detail pane.
    fn cog_scroll_active(&mut self, down: f32, to_events: bool, cx: &mut Context<Self>) {
        if to_events {
            self.cog_scroll_events(down, cx);
        } else {
            self.cog_scroll(down, cx);
        }
    }

    /// Push a global-invalidation (theme / zoom) onto every Cog tile's cached
    /// body — the body reads the global theme/font/zoom off the root, which is
    /// in no per-tile seq. Mirrors `notify_linear_views`.
    pub(crate) fn notify_cog_views(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        let mut views: Vec<Entity<CogView>> = Vec::new();
        for wsp in self.workspace.workspaces.iter() {
            wsp.for_each_attached_window(&mut |w| {
                if let App::Cog(tile) = &w.content
                    && let Some(v) = &tile.view
                {
                    views.push(v.clone());
                }
            });
        }
        for v in views {
            let label = v.read(cx).perf_label();
            record_notify(label, reason);
            v.update(cx, |_cv, vcx| vcx.notify());
        }
    }

    fn cog_focused_tile_view(&self) -> Option<Entity<CogView>> {
        match self.workspace.focused_content()? {
            App::Cog(tile) => tile.view.clone(),
            _ => None,
        }
    }

    /// Get-or-create the cached [`CogView`] for the tile at `target`.
    fn ensure_cog_view(
        &mut self,
        target: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Option<Entity<CogView>> {
        match self.cog_tile_by_id_mut(target) {
            Some(tile) => {
                if let Some(v) = &tile.view {
                    return Some(v.clone());
                }
            }
            None => return None,
        }
        let remembered = self.cog_tile_by_id_mut(target)?.remembered.clone();
        let weak = cx.entity().downgrade();
        let view = cx.new(|_| CogView::new(weak, remembered));
        let tile = self.cog_tile_by_id_mut(target)?;
        tile.view = Some(view.clone());
        Some(view)
    }

    fn cog_tile_by_id_mut(&mut self, id: workspace::WindowId) -> Option<&mut CogTile> {
        match &mut self.workspace.tile_mut(id)?.content {
            App::Cog(tile) => Some(tile),
            _ => None,
        }
    }

    /// Move keyboard focus in the focused Cog tile's view. `to_right = true`
    /// focuses the detail pane; `false` the selector.
    pub(crate) fn cog_set_focus(&mut self, to_right: bool, cx: &mut Context<Self>) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                if to_right {
                    cv.focus_right();
                } else {
                    cv.focus_left();
                }
                vcx.notify();
            });
            self.save_workspace_state();
        }
    }

    /// Toggle keyboard focus between the two panes (Tab).
    pub(crate) fn cog_toggle_focus(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.cog_focused_tile_view() {
            v.update(cx, |cv, vcx| {
                cv.toggle_focus();
                vcx.notify();
            });
            self.save_workspace_state();
        }
    }

    /// Key handler for a focused Cog tile. The tile is navigation-only (no text
    /// entry). Leaders (`space`/`.`/`?`) are handled first, then the press is
    /// routed to [`handle_cog_press`](Self::handle_cog_press).
    pub(crate) fn handle_cog_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Let global Cmd/Ctrl shortcuts fall through to their actions.
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        let press = keystroke_to_keypress(&ev.keystroke);

        // Universal leaders: this tile never captures text, so `space`/`.`/`?`
        // always open the menus first.
        if self.leader_intercept(&press, cx) {
            return;
        }
        self.handle_cog_press(press, cx);
    }

    /// The Cog tile's navigation model, at the [`KeyPress`] level (so tests can
    /// drive it directly). Focus decides what `j`/`k`/arrows do:
    ///
    /// - Explorer (graph list): `j`/`k` select a graph; `Enter`/`o`/`l`/`→` open it.
    /// - Graph, LEFT focus: `j`/`k` select a node; `Enter`/`o`/`l`/`→`/`Tab` move
    ///   focus to the detail pane; `Esc`/`h`/`←` go back to the graph list.
    /// - Graph, RIGHT focus: `j`/`k`/arrows and `d`/`u`/PageUp/Down scroll the
    ///   detail pane; `Esc`/`h`/`←`/`Tab` return focus to the node list.
    ///
    /// `r` reloads in both states.
    pub(crate) fn handle_cog_press(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        let (in_graphs, in_home, focused_right, focused_events, filtering) = self
            .cog_focused_tile_view()
            .map(|v| {
                let cv = v.read(cx);
                (
                    cv.in_graphs(),
                    cv.in_home(),
                    cv.focused_right(),
                    cv.focused_events(),
                    cv.is_filtering(),
                )
            })
            .unwrap_or((false, false, false, false, false));

        // Graph-explorer search sub-mode: printable keys type into the filter;
        // arrows move within matches; Enter opens; Esc exits.
        if filtering {
            match press.key {
                Key::Esc => self.cog_filter_op(cx, |cv| cv.filter_clear()),
                Key::Enter => self.cog_open_selected_graph(cx),
                Key::Backspace => self.cog_filter_op(cx, |cv| cv.filter_backspace()),
                Key::Down => self.cog_select(1, cx),
                Key::Up => self.cog_select(-1, cx),
                Key::Char(c) => self.cog_filter_op(cx, |cv| cv.filter_push(c)),
                _ => {}
            }
            return;
        }

        match press.key {
            // `/` starts the graph-explorer search.
            Key::Char('/') if in_graphs => self.cog_filter_op(cx, |cv| cv.start_filter()),

            // Tab cycles selector → detail → events → selector.
            Key::Tab => self.cog_toggle_focus(cx),

            // Dive into / advance to the detail pane.
            Key::Enter | Key::Char('o') | Key::Char('l') | Key::Right => {
                if in_graphs {
                    self.cog_open_selected_graph(cx);
                } else if in_home {
                    let binding = self
                        .cog_focused_tile_view()
                        .and_then(|view| view.read(cx).selected_topic_binding());
                    match binding {
                        Some(binding) if binding.kind == CogTopicKind::Graph => {
                            self.cog_open_selected_graph(cx)
                        }
                        Some(_) => self.cog_set_focus(true, cx),
                        None => {
                            let agent_selected = self
                                .cog_focused_tile_view()
                                .and_then(|view| view.read(cx).selected_agent())
                                .is_some();
                            if agent_selected {
                                self.cog_set_focus(true, cx);
                            } else if let Some(view) = self.cog_focused_tile_view() {
                                view.update(cx, |cv, vcx| {
                                    if cv.toggle_selected_topic_folder() {
                                        vcx.notify();
                                    }
                                });
                            }
                        }
                    }
                } else {
                    self.cog_set_focus(true, cx);
                }
            }

            // Back out: a focused scroll pane → selector, then selector → graph list.
            Key::Esc | Key::Char('h') | Key::Left => {
                if in_graphs {
                    // nothing above the explorer
                } else if focused_right || focused_events {
                    self.cog_set_focus(false, cx);
                } else if self.cog_return_home(cx) {
                } else {
                    self.cog_load_graphs(cx);
                }
            }

            // j/k/arrows: scroll the focused scroll pane (events/detail), else select.
            Key::Char('j') | Key::Down => {
                if focused_events {
                    self.cog_scroll_events(60.0, cx);
                } else if focused_right {
                    self.cog_scroll(60.0, cx);
                } else {
                    self.cog_select(1, cx);
                }
            }
            Key::Char('k') | Key::Up => {
                if focused_events {
                    self.cog_scroll_events(-60.0, cx);
                } else if focused_right {
                    self.cog_scroll(-60.0, cx);
                } else {
                    self.cog_select(-1, cx);
                }
            }

            // d/u/PageUp/Down scroll the focused scroll pane (events or detail).
            Key::Char('d') => self.cog_scroll_active(220.0, focused_events, cx),
            Key::Char('u') => self.cog_scroll_active(-220.0, focused_events, cx),
            Key::PageDown => self.cog_scroll_active(440.0, focused_events, cx),
            Key::PageUp => self.cog_scroll_active(-440.0, focused_events, cx),

            Key::Char('r') => {
                if in_graphs || in_home {
                    self.cog_load_graphs(cx);
                } else {
                    self.cog_reload_current(cx);
                }
            }
            _ => {}
        }
    }

    /// Refresh the focused Cog tile from the local menu: reload the open graph,
    /// or the graph list if none is open.
    pub(crate) fn cog_refresh_focused(&mut self, cx: &mut Context<Self>) {
        let in_browser = self
            .cog_focused_tile_view()
            .map(|v| {
                let view = v.read(cx);
                view.in_graphs() || view.in_home()
            })
            .unwrap_or(true);
        if in_browser {
            self.cog_load_graphs(cx);
        } else {
            self.cog_reload_current(cx);
        }
    }

    /// Reload the currently-open graph bundle (the `r` refresh in a graph). Uses
    /// the in-place refresh path so the live-events feed is preserved (not the
    /// graph-change path, which would clear it and restart the watcher).
    fn cog_reload_current(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        self.cog_refresh_bundle(target, cx);
    }
}
