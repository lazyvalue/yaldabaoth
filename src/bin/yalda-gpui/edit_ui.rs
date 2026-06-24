//! Edit-view methods on YaldaGpuiView: entering/leaving edit + WP modes,
//! wiki-link open, reload-from-disk, key dispatch (insert/normal cores).
//! Extracted verbatim from main.rs (split-gpui-main, stage 2).

use super::*;

/// Lines the cursor moves per Ctrl-D / Ctrl-U (half page). A fixed sane
/// default: the dispatch site can't see the live viewport height, and the
/// render path scroll-reveals the cursor anyway, so an exact viewport-derived
/// count isn't required for correct behavior.
const HALF_PAGE_LINES: usize = 15;
/// Lines the cursor moves per Ctrl-F / Ctrl-B (full page).
const FULL_PAGE_LINES: usize = 30;

impl YaldaGpuiView {
    /// `Some(edit)` if currently editing, else `None`.
    pub(crate) fn edit_mut(&mut self) -> Option<&mut EditState> {
        match self
            .workspace
            .focused_content_mut()
            .expect("no focused window")
        {
            App::Buffer(BufferApp::Editing(e)) => Some(e),
            _ => None,
        }
    }

    /// Test-only: install a fresh Edit screen over `text` (Code view, Insert
    /// mode) so the headless harness can drive keystrokes through the real
    /// `build_edit_body_code` highlight path.
    #[cfg(test)]
    pub(crate) fn test_open_edit(&mut self, text: &str) {
        let core: workspace::SharedCore = std::rc::Rc::new(std::cell::RefCell::new(
            yalda::editor::EditorCore::new(text.to_string(), PathBuf::from("/tmp/harness.md")),
        ));
        let mut e = EditState::new(
            SharedEditor::new(1, core),
            "harness.md".into(),
            EditView::Code,
        );
        e.mode = EditMode::Insert;
        // Skip the boot splash so render() builds the real Edit body, not the
        // splash screen — the harness needs the highlight path to actually run.
        self.splash_until = None;
        self.set_screen(App::Buffer(BufferApp::Editing(e)));
    }

    /// Test-only: `(last_recomputed, last_was_skip)` of the focused Edit view's
    /// incremental highlight cache — the O(changed) latency-gate observable.
    #[cfg(test)]
    pub(crate) fn test_edit_cache_stats(&mut self) -> (usize, bool) {
        let e = self.edit_mut().expect("focused window is not an Edit view");
        (
            e.highlight_cache.last_recomputed,
            e.highlight_cache.last_was_skip,
        )
    }

    /// Test-only: install a fresh Doc screen rendering `blocks` so the headless
    /// harness can drive the real virtualized doc body. Skips the boot splash
    /// (otherwise `render()` builds the splash screen, not the doc list) and
    /// resets the per-frame block-build counter so the latency gate measures
    /// from a clean slate.
    #[cfg(test)]
    pub(crate) fn test_open_doc(&mut self, markdown: &str) {
        let blocks = render_with_wiki(markdown, &self.theme, None);
        self.set_screen(App::Buffer(BufferApp::Viewing(DocState::viewing(
            blocks,
            SharedString::new_static("harness.md"),
            None,
        ))));
        // The real doc body only renders once the splash deadline passes; clear
        // it so the harness exercises the list path immediately.
        self.splash_until = None;
        Self::test_reset_doc_block_builds();
    }

    /// Test-only: zero the virtualized-doc block-build counter.
    #[cfg(test)]
    pub(crate) fn test_reset_doc_block_builds() {
        DOC_BLOCK_BUILDS.with(|c| c.set(0));
    }

    /// Test-only: how many `block_element`s the doc list built since the last
    /// reset — the O(visible) latency-gate observable.
    #[cfg(test)]
    pub(crate) fn test_doc_block_builds() -> usize {
        DOC_BLOCK_BUILDS.with(|c| c.get())
    }

    /// Test-only: clear the doc render-decision tap (call before the frame to
    /// measure).
    #[cfg(test)]
    pub(crate) fn test_reset_doc_render_tap() {
        DOC_RENDER_TAP.with(|t| *t.borrow_mut() = DocRenderTap::default());
    }

