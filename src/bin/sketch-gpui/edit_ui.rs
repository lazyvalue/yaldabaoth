//! Edit-view methods on SketchGpuiView: entering/leaving edit + WP modes,
//! wiki-link open, reload-from-disk, key dispatch (insert/normal cores).
//! Extracted verbatim from main.rs (split-gpui-main, stage 2).

use super::*;

impl SketchGpuiView {
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
            sketch::editor::EditorCore::new(text.to_string(), PathBuf::from("/tmp/harness.md")),
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
        self.set_screen(App::Buffer(BufferApp::Viewing(DocState {
            blocks,
            file_label: SharedString::new_static("harness.md"),
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            source: None,
        })));
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
    /// preview) view. Bound to `Ctrl-W` in the SketchView key context.
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
                App::Buffer(BufferApp::Viewing(DocState {
                    blocks: Vec::new(),
                    file_label: SharedString::new_static(""),
                    cursor_block: 0,
                    list_state: DocState::new_list_state(0),
                    list_item_count: std::cell::Cell::new(0),
                    blocks_seq: 0,
                    blocks_snapshot: RefCell::new(None),
                    last_cursor_block: std::cell::Cell::new(None),
                    source: None,
                })),
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
                self.set_screen(App::Buffer(BufferApp::Viewing(DocState {
                    blocks,
                    file_label,
                    cursor_block: 0,
                    list_state: DocState::new_list_state(0),
                    list_item_count: std::cell::Cell::new(0),
                    blocks_seq: 0,
                    blocks_snapshot: RefCell::new(None),
                    last_cursor_block: std::cell::Cell::new(None),
                    source: Some(source),
                })));
            }
            App::Agent(ring) => {
                // B6: restore the Buffer the user opened Claude from. If none
                // was stashed, fall back to a fresh Picking at cwd — never
                // close the tile. AgentRing and all its sessions drop here,
                // taking pump tasks and ACP channels with them.
                let buffer = match ring.underlying {
                    Some(boxed) => *boxed,
                    None => BufferApp::Picking(BrowserWindow::standalone(
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    )),
                };
                self.set_screen(App::Buffer(buffer));
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
        self.set_screen(App::Buffer(BufferApp::Viewing(DocState {
            blocks,
            file_label: label,
            cursor_block: 0,
            list_state: DocState::new_list_state(0),
            list_item_count: std::cell::Cell::new(0),
            blocks_seq: 0,
            blocks_snapshot: RefCell::new(None),
            last_cursor_block: std::cell::Cell::new(None),
            source: Some(DocSource::new(buf_id, core)),
        })));
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
                if let Some(App::Buffer(BufferApp::Editing(e))) = self.workspace.focused_content_mut() {
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
                self.set_screen(App::Buffer(BufferApp::Viewing(DocState {
                    blocks,
                    file_label: label,
                    cursor_block: 0,
                    list_state: DocState::new_list_state(0),
                    list_item_count: std::cell::Cell::new(0),
                    blocks_seq: 0,
                    blocks_snapshot: RefCell::new(None),
                    last_cursor_block: std::cell::Cell::new(None),
                    source,
                })));
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

        // Local leader: bare `.` in normal mode opens the Edit local menu
        // (spec-menu-scopes.md Behavior 3 — insert mode keeps `.` as text).
        if mode == EditMode::Normal && press.modifiers.is_empty() && press.key == Key::Char('.') {
            self.open_local_menu_inner(cx);
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
                editor.insert_char('\n');
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
                editor.cursor_jump_top();
            }
            "goto-bottom" => {
                editor.pre_move(false);
                editor.jump_cursor_bottom();
            }
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
            "delete-selection" => {
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
            }
            "change-selection" => {
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
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
                editor.delete_char_at_cursor();
            }
            "delete-line" => {
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
}
