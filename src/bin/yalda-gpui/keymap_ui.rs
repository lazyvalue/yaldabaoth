//! `App::Keymap` methods on `YaldaGpuiView`: open the tile, route keys, and
//! drive the live rebind. The bindings live on `self.keymap_registry` (the one
//! source of truth `register_keymap` also applies); rebinding mutates it,
//! re-applies the whole keymap to the app (`clear_key_bindings` + `bind_keys`),
//! and persists the overrides. The body render lives in `keymap_view.rs`.
//!
//! Rebind capture GRABS the keyboard: on `begin`, the app's keymap is cleared so
//! the chord the user presses is recorded instead of firing its old action;
//! commit / cancel re-apply the registry, restoring bindings (with the new one
//! in place on a successful commit).

use super::*;

impl YaldaGpuiView {
    /// Open the keybindings reference (Cmd-/). Replaces the focused tile's
    /// content with a fresh `App::Keymap`. No-op if already on one.
    pub(crate) fn open_keymap(&mut self, _: &OpenKeymap, _w: &mut Window, cx: &mut Context<Self>) {
        self.open_keymap_inner(cx);
    }

    pub(crate) fn open_keymap_inner(&mut self, cx: &mut Context<Self>) {
        if matches!(self.workspace.focused_content(), Some(App::Keymap(_))) {
            return;
        }
        self.set_screen(App::Keymap(KeymapTile::new()));
        cx.notify();
    }

    /// The cached body of the focused Keymap tile, if any.
    pub(crate) fn keymap_focused_view(&self) -> Option<Entity<KeymapView>> {
        match self.workspace.focused_content()? {
            App::Keymap(t) => t.view.clone(),
            _ => None,
        }
    }

    /// Number of rebindable rows currently visible (registry + the view's
    /// filter). Computed from `self.keymap_registry` directly — never a
    /// root-entity read — so it's safe to call from a `&mut self` handler.
    fn keymap_visible_count(&self, view: &Entity<KeymapView>, cx: &GpuiApp) -> usize {
        let filter = view.read(cx).filter().to_string();
        keymap_visible_order(&self.keymap_registry, &filter).len()
    }

    /// The registry entry the browse cursor is on (same visible order the body
    /// renders), or `None`.
    fn keymap_cursor_entry(&self, view: &Entity<KeymapView>, cx: &GpuiApp) -> Option<usize> {
        let (filter, cursor) = {
            let v = view.read(cx);
            (v.filter().to_string(), v.cursor())
        };
        keymap_visible_order(&self.keymap_registry, &filter)
            .get(cursor)
            .copied()
    }

    /// Rebuild the app keymap from the current registry (also the restore path
    /// after a capture grab).
    fn keymap_reapply(&self, cx: &mut Context<Self>) {
        self.keymap_registry.apply(cx);
    }

    /// Top-level key handler for a focused Keymap tile.
    pub(crate) fn handle_keymap_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.keymap_focused_view() else {
            return;
        };

        // Capture mode owns every keystroke — while it's active the app's keymap
        // is cleared, so nothing else can fire.
        if view.read(cx).is_capturing() {
            self.keymap_capture_key(&view, ev, cx);
            return;
        }

        // Global Cmd/Ctrl shortcuts fall through to their actions.
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        let press = keystroke_to_keypress(&ev.keystroke);

        if view.read(cx).mode() == KeymapMode::Filter {
            self.keymap_filter_key(&view, press, cx);
            return;
        }