    /// Test-only: snapshot the doc render-decision tap — what the last frame(s)
    /// since reset decided to paint / select / cursor-bar.
    #[cfg(test)]
    pub(crate) fn test_doc_render_tap() -> DocRenderTap {
        DOC_RENDER_TAP.with(|t| t.borrow().clone())
    }

    /// Swap from Doc view into Edit screen with the Code (raw markdown) view.
    pub(crate) fn enter_edit(&mut self, _: &EnterEdit, _w: &mut Window, cx: &mut Context<Self>) {
        self.enter_edit_with(EditView::Code, cx);
    }

    /// Swap from Doc view into Edit screen with the Word-Processor (live
    /// preview) view. Bound to `Ctrl-W` in the YaldaView key context.
    pub(crate) fn enter_wp(&mut self, _: &EnterWp, _w: &mut Window, cx: &mut Context<Self>) {
        self.enter_edit_with(EditView::WordProcessor, cx);
    }

    /// Common entry point: restore the cached EditState if one exists (so
    /// unsaved edits survive the round-trip) or build a fresh editor from
    /// disk. The chosen `view` is applied either way — switching from Code
    /// → WP without losing cursor/buffer state is just `cached.view = view`.
    pub(crate) fn enter_edit_with(&mut self, view: EditView, cx: &mut Context<Self>) {
        // 5c: bind the Edit view to the Doc's SHARED pooled core (same text +
        // undo), so edits show live in any Doc tile of the file and there's no
        // stash to shuttle. Snapshot the (id, core) without holding the borrow
        // across the pool mutation below.
        let (shared, label): (
            Option<(workspace::FileBufferId, workspace::SharedCore)>,
            SharedString,
        ) = match self.workspace.focused_content_mut() {
            Some(App::Buffer(BufferApp::Viewing(d))) => (
                d.source.as_ref().map(|s| (s.buffer_id, s.core.clone())),
                d.file_label.clone(),
            ),
            _ => return,
        };
        let (id, core) = match shared {
            Some(pair) => pair,
            None => {
                // Source-less Doc (string-backed, or not yet pool-bound): open
                // the file by label and bind a fresh pooled core.
                let path: PathBuf = label.to_string().into();
                match self.workspace.open_and_retain(&path) {
                    Ok(pair) => pair,
                    Err(_) => return,
                }
            }
        };
        let mut edit_state = EditState::new(SharedEditor::new(id, core), label, view);
        edit_state.view = view;
        self.set_screen(App::Buffer(BufferApp::Editing(edit_state)));
        cx.notify();
    }

    /// Edit → Doc round trip. The new Doc keeps the SAME pooled core (5c), so
    /// it shows the buffer's *current* (unsaved) text and shares undo with any
    /// other view of the file. No stash — the shared core IS the live state.
    /// (Step-2 TODO: stash the EditorView cursor so re-entering Edit lands
    /// where the user left off; today the cursor resets to the top.)
    pub(crate) fn back_to_doc(&mut self, cx: &mut Context<Self>) {
        let prev = self
            .workspace
            .replace_focused_content(
                // Placeholder; overwritten in every match arm below.
                App::Buffer(BufferApp::Viewing(DocState::viewing(
                    Vec::new(),
                    SharedString::new_static(""),
                    None,
                ))),
            )
            .expect("workspace has no focused window");
        match prev {
            App::Buffer(BufferApp::Editing(edit)) => {
                let edit_path = PathBuf::from(edit.file_label.as_ref());
                let blocks =
                    render_with_wiki(&edit.editor.full_text(), &self.theme, Some(&edit_path));
                let file_label = edit.file_label.clone();
                // 5c: the new Doc keeps the SAME pooled core the Edit view held
                // (shared text + undo). No stash — the core IS the live state.
                let source = DocSource::new(edit.editor.buffer_id, edit.editor.core.clone());
                self.set_screen(App::Buffer(BufferApp::Viewing(DocState::viewing(
                    blocks,
                    file_label,
                    Some(source),
                ))));
            }
            other => {
                self.set_screen(other);
                return;
            }
        }
        cx.notify();
    }

