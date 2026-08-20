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
        if matches!(self.workspace.focused_content(), Some(App::Linear(_))) {
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
                Err(
                    "LINEAR_API_KEY is not set. Create a Linear personal API key \
                     (Linear → Settings → Security & access → Personal API keys), \
                     export it, and relaunch yalda."
                        .to_string(),
                ),
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
                        None => linear::fetch_projects(&key, &query).map(LinearFetch::Projects),
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
        // Stale-guard up front (a newer query superseded this one).
        match self.linear_tile_by_id_mut(target) {
            Some(tile) if tile.req == req => {}
            _ => return,
        }

        // A project-name search resolves to one of three outcomes; the
        // single-match case kicks off a second fetch (full detail by id).
        if let Ok(LinearFetch::Projects(cands)) = result {
            match cands.len() {
                0 => self.linear_set_view(
                    target,
                    LinearViewState::Error(
                        "No project matching that name — type part of the project name (not an issue id)."
                            .to_string(),
                    ),
                    cx,
                ),
                1 => {
                    let c = cands.into_iter().next().unwrap();
                    self.linear_open_project(target, &c.id, c.name, cx);
                }
                _ => self.linear_set_view(
                    target,
                    LinearViewState::ProjectPicker {
                        candidates: cands,
                        selected: 0,
                    },
                    cx,
                ),
            }
            return;
        }

        // Issue / full-project / error: denormalize the title, set the body.
        if let Some(tile) = self.linear_tile_by_id_mut(target) {
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
                _ => {}
            }
        }
        let new_state = match result {
            Ok(LinearFetch::Issue(i)) => LinearViewState::Issue(i),
            Ok(LinearFetch::Project(p)) => LinearViewState::Project(p),
            Ok(LinearFetch::Projects(_)) => return, // handled above
            Err(e) => LinearViewState::Error(e),
        };
        self.linear_set_view(target, new_state, cx);
    }

    /// Push a new body state onto the tile's cached view (mutation-site notify),
    /// then notify the root for the workspace-strip title. If the tile is in Normal
    /// mode, re-enter browse so the cursor is on the first link of the freshly
    /// loaded body (Insert leaves it hidden — you're typing).
    fn linear_set_view(
        &mut self,
        target: workspace::WindowId,
        state: LinearViewState,
        cx: &mut Context<Self>,
    ) {
        let (view, normal) = match self.linear_tile_by_id_mut(target) {
            Some(t) => (t.view.clone(), t.mode == LinearMode::Normal),
            None => (None, false),
        };
        if let Some(v) = view {
            v.update(cx, |lv, vcx| {
                lv.set_state(state);
                if normal {
                    lv.enter_select();
                }
                vcx.notify();
            });
        }
        cx.notify();
    }

    /// Open a project by id (the picker / single-match path): bump the request
    /// id, set the title + loading, and fetch full detail off the paint thread.
    pub(crate) fn linear_open_project(
        &mut self,
        target: workspace::WindowId,
        id: &str,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = id.to_string();
        let req = {
            let Some(tile) = self.linear_tile_by_id_mut(target) else {
                return;
            };
            tile.req += 1;
            if let Some(n) = &name {
                tile.title = n.clone();
            }
            tile.req
        };
        let label = name.unwrap_or_else(|| "project".into());
        self.linear_set_view(
            target,
            LinearViewState::Loading(format!("loading \"{label}\"…")),
            cx,
        );

        let Some(key) = linear::api_key() else {
            self.linear_apply(
                target,
                req,
                Err("LINEAR_API_KEY is not set.".to_string()),
                cx,
            );
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    linear::fetch_project_by_id(&key, &id)
                        .map(|p| LinearFetch::Project(Box::new(p)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.linear_apply(target, req, result, cx);
            });
        })
        .detach();
    }

    /// Jump to the body cursor's target: set the input to the target string and
    /// run the normal submit path, which fetches the issue (or runs the
    /// project-name search) and renders it into THIS tile.
    fn linear_activate_selected(&mut self, cx: &mut Context<Self>) {
        let target = self
            .linear_focused_tile_view()
            .and_then(|v| v.read(cx).selected_target());
        let query = match target {
            Some(NavTarget::Issue(id)) if !id.trim().is_empty() => id,
            Some(NavTarget::Project(name)) => name,
            _ => return,
        };
        if let Some(t) = self.linear_focused_tile_mut() {
            t.input = query;
        }
        self.linear_submit(cx);
    }

    /// Open the project highlighted in the focused tile's picker.
    fn linear_open_selected_project(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.workspace.focused_window_id() else {
            return;
        };
        let cand = self
            .linear_focused_tile_view()
            .and_then(|v| v.read(cx).selected_candidate());
        if let Some(c) = cand {
            self.linear_open_project(target, &c.id, c.name, cx);
        }
    }

    fn linear_focused_tile_view(&self) -> Option<Entity<LinearView>> {
        match self.workspace.focused_content()? {
            App::Linear(tile) => tile.view.clone(),
            _ => None,
        }
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
        match self.workspace.focused_content_mut()? {
            App::Linear(tile) => Some(tile),
            _ => None,
        }
    }

    fn linear_tile_by_id_mut(&mut self, id: workspace::WindowId) -> Option<&mut LinearTile> {
        match &mut self.workspace.tile_mut(id)?.content {
            App::Linear(tile) => Some(tile),
            _ => None,
        }
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

    /// The focused Linear tile's current mode (Insert if not a Linear tile).
    fn linear_focused_mode(&self) -> LinearMode {
        match self.workspace.focused_content() {
            Some(App::Linear(t)) => t.mode,
            _ => LinearMode::Insert,
        }
    }

    /// Switch the focused Linear tile's mode. Entering Normal shows the browse
    /// cursor (on the body's first link, if any); entering Insert hides it so
    /// the caret-blinking input is the only focus.
    pub(crate) fn linear_set_mode(&mut self, mode: LinearMode, cx: &mut Context<Self>) {
        let view = match self.linear_focused_tile_mut() {
            Some(t) => {
                t.mode = mode;
                t.view.clone()
            }
            None => return,
        };
        if let Some(v) = view {
            v.update(cx, |lv, c| {
                match mode {
                    LinearMode::Normal => lv.enter_select(),
                    LinearMode::Insert => lv.exit_select(),
                }
                c.notify();
            });
        }
        cx.notify();
    }

    /// Move the browse cursor (Normal mode), entering browse on the first link
    /// if it wasn't already.
    fn linear_nav(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(view) = self.linear_focused_tile_view() {
            view.update(cx, |lv, c| {
                lv.enter_select();
                lv.nav_move(delta);
                c.notify();
            });
        }
    }

    /// Open the loaded issue/project in the system browser (macOS `open`).
    pub(crate) fn linear_open_url(&mut self, cx: &mut Context<Self>) {
        let url = self
            .linear_focused_tile_view()
            .and_then(|v| v.read(cx).current_url());
        if let Some(url) = url {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
    }

    /// Copy the loaded issue/project URL to the clipboard.
    pub(crate) fn linear_copy_url(&mut self, cx: &mut Context<Self>) {
        let url = self
            .linear_focused_tile_view()
            .and_then(|v| v.read(cx).current_url());
        if let Some(url) = url {
            cx.write_to_clipboard(ClipboardItem::new_string(url));
        }
    }

    /// Key handler for a focused Linear tile. The tile is **modal**: in Insert
    /// the printable keys edit the query line (only the input row re-renders —
    /// the body stays cached); in Normal they are commands, so `<space>`/`.`
    /// reach the tile / workspace menus and j/k browse the body's links. The
    /// project picker is its own transient sub-modal, handled first.
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

        // Universal leaders: in Normal mode (and the project picker — both are
        // navigation, not text entry), `<space>`/`.`/`?` open the menus first.
        if self.leader_intercept(&press, cx) {
            return;
        }

        // Picker mode: the body is a list of project candidates — reinterpret
        // navigation keys to move/select within it (Esc returns to editing).
        if let Some(view) = self.linear_focused_tile_view()
            && view.read(cx).is_picker()
        {
            match press.key {
                Key::Up | Key::Char('k') => {
                    view.update(cx, |lv, c| {
                        lv.picker_move(-1);
                        c.notify();
                    });
                }
                Key::Down | Key::Char('j') => {
                    view.update(cx, |lv, c| {
                        lv.picker_move(1);
                        c.notify();
                    });
                }
                Key::Enter => self.linear_open_selected_project(cx),
                Key::Char(d) if d.is_ascii_digit() && d != '0' => {
                    let idx = (d as u8 - b'1') as usize;
                    view.update(cx, |lv, c| {
                        lv.picker_set(idx);
                        c.notify();
                    });
                    self.linear_open_selected_project(cx);
                }
                Key::Esc => {
                    view.update(cx, |lv, c| {
                        lv.set_state(LinearViewState::Empty);
                        c.notify();
                    });
                }
                _ => {}
            }
            return;
        }

        match self.linear_focused_mode() {
            LinearMode::Insert => self.handle_linear_insert_key(press, cx),
            LinearMode::Normal => self.handle_linear_normal_key(press, cx),
        }
    }

    /// Insert mode: type the query. Enter fetches; Esc drops to Normal (so the
    /// menus and motions become reachable); arrows / PageUp-Down scroll the body.
    pub(crate) fn handle_linear_insert_key(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        match press.key {
            Key::Enter => self.linear_submit(cx),
            Key::Esc => self.linear_set_mode(LinearMode::Normal, cx),
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

    /// Normal mode: printable keys are commands, not text. `<space>` opens the
    /// Linear (tile) local menu, `.` the workspace menu, `i`/`a`/`/` return to Insert,
    /// j/k browse the body's links and Enter jumps to the selected one. Unbound
    /// keys are no-ops (never typed) — that's what keeps `<space>`/`.` free.
    pub(crate) fn handle_linear_normal_key(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        match press.key {
            // Leaders (space/./?) are handled universally in `handle_linear_key`.
            Key::Char('i') | Key::Char('a') | Key::Char('/') => {
                self.linear_set_mode(LinearMode::Insert, cx)
            }
            Key::Char('j') | Key::Down => self.linear_nav(1, cx),
            Key::Char('k') | Key::Up => self.linear_nav(-1, cx),
            Key::Enter | Key::Char('o') => self.linear_activate_selected(cx),
            Key::PageDown => self.linear_scroll(400.0, cx),
            Key::PageUp => self.linear_scroll(-400.0, cx),
            // Esc clears the query and the browse cursor (a reset), staying in
            // Normal — Insert's Esc is what entered Normal in the first place.
            Key::Esc => {
                let view = match self.linear_focused_tile_mut() {
                    Some(t) => {
                        t.input.clear();
                        t.view.clone()
                    }
                    None => return,
                };
                if let Some(v) = view {
                    v.update(cx, |lv, c| {
                        lv.exit_select();
                        c.notify();
                    });
                }
                cx.notify();
            }
            _ => {}
        }
    }
}