        // Browse mode: universal leaders (space/./?) first, then vim keys.
        if self.leader_intercept(&press, cx) {
            return;
        }
        self.keymap_browse_key(&view, press, cx);
    }

    /// Filter mode: type into the search box; Enter keeps the filter and returns
    /// to Browse; Esc clears the filter and returns to Browse.
    fn keymap_filter_key(
        &mut self,
        view: &Entity<KeymapView>,
        press: KeyPress,
        cx: &mut Context<Self>,
    ) {
        match press.key {
            Key::Enter => view.update(cx, |kv, c| {
                kv.set_mode(KeymapMode::Browse);
                c.notify();
            }),
            Key::Esc => view.update(cx, |kv, c| {
                kv.clear_filter();
                kv.set_mode(KeymapMode::Browse);
                c.notify();
            }),
            Key::Backspace => view.update(cx, |kv, c| {
                kv.backspace_filter();
                c.notify();
            }),
            Key::Char(ch) => view.update(cx, |kv, c| {
                kv.push_filter(ch);
                c.notify();
            }),
            _ => {}
        }
    }

    /// Browse mode: vim navigation + rebind / reset commands.
    fn keymap_browse_key(
        &mut self,
        view: &Entity<KeymapView>,
        press: KeyPress,
        cx: &mut Context<Self>,
    ) {
        match press.key {
            Key::Char('j') | Key::Down => {
                let n = self.keymap_visible_count(view, cx);
                view.update(cx, |kv, c| {
                    kv.move_cursor(1, n);
                    c.notify();
                });
            }
            Key::Char('k') | Key::Up => {
                let n = self.keymap_visible_count(view, cx);
                view.update(cx, |kv, c| {
                    kv.move_cursor(-1, n);
                    c.notify();
                });
            }
            Key::Char('g') => view.update(cx, |kv, c| {
                kv.cursor_home();
                c.notify();
            }),
            Key::Char('G') => {
                let n = self.keymap_visible_count(view, cx);
                view.update(cx, |kv, c| {
                    kv.cursor_end(n);
                    c.notify();
                });
            }
            Key::PageDown => view.update(cx, |kv, c| {
                kv.scroll_by(400.0);
                c.notify();
            }),
            Key::PageUp => view.update(cx, |kv, c| {
                kv.scroll_by(-400.0);
                c.notify();
            }),
            Key::Char('/') | Key::Char('i') => view.update(cx, |kv, c| {
                kv.set_mode(KeymapMode::Filter);
                c.notify();
            }),
            Key::Enter | Key::Char('r') => self.keymap_begin_rebind(view, cx),
            Key::Char('x') | Key::Char('d') => self.keymap_reset_cursor(view, cx),
            Key::Char('R') => self.keymap_reset_all(cx),
            Key::Esc => view.update(cx, |kv, c| {
                if !kv.filter().is_empty() {
                    kv.clear_filter();
                    c.notify();
                }
            }),
            _ => {}
        }
    }

    /// Begin capturing a new binding for the cursor's entry. Grabs the keyboard
    /// by clearing the app keymap so the pressed chord is recorded, not fired.
    fn keymap_begin_rebind(&mut self, view: &Entity<KeymapView>, cx: &mut Context<Self>) {
        let Some(idx) = self.keymap_cursor_entry(view, cx) else {
            return;
        };
        cx.clear_key_bindings();
        view.update(cx, |kv, c| {
            kv.begin_capture(idx);
            c.notify();
        });
    }

    /// Reset the cursor's entry to its default binding; re-apply + persist.
    fn keymap_reset_cursor(&mut self, view: &Entity<KeymapView>, cx: &mut Context<Self>) {
        let Some(idx) = self.keymap_cursor_entry(view, cx) else {
            return;
        };
        self.keymap_registry.reset(idx);
        self.keymap_registry.persist();
        self.keymap_reapply(cx);
        view.update(cx, |_, c| c.notify());
    }

    /// Reset every binding to its default; re-apply + persist.
    fn keymap_reset_all(&mut self, cx: &mut Context<Self>) {
        self.keymap_registry.reset_all();
        self.keymap_registry.persist();
        self.keymap_reapply(cx);
        if let Some(view) = self.keymap_focused_view() {
            view.update(cx, |_, c| c.notify());
        }
    }

    /// A keystroke arriving while a rebind capture is active. Enter commits, Esc
    /// cancels, Backspace drops the last chord; anything else is recorded.
    fn keymap_capture_key(
        &mut self,
        view: &Entity<KeymapView>,
        ev: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let bare = !(m.control || m.alt || m.platform || m.shift || m.function);
        match ks.key.as_str() {
            "enter" if bare => self.keymap_commit_rebind(view, cx),
            "escape" if bare => self.keymap_cancel_rebind(view, cx),
            "backspace" if bare => view.update(cx, |kv, c| {
                kv.pop_chord();
                c.notify();
            }),
            _ => {
                let chord = ks.unparse();
                view.update(cx, |kv, c| {
                    kv.push_chord(chord);
                    c.notify();
                });
            }
        }
    }

    /// Commit the captured chords as the new binding. On success: mutate the
    /// registry, persist, and re-apply the keymap (restoring bindings with the
    /// new one live). On failure (empty / unparseable / would shadow itself):
    /// keep capturing and show the error — bindings stay grabbed.
    fn keymap_commit_rebind(&mut self, view: &Entity<KeymapView>, cx: &mut Context<Self>) {
        let cap = view.read(cx).capture().cloned();
        let Some(cap) = cap else {
            return;
        };
        if cap.chords.is_empty() {
            // Nothing captured — treat Enter as cancel.
            self.keymap_cancel_rebind(view, cx);
            return;
        }
        let keystrokes = cap.chords.join(" ");
        if self.keymap_registry.rebind(cap.idx, &keystrokes) {
            self.keymap_registry.persist();
            view.update(cx, |kv, c| {
                kv.cancel_capture();
                c.notify();
            });
            self.keymap_reapply(cx); // restores bindings, now with the override live
        } else {
            view.update(cx, |kv, c| {
                kv.capture_failed("those keys didn't parse — try again");
                c.notify();
            });
        }
    }

    /// Abandon the capture and restore the previous keymap unchanged.
    fn keymap_cancel_rebind(&mut self, view: &Entity<KeymapView>, cx: &mut Context<Self>) {
        view.update(cx, |kv, c| {
            kv.cancel_capture();
            c.notify();
        });
        self.keymap_reapply(cx);
    }

    // ── Menu-command entry points (space-menu on the keymap tile) ────────────

    pub(crate) fn keymap_menu_filter(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.keymap_focused_view() {
            view.update(cx, |kv, c| {
                kv.set_mode(KeymapMode::Filter);
                c.notify();
            });
        }
    }

    pub(crate) fn keymap_menu_rebind(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.keymap_focused_view() {
            self.keymap_begin_rebind(&view, cx);
        }
    }

    pub(crate) fn keymap_menu_reset(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.keymap_focused_view() {
            self.keymap_reset_cursor(&view, cx);
        }
    }

    pub(crate) fn keymap_menu_reset_all(&mut self, cx: &mut Context<Self>) {
        self.keymap_reset_all(cx);
    }

    /// True when the focused Keymap tile is capturing text (filter box) or a
    /// rebind — gates the universal leaders (`focused_in_insert_mode`).
    pub(crate) fn keymap_captures_text(&self, cx: &GpuiApp) -> bool {
        match self.keymap_focused_view() {
            Some(view) => {
                let v = view.read(cx);
                v.is_capturing() || v.mode() == KeymapMode::Filter
            }
            None => false,
        }
    }
}