    /// Resolve a wiki link target (e.g. `notes`, `subdir/topic`) against
    /// the source doc's directory and replace the focused tile with the
    /// resulting Doc. Lookup order:
    ///   1. `<doc_dir>/<target>.md` — markdown convention; matches what
    ///      Obsidian / Foam / most wiki-aware editors do.
    ///   2. `<doc_dir>/<target>` — literal path, in case the user included
    ///      the extension already (or wants a non-md file).
    ///
    /// If neither exists, log to stderr and no-op (the tile stays put;
    /// nothing to navigate to).
    pub(crate) fn open_wiki_link(
        &mut self,
        target: &str,
        doc_dir: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) {
        let target = target.trim();
        if target.is_empty() {
            return;
        }
        let bases: Vec<PathBuf> = match doc_dir {
            Some(d) => vec![d.to_path_buf()],
            None => vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))],
        };
        let mut resolved: Option<PathBuf> = None;
        for base in &bases {
            let with_md = base.join(format!("{target}.md"));
            if with_md.is_file() {
                resolved = Some(with_md);
                break;
            }
            let bare = base.join(target);
            if bare.is_file() {
                resolved = Some(bare);
                break;
            }
        }
        let Some(path) = resolved else {
            eprintln!("wiki link: no file found for [[{}]]", target);
            return;
        };
        let canon = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();
        let label: SharedString = canon.into();
        // 5c: bind the wiki-link target Doc to its pooled core (dedup by path),
        // so it shares text/undo with any Edit view and live-tracks.
        let (buf_id, core) = match self.workspace.open_and_retain(&path) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("wiki link: cannot open {}: {err}", path.display());
                return;
            }
        };
        let blocks = render_with_wiki(
            &core.borrow().document().full_text(),
            &self.theme,
            Some(&path),
        );
        self.set_screen(App::Buffer(BufferApp::Viewing(DocState::viewing(
            blocks,
            label,
            Some(DocSource::new(buf_id, core)),
        ))));
        self.doc_selection = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Re-read the focused window's file from disk and rebuild its content,
    /// discarding any unsaved buffer state. Doc view: re-renders blocks and
    /// resets scroll/cursor (file may have shifted out from under the user).
    /// Edit view: replaces the Editor with a fresh one over the same path.
    /// Browser / Claude windows: no-op — there's no on-disk file to revert
    /// to. Read failures log to stderr (consistent with the existing open
    /// path) and leave the buffer untouched.
    pub(crate) fn reload_focused_from_disk(&mut self, cx: &mut Context<Self>) {
        // Extract the path (and, for Edit, the shared core handle) from the
        // focused window without holding a mutable borrow across file I/O +
        // workspace mutation.
        enum FocusKind {
            Doc(
                Option<(workspace::FileBufferId, workspace::SharedCore)>,
                PathBuf,
                SharedString,
            ),
            Edit(workspace::SharedCore, PathBuf),
        }
        let focus_kind = match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(d))) => FocusKind::Doc(
                d.source.as_ref().map(|s| (s.buffer_id, s.core.clone())),
                PathBuf::from(d.file_label.as_ref()),
                d.file_label.clone(),
            ),
            Some(App::Buffer(BufferApp::Editing(e))) => FocusKind::Edit(
                std::rc::Rc::clone(&e.editor.core),
                PathBuf::from(e.file_label.as_ref()),
            ),
            _ => return,
        };
        match focus_kind {
            // Edit reload resets the SHARED core in place, so every view of
            // the file (splits, also-shown tiles) sees the disk version — not
            // a fresh, un-shared buffer. The tile keeps its own cursor/scroll
            // and Code/WP sub-view (we never replace the EditState itself).
            FocusKind::Edit(core, path) => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(err) => {
                        eprintln!("reload: cannot read {}: {}", path.display(), err);
                        return;
                    }
                };
                *core.borrow_mut() = EditorCore::new(text, path);
                // The text may have shrunk; reset the focused view's cursor to
                // the top so it can't dangle past the new end (matches the old
                // reload-replaces-editor behavior). Other shared views keep
                // their own cursors.
                if let Some(App::Buffer(BufferApp::Editing(e))) =
                    self.workspace.focused_content_mut()
                {
                    e.editor.set_cursor(0, 0);
                    e.editor.clear_selection();
                }
            }
            // Doc reload: for a pool-bound Doc, reset the SHARED core to the
            // disk version in place (reverts every view of the file, like the
            // Edit path); for a legacy non-pooled Doc, render a fresh snapshot.
            FocusKind::Doc(pooled, path, label) => {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(err) => {
                        eprintln!("reload: cannot read {}: {}", path.display(), err);
                        return;
                    }
                };
                let (blocks, source) = match pooled {
                    Some((id, core)) => {
                        *core.borrow_mut() = EditorCore::new(text, path.clone());
                        let blocks = render_with_wiki(
                            &core.borrow().document().full_text(),
                            &self.theme,
                            Some(&path),
                        );
                        (blocks, Some(DocSource::new(id, core)))
                    }
                    None => {
                        let doc = Document::from_text(text, path.clone());
                        let blocks = render_with_wiki(&doc.full_text(), &self.theme, Some(&path));
                        (blocks, None)
                    }
                };
                self.set_screen(App::Buffer(BufferApp::Viewing(DocState::viewing(
                    blocks, label, source,
                ))));
            }
        }
        self.doc_selection = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Dispatch a key in Edit mode. Insert mode handles raw text input;
    /// Normal mode routes through the shared `KeybindManager` to map the
    /// keystroke to an action name, then this method dispatches a small
    /// subset of actions against the editor. `Ctrl-S` (save) and `Ctrl-V`
    /// (back to Doc view) are caught here before mode dispatch so they
    /// behave identically in both Insert and Normal.
    pub(crate) fn handle_edit_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);

        // Universal leaders: in normal mode, `<space>`/`.`/`?` open the menus
        // with top priority (insert mode keeps them as text).
        if self.leader_intercept(&press, cx) {
            return;
        }

        // Mode-independent shortcuts.
        if press.modifiers.contains(KMods::CONTROL)
            && let Key::Char(c) = press.key
        {
            match c {
                's' | 'S' => {
                    self.save_buffer(cx);
                    return;
                }
                'v' | 'V' => {
                    self.back_to_doc(cx);
                    return;
                }
                'w' | 'W' => {
                    self.toggle_edit_view(cx);
                    return;
                }
                _ => {}
            }
        }

        let mode = match self.edit_mut() {
            Some(e) => e.mode,
            None => return,
        };

        // Tab/Shift-Tab in normal mode cycle buffers.
        if mode == EditMode::Normal {
            match press.key {
                Key::Tab => {
                    if self.workspace.tabs.len() > 1 {
                        let next = (self.workspace.active_tab + 1) % self.workspace.tabs.len();
                        self.switch_to_buffer(next);
                        cx.notify();
                    }
                    return;
                }
                Key::BackTab => {
                    if self.workspace.tabs.len() > 1 {
                        let prev = if self.workspace.active_tab == 0 {
                            self.workspace.tabs.len() - 1
                        } else {
                            self.workspace.active_tab - 1
                        };
                        self.switch_to_buffer(prev);
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // In normal mode, intercept bare `m`/`'` to start a mark chord.
        if mode == EditMode::Normal && self.try_start_mark_chord(&press.key, &press.modifiers, cx) {
            return;
        }

        match mode {
            EditMode::Insert => self.dispatch_insert(press, cx),
            EditMode::Normal => self.dispatch_normal(press, cx),
        }
    }

    /// Flip between Code and WordProcessor views without touching buffer
    /// state. Bound to `Ctrl-W`.
    pub(crate) fn toggle_edit_view(&mut self, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        edit.view = match edit.view {
            EditView::Code => EditView::WordProcessor,
            EditView::WordProcessor => EditView::Code,
        };
        cx.notify();
    }

    /// Save the current edit buffer; record the outcome on `last_save_msg`
    /// so the footer can surface it. No-op if the screen isn't Edit.
    pub(crate) fn save_buffer(&mut self, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        let msg: SharedString = match edit.editor.save() {
            Ok(()) => "saved".into(),
            Err(e) => format!("save failed: {}", e).into(),
        };
        edit.last_save_msg = Some(msg);
        cx.notify();
    }

    pub(crate) fn dispatch_insert(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        // Any non-save key invalidates the transient save message.
        edit.last_save_msg = None;
        let was_insert = edit.mode == EditMode::Insert;
        Self::dispatch_insert_core(&mut edit.editor, &mut edit.mode, press);
        // Track the `.` (last-edit) mark on any text-producing key in insert mode.
        if was_insert && let Some(wid) = self.workspace.focused_window_id() {
            self.workspace.marks.last_edit = Some(wid);
        }
        cx.notify();
    }

    /// Insert-mode dispatch on raw `(editor, mode)` references — shared by
    /// the Edit screen and the Claude (ACP) screen so both have the same
    /// typing semantics. Unlike the wrapper above, this does not call
    /// `cx.notify()` — the caller must.
    pub(crate) fn dispatch_insert_core<E: EditOps>(
        editor: &mut E,
        mode: &mut EditMode,
        press: KeyPress,
    ) {
        match press.key {
            Key::Esc => {
                editor.end_insert();
                *mode = EditMode::Normal;
                // Vim convention: cursor steps back one column on leaving insert.
                if editor.cursor().col > 0 {
                    editor.cursor_move_left();
                }
            }
            Key::Enter => {
                match list_continuation_action(editor) {
                    Some(ListContinuation::Continue(prefix)) => {
                        editor.insert_char('\n');
                        for ch in prefix.chars() {
                            editor.insert_char(ch);
                        }
                    }
                    Some(ListContinuation::Terminate) => {
                        // Enter on an empty list item ends the list: wipe the
                        // dangling marker, then drop to a fresh blank line.
                        let col = editor.cursor().col;
                        for _ in 0..col {
                            editor.backspace();
                        }
                        editor.insert_char('\n');
                    }
                    None => editor.insert_char('\n'),
                }
            }
            Key::Backspace => {
                editor.backspace();
            }
            Key::Tab => {
                editor.insert_char(' ');
                editor.insert_char(' ');
            }
            Key::Char(c) => {
                if press.modifiers.contains(KMods::CONTROL) {
                    // Ignore ctrl-chords in insert mode for the MVP; only
                    // bare typed chars produce text.
                    return;
                }
                editor.insert_char(c);
            }
            _ => {}
        }
    }

    pub(crate) fn dispatch_normal(&mut self, press: KeyPress, cx: &mut Context<Self>) {
        let edit = match self.edit_mut() {
            Some(e) => e,
            None => return,
        };
        edit.last_save_msg = None;

        // `r{char}` replace-char chord. `r` arms `pending_replace`; the next
        // keypress is consumed as the replacement (Esc / non-char cancels).
        if edit.pending_replace {
            edit.pending_replace = false;
            if let Key::Char(c) = press.key
                && !press.modifiers.contains(KMods::CONTROL)
            {
                edit.editor.replace_char_at_cursor(c);
            }
            cx.notify();
            return;
        }
        if press.key == Key::Char('r') && press.modifiers.is_empty() {
            edit.pending_replace = true;
            edit.last_save_msg = Some("replace".into());
            cx.notify();
            return;
        }

        match Self::dispatch_normal_core(
            &mut edit.editor,
            &mut edit.mode,
            &mut edit.keybinds,
            press,
        ) {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                edit.last_save_msg = Some("yanked".into());
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
            NormalOutcome::Paste { before } => {
                if let Some(e) = self.edit_mut() {
                    if Self::apply_paste(&mut e.editor, before) {
                        e.last_save_msg = Some("put".into());
                    }
                    cx.notify();
                }
            }
        }
    }

    /// Normal-mode dispatch on raw `(editor, mode, keybinds)` references —
    /// shared by the Edit screen and the Claude (ACP) screen. Caller is
    /// responsible for `cx.notify()` and any post-action status messaging
    /// based on the returned `NormalOutcome`.
    pub(crate) fn dispatch_normal_core<E: EditOps>(
        editor: &mut E,
        mode: &mut EditMode,
        keybinds: &mut KeybindManager,
        press: KeyPress,
    ) -> NormalOutcome {
        // Esc clears any active selection and exits extend mode.
        if press.key == Key::Esc {
            editor.set_extend_mode(false);
            editor.clear_selection();
            return NormalOutcome::Handled;
        }

        let action_name = match keybinds.process_key(press) {
            Some(name) => name,
            None => return NormalOutcome::Skipped,
        };
        // Numeric count prefix typed ahead of this action (e.g. `42` in
        // `42G`). Taken-and-cleared here; arms that don't use it ignore it.
        let count = keybinds.take_count();

        match action_name.as_str() {
            // ---- Pure motions: collapse selection (or extend in extend mode) ----
            "move-down" => {
                editor.pre_move(false);
                editor.move_down(false);
            }
            "move-up" => {
                editor.pre_move(false);
                editor.cursor_move_up();
                editor.clamp_cursor_col(false);
            }
            "move-left" => {
                editor.pre_move(false);
                editor.cursor_move_left();
            }
            "move-right" => {
                editor.pre_move(false);
                editor.move_right_clamped(false);
            }
            "move-line-start" => {
                editor.pre_move(false);
                editor.cursor_move_line_start();
            }
            "move-line-first-non-blank" => {
                editor.pre_move(false);
                editor.move_cursor_first_non_blank();
            }
            "move-line-end" => {
                editor.pre_move(false);
                editor.move_cursor_line_end(false);
            }
            // ---- Word motions: create a fresh selection from cursor → motion target ----
            "move-word-forward" => {
                editor.pre_move(true);
                editor.move_cursor_word_forward();
            }
            "move-word-backward" => {
                editor.pre_move(true);
                editor.move_cursor_word_backward();
            }
            "move-word-end" => {
                editor.pre_move(true);
                editor.move_cursor_word_end();
            }
            // ---- Doc-level jumps ----
            "goto-top" => {
                editor.pre_move(false);
                // `<count>gg` jumps to line `count` (1-indexed); bare `gg`
                // goes to the top.
                match count {
                    Some(n) => editor.jump_to_line(n.saturating_sub(1)),
                    None => editor.cursor_jump_top(),
                }
            }
            "goto-bottom" => {
                editor.pre_move(false);
                // `<count>G` jumps to line `count` (1-indexed); bare `G`
                // goes to the last line.
                match count {
                    Some(n) => editor.jump_to_line(n.saturating_sub(1)),
                    None => editor.jump_cursor_bottom(),
                }
            }
            // ---- Half / full page paging ----
            // The Edit + Agent render paths both scroll-to-reveal the cursor
            // line every frame, so paging is just a cursor move by N lines;
            // no viewport-height plumbing is needed at this `self`-less site.
            "half-page-down" => {
                editor.pre_move(false);
                Self::page_cursor(editor, HALF_PAGE_LINES as isize);
            }
            "half-page-up" => {
                editor.pre_move(false);
                Self::page_cursor(editor, -(HALF_PAGE_LINES as isize));
            }
            "full-page-down" => {
                editor.pre_move(false);
                Self::page_cursor(editor, FULL_PAGE_LINES as isize);
            }
            "full-page-up" => {
                editor.pre_move(false);
                Self::page_cursor(editor, -(FULL_PAGE_LINES as isize));
            }
            // ---- Put (paste) — deferred to the caller for clipboard access ----
            "paste" => return NormalOutcome::Paste { before: false },
            "paste-before" => return NormalOutcome::Paste { before: true },
            // ---- Mode switches ----
            "insert-mode" => {
                if let Some(((sl, sc), _)) = editor.selection_range() {
                    editor.cursor_set(sl, sc);
                    editor.clear_selection();
                }
                editor.set_extend_mode(false);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "insert-after" => {
                if let Some((_, (el, ec))) = editor.selection_range() {
                    let line_len = editor.line_len_chars(el);
                    let new_col = if ec < line_len { ec + 1 } else { ec };
                    editor.cursor_set(el, new_col);
                    editor.clear_selection();
                } else {
                    editor.move_right_clamped(true);
                }
                editor.set_extend_mode(false);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "open-line-below" => {
                editor.open_line_below();
                *mode = EditMode::Insert;
            }
            "open-line-above" => {
                editor.open_line_above();
                *mode = EditMode::Insert;
            }
            // ---- Helix selection actions ----
            "delete-selection" => Self::yank_then_delete_selection(editor),
            "change-selection" => {
                Self::yank_then_delete_selection(editor);
                editor.begin_insert();
                *mode = EditMode::Insert;
            }
            "yank-selection" => {
                let text = match editor.yank_selection() {
                    Some(t) if !t.is_empty() => t,
                    _ => editor
                        .line_text_at_cursor()
                        .trim_end_matches('\n')
                        .to_string(),
                };
                Self::yank_to_clipboard(&text);
                return NormalOutcome::Yanked;
            }
            "collapse-selection" => editor.collapse_selection(),
            "flip-selection" => editor.flip_selection(),
            "select-all" => editor.select_all(),
            "extend-line" => editor.extend_by_line(),
            "toggle-extend-mode" => {
                editor.toggle_extend_mode();
                if editor.extend_mode() && editor.selection_anchor().is_none() {
                    editor.anchor_at_cursor();
                }
            }
            // ---- Direct-edit actions (still callable via custom config) ----
            "delete-char" => {
                if let Some(t) = char_under_cursor(editor) {
                    Self::yank_to_clipboard(&t);
                }
                editor.delete_char_at_cursor();
            }
            "delete-line" => {
                let line = editor.line_text_at_cursor();
                if !line.is_empty() {
                    Self::yank_to_clipboard(&line);
                }
                editor.delete_current_line();
            }
            "undo" => {
                editor.undo();
            }
            "redo" => {
                editor.redo();
            }
            "quit" | "force-quit" => return NormalOutcome::Quit,
            "open-menu" => return NormalOutcome::OpenMenu,
            _ => return NormalOutcome::Skipped,
        }
        NormalOutcome::Handled
    }

    /// Move the cursor by `delta` lines (negative = up), clamped to the
    /// document bounds, resetting column to a clamped position on the new
    /// line. Shared by the half/full-page paging actions.
    fn page_cursor<E: EditOps>(editor: &mut E, delta: isize) {
        let cur = editor.cursor().line as isize;
        let last = editor.line_count().saturating_sub(1) as isize;
        let target = (cur + delta).clamp(0, last.max(0)) as usize;
        editor.jump_to_line(target);
    }

    /// Vim default-register semantics for a delete: copy the about-to-be-
    /// deleted text to the clipboard (yalda's yank buffer) before removing it,
    /// so a subsequent `p`/`P` puts it back. Deletes the active selection, or
    /// the single character under the cursor when there's no selection.
    fn yank_then_delete_selection<E: EditOps>(editor: &mut E) {
        if editor.selection_anchor().is_some() {
            if let Some(t) = editor.yank_selection().filter(|s| !s.is_empty()) {
                Self::yank_to_clipboard(&t);
            }
            editor.delete_selection();
        } else {
            if let Some(t) = char_under_cursor(editor) {
                Self::yank_to_clipboard(&t);
            }
            editor.delete_char_at_cursor();
        }
    }

    /// Charwise put of `text` at (P, `before=true`) or just after (p,
    /// `before=false`) the cursor. Charwise because yalda's yank stores raw
    /// text in the system clipboard with no linewise-vs-charwise register
    /// metadata. Leaves the cursor on the last inserted character (vim
    /// convention). Returns false if there was nothing to insert.
    fn put_text<E: EditOps>(editor: &mut E, text: &str, before: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        // For `p`, start inserting after the cursor's char (unless the line
        // is empty / cursor already past end). `begin_insert`/`end_insert`
        // bracket the splice so it lands as one undo group, matching how
        // insert-mode typing is grouped.
        if !before {
            let line = editor.cursor().line;
            if editor.line_len_chars(line) > 0 {
                editor.move_right_clamped(true);
            }
        }
        editor.begin_insert();
        for ch in text.chars() {
            editor.insert_char(ch);
        }
        editor.end_insert();
        // Step back onto the last inserted char (cursor sits one past it).
        if editor.cursor().col > 0 {
            editor.cursor_move_left();
        }
        true
    }

    /// Resolve a [`NormalOutcome::Paste`] by reading the system clipboard and
    /// putting it into `editor`. Shared by the Edit and Agent dispatch sites.
    pub(crate) fn apply_paste<E: EditOps>(editor: &mut E, before: bool) -> bool {
        match Self::read_from_clipboard() {
            Some(text) => Self::put_text(editor, &text, before),
            None => false,
        }
    }
}

/// What pressing Enter should do on a list/TODO line.
pub(crate) enum ListContinuation {
    /// Start the next item with this prefix (indent + marker).
    Continue(String),
    /// The current item is empty — clear its marker and break the list.
    Terminate,
}

/// Decide whether an Enter keypress on the cursor's line should auto-continue a
/// markdown list / TODO / blockquote. Returns `None` for ordinary lines (plain
/// newline) and when the cursor isn't at end-of-line — splitting mid-line keeps
/// the naive behavior to avoid surprising the typist.
/// The single character under the cursor as an owned string, used to seed the
/// yank buffer on a vim-style delete. `None` on an empty line or when the
/// cursor sits past end-of-line (nothing to delete/yank).
pub(crate) fn char_under_cursor<E: EditOps>(editor: &E) -> Option<String> {
    let raw = editor.line_text_at_cursor();
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    line.chars().nth(editor.cursor().col).map(|c| c.to_string())
}

pub(crate) fn list_continuation_action<E: EditOps>(editor: &E) -> Option<ListContinuation> {
    let cur = editor.cursor();
    let raw = editor.line_text_at_cursor();
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    // Only continue from the end of the line's content.
    if cur.col < line.chars().count() {
        return None;
    }
    let indent_len = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let indent: String = line.chars().take(indent_len).collect();
    let rest: String = line.chars().skip(indent_len).collect();
    let (marker_chars, continuation) = parse_list_marker(&rest)?;
    let content: String = rest.chars().skip(marker_chars).collect();
    if content.trim().is_empty() {
        return Some(ListContinuation::Terminate);
    }
    Some(ListContinuation::Continue(format!("{indent}{continuation}")))
}

/// Given the post-indent remainder of a line, recognize a leading list marker.
/// Returns `(chars consumed by the marker, the prefix to start the next item)`.
/// Checkbox items reset to unchecked; ordered items increment.
fn parse_list_marker(rest: &str) -> Option<(usize, String)> {
    let chars: Vec<char> = rest.chars().collect();
    // Bullet markers: `-`, `*`, `+` followed by a space.
    if chars.len() >= 2 && matches!(chars[0], '-' | '*' | '+') && chars[1] == ' ' {
        let bullet = chars[0];
        // Checkbox: `- [ ] ` / `- [x] ` / `- [X] `.
        if chars.len() >= 6
            && chars[2] == '['
            && matches!(chars[3], ' ' | 'x' | 'X')
            && chars[4] == ']'
            && chars[5] == ' '
        {
            return Some((6, format!("{bullet} [ ] ")));
        }
        return Some((2, format!("{bullet} ")));
    }
    // Blockquote: `> `.
    if chars.len() >= 2 && chars[0] == '>' && chars[1] == ' ' {
        return Some((2, "> ".to_string()));
    }
    // Ordered list: digits then `.` or `)` then a space.
    let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && chars.len() >= digits + 2
        && matches!(chars[digits], '.' | ')')
        && chars[digits + 1] == ' '
    {
        let sep = chars[digits];
        let n: u64 = rest[..digits].parse().ok()?;
        return Some((digits + 2, format!("{}{sep} ", n.saturating_add(1))));
    }
    None
}

#[cfg(test)]
mod list_continuation_tests {
    use super::parse_list_marker;

    fn cont(rest: &str) -> Option<String> {
        parse_list_marker(rest).map(|(_, c)| c)
    }

    #[test]
    fn unchecked_todo_continues_unchecked() {
        assert_eq!(cont("- [ ] buy milk").as_deref(), Some("- [ ] "));
    }

    #[test]
    fn checked_todo_resets_to_unchecked() {
        assert_eq!(cont("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(cont("- [X] done").as_deref(), Some("- [ ] "));
    }

    #[test]
    fn bullets_preserve_their_marker() {
        assert_eq!(cont("- item").as_deref(), Some("- "));
        assert_eq!(cont("* item").as_deref(), Some("* "));
        assert_eq!(cont("+ item").as_deref(), Some("+ "));
    }

    #[test]
    fn star_todo_keeps_star_bullet() {
        assert_eq!(cont("* [ ] task").as_deref(), Some("* [ ] "));
    }

    #[test]
    fn ordered_lists_increment() {
        assert_eq!(cont("1. first").as_deref(), Some("2. "));
        assert_eq!(cont("9. nth").as_deref(), Some("10. "));
        assert_eq!(cont("3) paren").as_deref(), Some("4) "));
    }

    #[test]
    fn blockquote_continues() {
        assert_eq!(cont("> quote").as_deref(), Some("> "));
    }

    #[test]
    fn non_list_lines_dont_continue() {
        assert_eq!(cont("plain text"), None);
        assert_eq!(cont("# heading"), None);
        assert_eq!(cont("-no space"), None);
        assert_eq!(cont("---"), None);
        assert_eq!(cont(""), None);
    }

    #[test]
    fn marker_char_count_matches_marker_length() {
        assert_eq!(parse_list_marker("- [ ] x").unwrap().0, 6);
        assert_eq!(parse_list_marker("- x").unwrap().0, 2);
        assert_eq!(parse_list_marker("12. x").unwrap().0, 4);
    }
}
