//! Linear App methods on `YaldaGpuiView`: open the tile, accept the typed
//! identifier/name, run the fetch off the paint thread, fold the result into
//! the tile's cached [`LinearView`], and the per-tile key handler. The body
//! render lives in `linear_view.rs`; the API client + data model in `linear.rs`.

use super::*;

impl YaldaGpuiView {
    /// Open a Linear tile (Cmd-L / Ctrl-L). Replaces the focused tile's content
    /// with a fresh `App::Linear`. No-op if already on a Linear tile.
    pub(crate) fn open_linear(&mut self, _: &OpenLinear, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_linear_inner(cx);
    }

    pub(crate) fn open_linear_inner(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.workspace.focused_content().expect("no focused window"),
            App::Linear(_)
        ) {
            return;
        }
        self.set_screen(App::Linear(LinearTile::new()));
        cx.notify();
    }

    /// Submit the typed input: if it parses as `<KEY>-<number>` fetch that
    /// issue, otherwise treat it as a project name. Tagged with a monotonic id
    /// so a stale response is discarded in `linear_apply`.
    pub(crate) fn linear_submit(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        // Ensure the cached body view exists (it normally does — the tile has
        // rendered — but submit-before-first-render is possible).
        let view = self.ensure_linear_view(target, cx);

        let (req, parsed, query) = {
            let Some(tile) = self.linear_tile_by_id_mut(target) else {
                return;
            };
            let query = tile.input.trim().to_string();
            if query.is_empty() {
                return;
            }
            tile.req += 1;
            let parsed = linear::parse_identifier(&query);
            tile.title = match &parsed {
                Some((team, n)) => format!("{team}-{n}"),
                None => query.clone(),
            };
            (tile.req, parsed, query)
        };

        let loading = match &parsed {
            Some((team, n)) => format!("loading {team}-{n}…"),
            None => format!("loading project \"{query}\"…"),
        };
        if let Some(v) = &view {
            v.update(cx, |lv, vcx| {
                lv.set_state(LinearViewState::Loading(loading));
                vcx.notify();
            });
        }
        cx.notify(); // main: input line + title

        let Some(key) = linear::api_key() else {
            self.linear_apply(
                target,
                req,
                Err("LINEAR_API_KEY is not set. Create a Linear personal API key \
                     (Linear → Settings → Security & access → Personal API keys), \
                     export it, and relaunch yalda."
                    .to_string()),
                cx,
            );
            return;
        };

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match parsed {
                        Some((team, number)) => linear::fetch_issue(&key, &team, number)
                            .map(|i| LinearFetch::Issue(Box::new(i))),
                        None => linear::fetch_project(&key, &query)
                            .map(|p| LinearFetch::Project(Box::new(p))),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.linear_apply(target, req, result, cx);
            });
        })
        .detach();
    }

    /// Fold a completed fetch into the tile that requested it (by stable
    /// `WindowId`, never ambient focus). Discards if the tile is gone, isn't a
    /// Linear tile, or has issued a newer request (`tile.req != req`).
    pub(crate) fn linear_apply(
        &mut self,
        target: workspace::WindowId,
        req: u64,
        result: Result<LinearFetch, String>,
        cx: &mut Context<Self>,
    ) {
        let view = {
            let Some(tile) = self.linear_tile_by_id_mut(target) else {
                return;
            };
            if tile.req != req {
                return;
            }
            // Denormalize the loaded title for the tab strip (read without cx).
            match &result {
                Ok(LinearFetch::Issue(i)) => {
                    if let Some(id) = &i.identifier {
                        tile.title = id.clone();
                    }
                }
                Ok(LinearFetch::Project(p)) => {
                    if let Some(n) = &p.name {
                        tile.title = n.clone();
                    }
                }
                Err(_) => {}
            }
            tile.view.clone()
        };

        let new_state = match result {
            Ok(LinearFetch::Issue(i)) => LinearViewState::Issue(i),
            Ok(LinearFetch::Project(p)) => LinearViewState::Project(p),
            Err(e) => LinearViewState::Error(e),
        };
        if let Some(v) = view {
            v.update(cx, |lv, vcx| {
                lv.set_state(new_state);
                vcx.notify();
            });
        }
        cx.notify(); // main: title changed
    }

    /// Get-or-create the cached [`LinearView`] for the tile at `target`. The two
    /// `linear_tile_by_id_mut` borrows are sequential (the `cx.new` between them
    /// needs `cx` exclusively), so there's no borrow overlap.
    fn ensure_linear_view(
        &mut self,
        target: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Option<Entity<LinearView>> {
        match self.linear_tile_by_id_mut(target) {
            Some(tile) => {
                if let Some(v) = &tile.view {
                    return Some(v.clone());
                }
            }
            None => return None,
        }
        let weak = cx.entity().downgrade();
        let view = cx.new(|_| LinearView::new(weak));
        let tile = self.linear_tile_by_id_mut(target)?;
        tile.view = Some(view.clone());
        Some(view)
    }

    fn linear_focused_tile_mut(&mut self) -> Option<&mut LinearTile> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Linear(tile) => Some(tile),
            _ => None,
        }
    }

    fn linear_tile_by_id_mut(&mut self, id: workspace::WindowId) -> Option<&mut LinearTile> {
        for tab in self.workspace.tabs.iter_mut() {
            if let Some(w) = tab.layout.find_leaf_mut(id) {
                return match &mut w.content {
                    App::Linear(tile) => Some(tile),
                    _ => None,
                };
            }
        }
        None
    }

    /// Scroll the focused Linear tile's body. The view (not the tile) owns the
    /// scroll, so this notifies the cached body — input typing never does.
    fn linear_scroll(&mut self, down: f32, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        let view = self
            .linear_tile_by_id_mut(target)
            .and_then(|t| t.view.clone());
        if let Some(v) = view {
            v.update(cx, |lv, vcx| {
                lv.scroll_by(down);
                vcx.notify();
            });
        }
    }

    /// Key handler for a focused Linear tile. Printable keys edit the input line
    /// (which lives on the tile ⇒ only the input row re-renders, body stays
    /// cached); Enter fetches; Esc clears; arrows / PageUp-Down scroll the body.
    pub(crate) fn handle_linear_key(
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
        match press.key {
            Key::Enter => self.linear_submit(cx),
            Key::Esc => {
                if let Some(t) = self.linear_focused_tile_mut() {
                    t.input.clear();
                }
                cx.notify();
            }
            Key::Backspace => {
                if let Some(t) = self.linear_focused_tile_mut() {
                    t.input.pop();
                }
                cx.notify();
            }
            Key::Down => self.linear_scroll(48.0, cx),
            Key::Up => self.linear_scroll(-48.0, cx),
            Key::PageDown => self.linear_scroll(400.0, cx),
            Key::PageUp => self.linear_scroll(-400.0, cx),
            Key::Char(c) => {
                if let Some(t) = self.linear_focused_tile_mut() {
                    t.input.push(c);
                }
                cx.notify();
            }
            _ => {}
        }
    }
}
