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
        match self.workspace.focused_content_mut()? {
            App::Buffer(BufferApp::Editing(e)) => Some(e),
            _ => None,
        }
    }

    /// Test-only: install a fresh Edit screen over `text` (Code view, Insert
    /// mode) so the headless harness can drive keystrokes through the real
    /// `build_edit_body_code` highlight path.
    #[cfg(test)]
    pub(crate) fn test_open_edit(&mut self, text: &str) {
        self.test_open_edit_at_path(text, PathBuf::from("/tmp/harness.md"));
    }

    /// Test-only variant backed by a real path, for actions such as reload that
    /// resolve their target from the focused Buffer label.
    #[cfg(test)]
    pub(crate) fn test_open_edit_at_path(&mut self, text: &str, path: PathBuf) {
        let core: workspace::SharedCore = std::rc::Rc::new(std::cell::RefCell::new(
            yalda::editor::EditorCore::new(text.to_string(), path.clone()),
        ));
        let mut e = EditState::new(
            SharedEditor::new(1, core),
            path.display().to_string().into(),
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
    pub(crate) fn test_edit_cache_stats(&mut self, cx: &GpuiApp) -> (usize, bool) {
        let body = self.test_edit_body().expect("edit body view not created yet");
        let b = body.read(cx);
        (
            b.highlight_cache.last_recomputed,
            b.highlight_cache.last_was_skip,
        )
    }

    /// Test-only: the focused Edit tile's cached body view (C10), once rendered.
    #[cfg(test)]
    pub(crate) fn test_edit_body(&mut self) -> Option<Entity<EditBodyView>> {
        self.edit_mut()?.body.clone()
    }

    /// Test-only: the focused Edit body's virtualized row-list state (scroll).
    #[cfg(test)]
    pub(crate) fn test_edit_list(&mut self, cx: &GpuiApp) -> gpui::ListState {
        let body = self.test_edit_body().expect("edit body view not created yet");
        body.read(cx).list.state().clone()
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
    /// preview) view. Bound to `Ctrl-Shift-E` in the YaldaView key context.
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
        let (shared, label, place, folds): (
            Option<(workspace::FileBufferId, workspace::SharedCore)>,
            SharedString,
            Option<DocPlace>,
            DocFolds,
        ) = match self.workspace.focused_content_mut() {
            Some(App::Buffer(BufferApp::Viewing(d))) => (
                d.source.as_ref().map(|s| (s.buffer_id, s.core.clone())),
                d.file_label.clone(),
                DocPlace::of(d),
                d.folds.clone(),
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
        edit_state.doc_folds = folds;
        // UXI-Buffer-8: keep the reading position — caret at the focused
        // block's first source line, the top visible block's first line at the
        // top of the edit viewport. Unmapped Docs keep the 0,0 landing.
        if let Some(place) = place {
            let last_line = edit_state.editor.line_count().saturating_sub(1);
            let caret_line = place.cursor_line.min(last_line);
            let top_line = place.top_line.min(caret_line);
            edit_state.editor.set_cursor(caret_line, 0);
            edit_state.pending_land = Some((
                gpui::ListOffset {
                    item_ix: top_line,
                    offset_in_item: px(0.0),
                },
                caret_line,
            ));
            edit_state.doc_return = Some(DocReturn {
                edit_seq: edit_state.editor.edit_seq(),
                caret: (caret_line, 0),
                cursor_block: place.cursor_block,
                top: place.top,
            });
        }
        self.set_screen(App::Buffer(BufferApp::Editing(edit_state)));
        cx.notify();
    }

    /// Edit → Doc round trip. The new Doc keeps the SAME pooled core (5c), so
    /// it shows the buffer's *current* (unsaved) text and shares undo with any
    /// other view of the file. No stash — the shared core IS the live state.
    /// Position carries over (UXI-Buffer-9/10): the Doc's cursor block is the
    /// block holding the caret line and its top block the one holding the top
    /// visible edit line — or, after a no-op round trip, exactly where the Doc
    /// stood when Edit was entered.
    pub(crate) fn back_to_doc(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.workspace.replace_focused_content(
            // Placeholder; overwritten in every match arm below.
            App::Buffer(BufferApp::Viewing(DocState::viewing(
                Vec::new(),
                SharedString::new_static(""),
                None,
            ))),
        ) else {
            return;
        };
        match prev {
            App::Buffer(BufferApp::Editing(edit)) => {
                let edit_path = PathBuf::from(edit.file_label.as_ref());
                let blocks =
                    render_with_wiki_mapped(&edit.editor.full_text(), &self.theme, Some(&edit_path));
                let file_label = edit.file_label.clone();
                // 5c: the new Doc keeps the SAME pooled core the Edit view held
                // (shared text + undo). No stash — the core IS the live state.
                let source = DocSource::new(edit.editor.buffer_id, edit.editor.core.clone());
                let edit_top = edit
                    .body
                    .as_ref()
                    .map(|b| b.read(cx).list.state().logical_scroll_top().item_ix);
                let landing = doc_landing_from_edit(&edit, edit_top, &blocks);
                let mut doc = DocState::viewing(blocks, file_label, Some(source));
                // The Doc's folds survive the round trip (UXI-Buffer-14), re-keyed
                // onto the edited text; a landing inside a fold opens it.
                doc.restore_folds(edit.doc_folds.clone());
                if let Some((cursor_block, top)) = landing {
                    doc.reveal_fold(cursor_block);
                    doc.cursor_block = cursor_block;
                    doc.list.land(top, cursor_block);
                }
                self.set_screen(App::Buffer(BufferApp::Viewing(doc)));
            }
            other => {
                self.set_screen(other);
                return;
            }
        }
        cx.notify();
    }

    /// Resolve a wiki/local Markdown link target (e.g. `notes`,
    /// `subdir/topic.md`) against the source doc's directory and open the
    /// resulting Doc in a new buffer tile beside the source. Lookup order:
    ///   1. `<doc_dir>/<target>.md` — markdown convention; matches what
    ///      Obsidian / Foam / most wiki-aware editors do.
    ///   2. `<doc_dir>/<target>` — literal path, in case the user included
    ///      the extension already (or wants a non-md file).
    ///
    /// If neither exists, log to stderr and no-op (the source tile stays put;
    /// nothing opens).
    pub(crate) fn open_wiki_link(
        &mut self,
        target: &str,
        doc_dir: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) {
        let normalized = normalize_local_link_target(target);
        let target = normalized.trim();
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
        // Build through the shared-buffer path, then add a distinct tile
        // instead of replacing the document containing the clicked link.
        let Some(content) = self.make_doc_content(&path) else {
            return;
        };
        if self
            .workspace
            .split_focused(workspace::SplitDir::V, content)
            .is_none()
        {
            return;
        }
        self.workspace.retile_active();
        self.doc_selection = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Dispatch one rendered Markdown link. Web/mail links use the OS default
    /// handler; local links resolve against `base_dir` and open in a new buffer
    /// tile. Shared by document and agent-transcript click handlers.
    pub(crate) fn open_link_target(
        &mut self,
        target: &str,
        base_dir: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) {
        match classify_link(target) {
            LinkTarget::External(url) => self.open_external_link(&url, cx),
            LinkTarget::Wiki(path) => self.open_wiki_link(&path, base_dir, cx),
        }
    }

    /// Open an external URL in the OS default handler (macOS `open`, Linux
    /// `xdg-open` → the default browser). Only called for links `classify_link`
    /// deemed `External` (http/https/mailto), so this never launches an
    /// arbitrary local handler. Best-effort: a spawn failure is logged, not
    /// surfaced. NOTE: the actual browser launch is a live-subprocess side
    /// effect — verified by a human, not headlessly (harness gap #2); the
    /// routing decision that fixes bug-0018 is guarded by `classify_link`.
    pub(crate) fn open_external_link(&mut self, url: &str, _cx: &mut Context<Self>) {
        if let Err(e) = open_in_default_handler(url) {
            eprintln!("open external link {url}: {e}");
        }
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
                core.borrow_mut().replace_text(text, path);
                self.note_core_matches_disk(&core);
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
                        core.borrow_mut().replace_text(text, path.clone());
                        self.note_core_matches_disk(&core);
                        let blocks = render_with_wiki_mapped(
                            &core.borrow().document().full_text(),
                            &self.theme,
                            Some(&path),
                        );
                        (blocks, Some(DocSource::new(id, core)))
                    }
                    None => {
                        let doc = Document::from_text(text, path.clone());
                        let blocks =
                            render_with_wiki_mapped(&doc.full_text(), &self.theme, Some(&path));
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
                    if self.workspace.workspaces.len() > 1 {
                        let next =
                            (self.workspace.active_workspace + 1) % self.workspace.workspaces.len();
                        self.switch_to_buffer(next);
                        cx.notify();
                    }
                    return;
                }
                Key::BackTab => {
                    if self.workspace.workspaces.len() > 1 {
                        let prev = if self.workspace.active_workspace == 0 {
                            self.workspace.workspaces.len() - 1
                        } else {
                            self.workspace.active_workspace - 1
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
    /// state. Exposed through the Buffer tile menu; bare `Ctrl-W` is reserved
    /// exclusively as the shell's multi-key workspace prefix.
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
        let core = match self.edit_mut() {
            Some(e) => Rc::clone(&e.editor.core),
            None => return,
        };
        // A disk conflict (UXI-Buffer-5) blocks the save until the user picks
        // keep-mine / reload-theirs — Ctrl-S must not silently clobber an
        // external change. Otherwise write through THE file-sync write path
        // (atomic + echo-suppressed, UXI-Buffer-6/7).
        let path = core.borrow().document().file_path.clone();
        let msg: SharedString = if self.buffer_has_disk_conflict(&path) {
            "not saved: changed on disk — space k keep mine / space R reload theirs".into()
        } else {
            match self.write_core_to_disk(&core) {
                Ok(()) => "saved".into(),
                Err(e) => format!("save failed: {}", e).into(),
            }
        };
        let Some(edit) = self.edit_mut() else { return };
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
                        editor.insert_str(&format!("\n{prefix}"));
                    }
                    Some(ListContinuation::Terminate) => {
                        // Enter on an empty list item ends the list: wipe the
                        // dangling marker (one bulk delete), then drop to a
                        // fresh blank line.
                        let col = editor.cursor().col;
                        editor.delete_back_in_line(col);
                        editor.insert_char('\n');
                    }
                    None => editor.insert_char('\n'),
                }
            }
            Key::Backspace => {
                editor.backspace();
            }
            Key::Tab => editor.insert_str("  "),
            // Caret motion in insert mode. `insert_mode=true` lets the caret
            // rest one past EOL (unlike Normal). Shared by the buffer EditView
            // and the agent compose, so both get identical arrow/Home/End/Delete
            // behaviour — the "arrows are dead in the message box" bug.
            Key::Left => editor.cursor_move_left(),
            Key::Right => editor.move_right_clamped(true),
            Key::Up => {
                editor.cursor_move_up();
                editor.clamp_cursor_col(true);
            }
            Key::Down => editor.move_down(true),
            Key::Home => editor.cursor_move_line_start(),
            Key::End => editor.move_cursor_line_end(true),
            Key::Delete => editor.delete_forward_in_insert(),
            Key::Char(c) => {
                if press.modifiers.contains(KMods::CONTROL)
                    || press.modifiers.contains(KMods::PLATFORM)
                {
                    // Ignore ctrl-/cmd-chords in insert mode; only bare typed
                    // chars produce text. Without the PLATFORM guard an unbound
                    // Cmd chord (cmd-s, cmd-z) arrives as a bare Char and gets
                    // typed into the buffer.
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

        let mut register = None;
        let outcome = Self::dispatch_normal_core(
            &mut edit.editor,
            &mut edit.mode,
            &mut edit.keybinds,
            press,
            &mut register,
        );
        Self::write_register(register, cx);
        match outcome {
            NormalOutcome::Skipped => {}
            NormalOutcome::Handled => cx.notify(),
            NormalOutcome::Yanked => {
                edit.last_save_msg = Some("yanked".into());
                cx.notify();
            }
            NormalOutcome::Quit => cx.quit(),
            NormalOutcome::OpenMenu => self.open_menu_inner(cx),
            NormalOutcome::Paste { before } => {
                let text = Self::clipboard_text(cx);
                if let Some(e) = self.edit_mut() {
                    if Self::apply_paste(&mut e.editor, text, before) {
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
    /// based on the returned `NormalOutcome`, and for handing `register` (the
    /// text a yank/delete put in vim's default register, if any) to
    /// [`Self::write_register`] — the core has no `cx`, so it can't reach the
    /// clipboard itself (C6: one clipboard path, GPUI's).
    pub(crate) fn dispatch_normal_core<E: EditOps>(
        editor: &mut E,
        mode: &mut EditMode,
        keybinds: &mut KeybindManager,
        press: KeyPress,
        register: &mut Option<String>,
    ) -> NormalOutcome {
        // Esc clears any active selection and exits extend mode.
        if press.key == Key::Esc {
            editor.set_extend_mode(false);
            editor.clear_selection();
            return NormalOutcome::Handled;
        }

        // Unbound Cmd chords (e.g. cmd-a, cmd-d) fall through to here as a bare
        // Char; without this they'd fire the letter's vim action (cmd-a →
        // insert-after, cmd-d → delete). Global Cmd bindings are consumed by the
        // GPUI keymap before on_key_down, so nothing legitimate reaches here.
        if press.modifiers.contains(KMods::PLATFORM) {
            return NormalOutcome::Skipped;
        }

        let Some(mut action_name) = keybinds.process_key(press) else {
            return NormalOutcome::Skipped;
        };
        // B17: a key that breaks a multi-key prefix can resolve two actions
        // (the prefix key's own binding, then the re-fed key). Run them in
        // order; an outcome the caller must act on (paste, quit, menu, yank)
        // ends the run and drops the rest.
        let mut result = NormalOutcome::Skipped;
        loop {
            let count = keybinds.take_count();
            match Self::run_normal_action(editor, mode, action_name, count, register) {
                NormalOutcome::Skipped => {}
                NormalOutcome::Handled => result = NormalOutcome::Handled,
                other => {
                    while keybinds.next_queued_action().is_some() {
                        keybinds.take_count();
                    }
                    return other;
                }
            }
            match keybinds.next_queued_action() {
                Some(next) => action_name = next,
                None => return result,
            }
        }
    }

    /// Run one resolved Normal-mode action (`action_name`, with the numeric
    /// count prefix typed ahead of it, e.g. `42` in `42G`).
    fn run_normal_action<E: EditOps>(
        editor: &mut E,
        mode: &mut EditMode,
        action_name: String,
        count: Option<usize>,
        register: &mut Option<String>,
    ) -> NormalOutcome {
        // Repeat count for motions: `10j` moves ten lines, `3w` three words.
        // Capped so a pathological `999999999j` can't spin. pre_move runs once
        // (it collapses/extends the selection); only the inner step repeats.
        let n = count.unwrap_or(1).min(100_000);

        match action_name.as_str() {
            // ---- Pure motions: collapse selection (or extend in extend mode) ----
            "move-down" => {
                editor.pre_move(false);
                for _ in 0..n {
                    editor.move_down(false);
                }
                editor.normalize_linewise_selection();
            }
            "move-up" => {
                editor.pre_move(false);
                for _ in 0..n {
                    editor.cursor_move_up();
                }
                editor.clamp_cursor_col(false);
                editor.normalize_linewise_selection();
            }
            "move-left" => {
                editor.pre_move(false);
                for _ in 0..n {
                    editor.cursor_move_left();
                }
                editor.normalize_linewise_selection();
            }
            "move-right" => {
                editor.pre_move(false);
                for _ in 0..n {
                    editor.move_right_clamped(false);
                }
                editor.normalize_linewise_selection();
            }
            "move-line-start" => {
                editor.pre_move(false);
                editor.cursor_move_line_start();
                editor.normalize_linewise_selection();
            }
            "move-line-first-non-blank" => {
                editor.pre_move(false);
                editor.move_cursor_first_non_blank();
                editor.normalize_linewise_selection();
            }
            "move-line-end" => {
                editor.pre_move(false);
                editor.move_cursor_line_end(false);
                editor.normalize_linewise_selection();
            }
            // ---- Word motions: create a fresh selection from cursor → motion target ----
            "move-word-forward" => {
                editor.pre_move(true);
                for _ in 0..n {
                    editor.move_cursor_word_forward();
                }
                editor.normalize_linewise_selection();
            }
            "move-word-backward" => {
                editor.pre_move(true);
                for _ in 0..n {
                    editor.move_cursor_word_backward();
                }
                editor.normalize_linewise_selection();
            }
            "move-word-end" => {
                editor.pre_move(true);
                for _ in 0..n {
                    editor.move_cursor_word_end();
                }
                editor.normalize_linewise_selection();
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
                editor.normalize_linewise_selection();
            }
            "goto-bottom" => {
                editor.pre_move(false);
                // `<count>G` jumps to line `count` (1-indexed); bare `G`
                // goes to the last line.
                match count {
                    Some(n) => editor.jump_to_line(n.saturating_sub(1)),
                    None => editor.jump_cursor_bottom(),
                }
                editor.normalize_linewise_selection();
            }
            // ---- Half / full page paging ----
            // The Edit + Agent render paths both scroll-to-reveal the cursor
            // line every frame, so paging is just a cursor move by N lines;
            // no viewport-height plumbing is needed at this `self`-less site.
            "half-page-down" => {
                editor.pre_move(false);
                Self::page_cursor(editor, HALF_PAGE_LINES as isize);
                editor.normalize_linewise_selection();
            }
            "half-page-up" => {
                editor.pre_move(false);
                Self::page_cursor(editor, -(HALF_PAGE_LINES as isize));
                editor.normalize_linewise_selection();
            }
            "full-page-down" => {
                editor.pre_move(false);
                Self::page_cursor(editor, FULL_PAGE_LINES as isize);
                editor.normalize_linewise_selection();
            }
            "full-page-up" => {
                editor.pre_move(false);
                Self::page_cursor(editor, -(FULL_PAGE_LINES as isize));
                editor.normalize_linewise_selection();
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
            "delete-selection" => Self::yank_then_delete_selection(editor, register),
            "change-selection" => {
                Self::yank_then_delete_selection(editor, register);
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
                *register = Some(text);
                return NormalOutcome::Yanked;
            }
            "collapse-selection" => editor.collapse_selection(),
            "flip-selection" => editor.flip_selection(),
            "select-all" => editor.select_all(),
            "extend-line" => editor.extend_by_line(),
            // `V`: true linewise visual mode. Unlike plain extend-mode, every
            // following motion is normalized back to whole logical lines.
            "select-line" => editor.select_linewise(),
            "toggle-extend-mode" => {
                editor.toggle_extend_mode();
                if editor.extend_mode() && editor.selection_anchor().is_none() {
                    editor.anchor_at_cursor();
                }
            }
            // ---- Direct-edit actions (still callable via custom config) ----
            "delete-char" => Self::delete_chars(editor, n, register),
            "delete-line" => {
                let line = editor.line_text_at_cursor();
                if !line.is_empty() {
                    *register = Some(line);
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
    fn yank_then_delete_selection<E: EditOps>(editor: &mut E, register: &mut Option<String>) {
        if editor.selection_anchor().is_some() {
            if let Some(t) = editor.yank_selection().filter(|s| !s.is_empty()) {
                *register = Some(t);
            }
            editor.delete_selection();
        } else {
            if let Some(t) = char_under_cursor(editor) {
                *register = Some(t);
            }
            editor.delete_char_at_cursor();
        }
    }

    /// Counted `delete-char` (`5x`): ONE range delete of up to `n` chars on
    /// the cursor's line (vim `x` never crosses the newline) — one undo group,
    /// one reparse — yanking the whole deleted run (C6). A single char keeps
    /// the plain `delete_char_at_cursor` path.
    fn delete_chars<E: EditOps>(editor: &mut E, n: usize, register: &mut Option<String>) {
        let cur = editor.cursor();
        let raw = editor.line_text_at_cursor();
        let line_chars = raw.strip_suffix('\n').unwrap_or(&raw).chars().count();
        let m = n.min(line_chars.saturating_sub(cur.col));
        if m <= 1 {
            if let Some(t) = char_under_cursor(editor) {
                *register = Some(t);
            }
            editor.delete_char_at_cursor();
            return;
        }
        // Selections are [anchor, cursor): anchor here, cursor `m` chars on.
        editor.cursor_set(cur.line, cur.col);
        editor.anchor_at_cursor();
        editor.cursor_set(cur.line, cur.col + m);
        if let Some(t) = editor.yank_selection().filter(|s| !s.is_empty()) {
            *register = Some(t);
        }
        editor.delete_selection();
        editor.clamp_cursor_col(false);
    }

    /// Put a yank/delete's `register` text on the system clipboard through
    /// GPUI (the same path Cmd-C uses; C6 — no blocking CLI subprocess).
    pub(crate) fn write_register(register: Option<String>, cx: &mut Context<Self>) {
        if let Some(t) = register {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
    }

    /// The system clipboard's text through GPUI, for a `p`/`P` put — line
    /// endings normalized to the `\n`-only editors (D15).
    pub(crate) fn clipboard_text(cx: &mut Context<Self>) -> Option<String> {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .map(|t| normalize_pasted_newlines(&t))
    }

    /// Charwise put of `text` at (P, `before=true`) or just after (p,
    /// `before=false`) the cursor. Charwise because yalda's yank stores raw
    /// text in the system clipboard with no linewise-vs-charwise register
    /// metadata. Leaves the cursor on the last inserted character (vim
    /// convention). Returns false if there was nothing to insert.
    pub(crate) fn put_text<E: EditOps>(editor: &mut E, text: &str, before: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        // For `p`, start inserting after the cursor's char (unless the line
        // is empty / cursor already past end). `paste_str` lands the text as
        // one bulk splice in one undo group (B10).
        if !before {
            let line = editor.cursor().line;
            if editor.line_len_chars(line) > 0 {
                editor.move_right_clamped(true);
            }
        }
        editor.paste_str(text);
        // Step back onto the last inserted char (cursor sits one past it).
        if editor.cursor().col > 0 {
            editor.cursor_move_left();
        }
        true
    }

    /// Resolve a [`NormalOutcome::Paste`] by putting the clipboard `text`
    /// ([`Self::clipboard_text`]) into `editor`. Shared by the Edit and Agent
    /// dispatch sites.
    pub(crate) fn apply_paste<E: EditOps>(editor: &mut E, text: Option<String>, before: bool) -> bool {
        match text {
            Some(text) => Self::put_text(editor, &text, before),
            None => false,
        }
    }
}

/// A source-mapped Doc's reading position, projected onto source lines
/// (UXI-Buffer-8). `None` for an unmapped Doc (no `spans`).
struct DocPlace {
    cursor_block: usize,
    /// First source line of the focused block — where the Edit caret lands.
    cursor_line: usize,
    /// First source line of the top visible block — the Edit view's top row.
    top_line: usize,
    /// The Doc list's logical scroll top, stashed for an exact round trip.
    top: gpui::ListOffset,
}

impl DocPlace {
    fn of(d: &DocState) -> Option<Self> {
        let n = d.blocks.len();
        if n == 0 || d.spans.len() != n {
            return None;
        }
        let cursor_block = d.cursor_block.min(n - 1);
        let top = d.list.state().logical_scroll_top();
        let top_block = top.item_ix.min(n - 1);
        Some(Self {
            cursor_block,
            cursor_line: d.spans[cursor_block].lines.start,
            top_line: d.spans[top_block].lines.start,
            top,
        })
    }
}

/// Where the Doc rebuilt by `back_to_doc` lands (UXI-Buffer-9/10):
/// `(cursor_block, list top)`. A no-op round trip (no edit, caret unmoved since
/// Doc→Edit) restores the stashed Doc position exactly; otherwise the cursor is
/// the block holding the caret line and the top is the block holding the top
/// visible edit line. `None` for unmapped output (keeps the top-of-doc landing).
fn doc_landing_from_edit(
    edit: &EditState,
    edit_top: Option<usize>,
    rendered: &Rendered,
) -> Option<(usize, gpui::ListOffset)> {
    let n = rendered.blocks.len();
    if n == 0 || rendered.spans.len() != n {
        return None;
    }
    let caret = edit.editor.cursor();
    if let Some(r) = edit.doc_return
        && r.edit_seq == edit.editor.edit_seq()
        && r.caret == (caret.line, caret.col)
        && r.cursor_block < n
    {
        return Some((r.cursor_block, r.top));
    }
    let cursor_block = rendered.block_at_line(caret.line)?;
    let top_line = edit_top.unwrap_or(caret.line);
    let top_block = rendered.block_at_line(top_line)?.min(cursor_block);
    Some((
        cursor_block,
        gpui::ListOffset {
            item_ix: top_block,
            offset_in_item: px(0.0),
        },
    ))
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
    Some(ListContinuation::Continue(format!(
        "{indent}{continuation}"
    )))
}

/// Given the post-indent remainder of a line, recognize a leading list marker.
/// Returns `(chars consumed by the marker, the prefix to start the next item)`.
/// Checkbox items reset to unchecked; ordered items increment.
fn parse_list_marker(rest: &str) -> Option<(usize, String)> {
    use yalda::md_line;
    // Markers are ASCII, so the shared parser's byte lengths are char counts.
    // Bullet markers: `-`, `*`, `+` followed by a space.
    if let Some(bullet) = md_line::bullet_marker(rest) {
        let bullet = bullet as char;
        // Checkbox: `- [ ] ` / `- [x] ` / `- [X] ` — resets to unchecked.
        if md_line::checkbox_after_bullet(rest).is_some() {
            return Some((6, format!("{bullet} [ ] ")));
        }
        return Some((2, format!("{bullet} ")));
    }
    // Blockquote: `> ` (a bare `>` without the space doesn't continue).
    if rest.starts_with("> ") {
        return Some((2, "> ".to_string()));
    }
    // Ordered list: digits then `.` or `)` then a space.
    let (digits, sep) = md_line::ordered_marker(rest)?;
    let sep = sep as char;
    let n: u64 = rest[..digits].parse().ok()?;
    Some((digits + 2, format!("{}{sep} ", n.saturating_add(1))))
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

    /// C9 behavior pin: the pre-refactor char-walk, verbatim, as an oracle —
    /// the shared-parser version must agree on every input of the corpus.
    #[test]
    fn matches_legacy_char_walk() {
        fn old(rest: &str) -> Option<(usize, String)> {
            let chars: Vec<char> = rest.chars().collect();
            if chars.len() >= 2 && matches!(chars[0], '-' | '*' | '+') && chars[1] == ' ' {
                let bullet = chars[0];
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
            if chars.len() >= 2 && chars[0] == '>' && chars[1] == ' ' {
                return Some((2, "> ".to_string()));
            }
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
        // Indent is stripped by the caller, so no TAB/`#` in this alphabet.
        let alpha = ['-', '*', '>', ' ', '1', '.', ')', '[', ']', 'x', 'X'];
        crate::c9_for_each_corpus(6, &alpha, &mut |s| {
            assert_eq!(parse_list_marker(s), old(s), "parse_list_marker({s:?})");
        });
        for s in ["* [x] é", "99999999999999999999999. x", "é- x", "- [ ] ü"] {
            assert_eq!(parse_list_marker(s), old(s), "parse_list_marker({s:?})");
        }
    }

    #[test]
    fn marker_char_count_matches_marker_length() {
        assert_eq!(parse_list_marker("- [ ] x").unwrap().0, 6);
        assert_eq!(parse_list_marker("- x").unwrap().0, 2);
        assert_eq!(parse_list_marker("12. x").unwrap().0, 4);
    }
}
