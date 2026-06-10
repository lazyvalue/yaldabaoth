//! Browser tile + file-browser/outline rail methods on SketchGpuiView:
//! navigation, filtering, rail focus/resize and selection. Extracted
//! verbatim from main.rs (split-gpui-main, stage 2).

use super::*;

impl SketchGpuiView {
    pub(crate) fn browser_down(
        &mut self,
        _: &BrowserDown,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            if let Some(wm) = &mut b.fb.worktree_mode {
                wm.move_down();
            } else {
                b.fb.move_down();
            }
            cx.notify();
        }
    }
    pub(crate) fn browser_up(&mut self, _: &BrowserUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.browser_mut() {
            if let Some(wm) = &mut b.fb.worktree_mode {
                wm.move_up();
            } else {
                b.fb.move_up();
            }
            cx.notify();
        }
    }
    pub(crate) fn browser_enter(
        &mut self,
        _: &BrowserEnter,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut()
            && b.fb.worktree_mode.is_some()
        {
            b.fb.select_worktree();
            cx.notify();
            return;
        }
        let to_open = match self.browser_mut() {
            Some(b) => b.fb.enter_selected(),
            None => return,
        };
        if let Some(path) = to_open {
            self.open_file(path);
        }
        cx.notify();
    }
    pub(crate) fn browser_parent(
        &mut self,
        _: &BrowserParent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                return; // no-op in worktree mode
            }
            b.fb.go_parent();
            cx.notify();
        }
    }
    pub(crate) fn browser_worktrees(
        &mut self,
        _: &BrowserWorktrees,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            if b.fb.worktree_mode.is_some() {
                b.fb.exit_worktree_mode();
            } else {
                b.fb.enter_worktree_mode();
            }
            cx.notify();
        }
    }
    pub(crate) fn browser_toggle_hidden(
        &mut self,
        _: &BrowserToggleHidden,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            b.fb.toggle_hidden();
            cx.notify();
        }
    }
    pub(crate) fn browser_cycle_sort(
        &mut self,
        _: &BrowserCycleSort,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            b.fb.cycle_sort();
            cx.notify();
        }
    }
    pub(crate) fn browser_close(
        &mut self,
        _: &BrowserClose,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If in worktree mode, Esc exits that overlay instead of closing.
        if let Some(b) = self.browser_mut()
            && b.fb.worktree_mode.is_some()
        {
            b.fb.exit_worktree_mode();
            cx.notify();
            return;
        }
        let underlying = match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Buffer(BufferApp::Picking(b)) => b.underlying.take(),
            _ => return,
        };
        // If the browser was opened over an existing tile (Cmd-O from a
        // Doc/Edit/Claude window), restore that prior content in place —
        // user pressed Esc/q to cancel the file pick.
        if let Some(boxed) = underlying {
            self.set_screen(*boxed);
            self.save_workspace_state();
            cx.notify();
            return;
        }
        // Standalone browser (new-tab open, persisted browser tab, split
        // fallback). Try to dismiss the tile:
        //   - one tile of a split → close just that tile.
        //   - sole tile in tab, multiple tabs → close the tab.
        //   - sole tile in sole tab → no-op. Esc/q is intentionally NOT a
        //     quit shortcut — too easy to lose the app by mashing keys.
        //     Quit lives on Cmd-Q.
        match self.workspace.close_focused() {
            Ok(Some(_)) => {
                self.save_workspace_state();
                cx.notify();
            }
            Ok(None) => {
                if self.workspace.tabs.len() > 1 {
                    let idx = self.workspace.active_tab;
                    self.workspace.close_tab(idx);
                    self.save_workspace_state();
                    cx.notify();
                }
            }
            Err(()) => {}
        }
    }

    pub(crate) fn browser_filter(
        &mut self,
        _: &BrowserFilter,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.browser_mut() {
            if b.fb.filter_mode {
                b.fb.clear_filter();
            } else {
                b.fb.filter_mode = true;
                b.fb.set_filter("");
            }
            cx.notify();
        }
    }

    /// Key-down handler for browser filter text input.
    pub(crate) fn handle_browser_filter_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let filter_mode = match self.browser_mut() {
            Some(b) => b.fb.filter_mode,
            None => return,
        };
        if !filter_mode {
            // Not filtering — bare `m`/`'` starts a mark chord so browser
            // tiles can be marked/jumped like any other tile.
            if self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
                cx.stop_propagation();
            }
            return;
        }
        let Some(b) = self.browser_mut() else { return };
        match press.key {
            Key::Esc => {
                b.fb.clear_filter();
                cx.notify();
                cx.stop_propagation();
            }
            Key::Enter => {
                // Open the selected result and exit filter.
                let entries: Vec<_> =
                    b.fb.visible_entries()
                        .iter()
                        .map(|e| e.path.clone())
                        .collect();
                let selected = b.fb.selected();
                if let Some(path) = entries.get(selected).cloned() {
                    let is_dir = path.is_dir();
                    b.fb.clear_filter();
                    if is_dir {
                        b.fb.navigate_to(path);
                        cx.notify();
                    } else {
                        self.open_file(path);
                        cx.notify();
                    }
                } else {
                    b.fb.clear_filter();
                    cx.notify();
                }
                cx.stop_propagation();
            }
            Key::Backspace => {
                let mut text = b.fb.filter_text().to_string();
                if text.pop().is_some() {
                    b.fb.set_filter(&text);
                } else {
                    b.fb.clear_filter();
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Char(c) => {
                let mut text = b.fb.filter_text().to_string();
                text.push(c);
                b.fb.set_filter(&text);
                cx.notify();
                cx.stop_propagation();
            }
            Key::Down => {
                let count = b.fb.visible_entries().len();
                if count > 0 {
                    let sel = (b.fb.selected() + 1) % count;
                    b.fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Up => {
                let count = b.fb.visible_entries().len();
                if count > 0 {
                    let sel = if b.fb.selected() == 0 {
                        count - 1
                    } else {
                        b.fb.selected() - 1
                    };
                    b.fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    // ---- Rail (persistent side column, spec-rail.md) -----------------------

    /// `&mut` to the active tab's rail state, if a rail is open.
    pub(crate) fn rail_mut(&mut self) -> Option<&mut workspace::RailState> {
        self.workspace.active_tab_mut()?.rail.as_mut()
    }

    /// True when the active tab has a rail open AND it currently holds focus.
    pub(crate) fn rail_is_focused(&self) -> bool {
        self.workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .map(|r| r.focused)
            .unwrap_or(false)
    }

    /// Sync `rail.focused` after a focus-motion: the rail holds focus only
    /// when the newly focused leaf is the one the rail is pinned to.
    pub(crate) fn sync_rail_focus_after_motion(&mut self) {
        let Some(tab) = self.workspace.active_tab_mut() else {
            return;
        };
        let Some(rail) = tab.rail.as_mut() else {
            return;
        };
        rail.focused = tab.focused == rail.pinned_to;
    }

    /// Toggle the file-browser rail (Cmd-B). Two-state model (spec §5):
    /// - closed / different kind  → open-and-focus a file browser at cwd.
    /// - file-browser already open → close it, return focus to content.
    pub(crate) fn toggle_file_browser_rail(
        &mut self,
        _: &ToggleFileBrowserRail,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_file_browser_rail_impl(cx);
    }

    /// Toggle-logic for the file-browser rail, shared by the keybinding action
    /// and the command menu (`rail-files`).
    pub(crate) fn toggle_file_browser_rail_impl(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.active_tab_mut() else {
            return;
        };
        match &tab.rail {
            Some(r) if r.content.is_file_browser() => {
                tab.rail = None;
            }
            existing => {
                let side = existing.as_ref().map(|r| r.side).unwrap_or_default();
                let pinned_to = tab.focused;
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let content = workspace::RailContent::FileBrowser(FileBrowser::new(cwd));
                tab.rail = Some(workspace::RailState::new(content, side, pinned_to));
            }
        }
        self.save_workspace_state();
        cx.notify();
    }

    /// Toggle the outline rail (Cmd-Shift-O). Two-state model (spec §5). The
    /// heading list is derived lazily on render from the focused window.
    pub(crate) fn toggle_outline_rail(
        &mut self,
        _: &ToggleOutlineRail,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_outline_rail_impl(cx);
    }

    /// Toggle-logic for the outline rail, shared by the keybinding action and
    /// the command menu (`rail-outline`).
    pub(crate) fn toggle_outline_rail_impl(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.active_tab_mut() else {
            return;
        };
        match &tab.rail {
            Some(r) if r.content.is_outline() => {
                tab.rail = None;
            }
            existing => {
                let side = existing.as_ref().map(|r| r.side).unwrap_or_default();
                let pinned_to = tab.focused;
                let content = workspace::RailContent::Outline(workspace::OutlineState::new());
                tab.rail = Some(workspace::RailState::new(content, side, pinned_to));
            }
        }
        self.save_workspace_state();
        cx.notify();
    }

    /// Flip which edge the rail anchors to (Cmd-Shift-B). No-op when no rail
    /// is open. Persisted in the workspace snapshot.
    pub(crate) fn flip_rail_side(
        &mut self,
        _: &FlipRailSide,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flip_rail_side_impl(cx);
    }

    /// Flip-logic shared by the keybinding action and the command menu
    /// (`rail-flip`).
    pub(crate) fn flip_rail_side_impl(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            r.side = match r.side {
                workspace::RailSide::Left => workspace::RailSide::Right,
                workspace::RailSide::Right => workspace::RailSide::Left,
            };
            self.save_workspace_state();
            cx.notify();
        }
    }

    /// Close the rail and return focus to the previously-focused split-tree
    /// leaf (spec §7 — `tab.focused` is the single source of truth).
    pub(crate) fn rail_close(&mut self, _: &RailClose, _w: &mut Window, cx: &mut Context<Self>) {
        // If in worktree mode, Esc exits that overlay instead of closing the rail.
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
            && fb.worktree_mode.is_some()
        {
            fb.exit_worktree_mode();
            cx.notify();
            return;
        }
        if let Some(tab) = self.workspace.active_tab_mut()
            && tab.rail.is_some()
        {
            tab.rail = None;
            self.save_workspace_state();
            cx.notify();
        }
    }

    pub(crate) fn rail_down(&mut self, _: &RailDown, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => {
                    if let Some(wm) = &mut fb.worktree_mode {
                        wm.move_down();
                    } else {
                        fb.move_down();
                    }
                }
                workspace::RailContent::Outline(o) => {
                    if !o.entries.is_empty() {
                        o.selected = (o.selected + 1) % o.entries.len();
                    }
                }
            }
            cx.notify();
        }
    }

    pub(crate) fn rail_up(&mut self, _: &RailUp, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut() {
            match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => {
                    if let Some(wm) = &mut fb.worktree_mode {
                        wm.move_up();
                    } else {
                        fb.move_up();
                    }
                }
                workspace::RailContent::Outline(o) => {
                    if !o.entries.is_empty() {
                        o.selected = if o.selected == 0 {
                            o.entries.len() - 1
                        } else {
                            o.selected - 1
                        };
                    }
                }
            }
            cx.notify();
        }
    }

    /// Enter the selected rail entry. File browser: open a file (rail stays
    /// open) or navigate into a directory. Outline: scroll the focused window
    /// to the heading's block/line.
    pub(crate) fn rail_select(&mut self, _: &RailSelect, _w: &mut Window, cx: &mut Context<Self>) {
        // Worktree mode: select worktree and navigate.
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
            && fb.worktree_mode.is_some()
        {
            fb.select_worktree();
            cx.notify();
            return;
        }
        // File browser: collect the action without holding the rail borrow.
        let to_open = match self.rail_mut() {
            Some(r) => match &mut r.content {
                workspace::RailContent::FileBrowser(fb) => fb.enter_selected(),
                workspace::RailContent::Outline(_) => None,
            },
            None => return,
        };
        if let Some(path) = to_open {
            // Selecting a file opens it in the focused leaf; the rail stays
            // open but yields focus back to the content (spec §7, §12).
            // `open_file` replaces a transient Browser tile in place or
            // pushes a new tab otherwise.
            self.open_file(path);
            if let Some(r) = self.rail_mut() {
                r.focused = false;
            }
            cx.notify();
            return;
        }

        // Outline: jump the focused window to the selected heading.
        let target = self
            .workspace
            .active_tab()
            .and_then(|t| t.rail.as_ref())
            .and_then(|r| match &r.content {
                workspace::RailContent::Outline(o) => {
                    o.entries.get(o.selected).map(|(_, _, idx)| *idx)
                }
                _ => None,
            });
        if let Some(idx) = target {
            match self.workspace.focused_content_mut() {
                Some(App::Buffer(BufferApp::Viewing(d))) => {
                    d.cursor_block = idx.min(d.blocks.len().saturating_sub(1));
                    d.reveal_block(d.cursor_block);
                }
                Some(App::Buffer(BufferApp::Editing(e))) => {
                    let lines = e.editor.line_count();
                    let line = idx.min(lines.saturating_sub(1));
                    e.editor.set_cursor(line, 0);
                    // The Edit body is now a virtualized `gpui::list`; reveal
                    // through the ListState (the old ScrollHandle drove the
                    // pre-virtualization overflow container).
                    if line < e.list_item_count {
                        e.list_state.scroll_to_reveal_item(line);
                    }
                }
                _ => {}
            }
            cx.notify();
        }
    }

    pub(crate) fn rail_parent(&mut self, _: &RailParent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
        {
            if fb.worktree_mode.is_some() {
                return; // no-op in worktree mode
            }
            fb.go_parent();
            cx.notify();
        }
    }

    pub(crate) fn rail_worktrees(
        &mut self,
        _: &RailWorktrees,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
        {
            if fb.worktree_mode.is_some() {
                fb.exit_worktree_mode();
            } else {
                fb.enter_worktree_mode();
            }
            cx.notify();
        }
    }

    pub(crate) fn rail_toggle_hidden(
        &mut self,
        _: &RailToggleHidden,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
        {
            fb.toggle_hidden();
            cx.notify();
        }
    }

    pub(crate) fn rail_cycle_sort(
        &mut self,
        _: &RailCycleSort,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
        {
            fb.cycle_sort();
            cx.notify();
        }
    }

    pub(crate) fn rail_filter(&mut self, _: &RailFilter, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.rail_mut()
            && let workspace::RailContent::FileBrowser(fb) = &mut r.content
        {
            if fb.filter_mode {
                fb.clear_filter();
            } else {
                fb.filter_mode = true;
                fb.set_filter("");
            }
            cx.notify();
        }
    }

    /// Key-down handler for rail filter text input.
    pub(crate) fn handle_rail_filter_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        let Some(r) = self.rail_mut() else { return };
        let fb = match &mut r.content {
            workspace::RailContent::FileBrowser(fb) => fb,
            _ => return,
        };
        if !fb.filter_mode {
            return;
        }
        match press.key {
            Key::Esc => {
                fb.clear_filter();
                cx.notify();
                cx.stop_propagation();
            }
            Key::Enter => {
                let entries: Vec<_> = fb
                    .visible_entries()
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                let selected = fb.selected();
                if let Some(path) = entries.get(selected).cloned() {
                    let is_dir = path.is_dir();
                    fb.clear_filter();
                    if is_dir {
                        fb.navigate_to(path);
                        cx.notify();
                    } else {
                        self.open_file(path);
                        cx.notify();
                    }
                } else {
                    let Some(r) = self.rail_mut() else { return };
                    if let workspace::RailContent::FileBrowser(fb) = &mut r.content {
                        fb.clear_filter();
                    }
                    cx.notify();
                }
                cx.stop_propagation();
            }
            Key::Backspace => {
                let mut text = fb.filter_text().to_string();
                if text.pop().is_some() {
                    fb.set_filter(&text);
                } else {
                    fb.clear_filter();
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Char(c) => {
                let mut text = fb.filter_text().to_string();
                text.push(c);
                fb.set_filter(&text);
                cx.notify();
                cx.stop_propagation();
            }
            Key::Down => {
                let count = fb.visible_entries().len();
                if count > 0 {
                    let sel = (fb.selected() + 1) % count;
                    fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            Key::Up => {
                let count = fb.visible_entries().len();
                if count > 0 {
                    let sel = if fb.selected() == 0 {
                        count - 1
                    } else {
                        fb.selected() - 1
                    };
                    fb.set_selected(sel);
                }
                cx.notify();
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}
