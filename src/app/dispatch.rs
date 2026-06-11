use yalda::blocks::RenderedBlock;
use yalda::buffer::NavMode;
use yalda::claude_channel::ChannelClient;
use yalda::keybind::Action;
use yalda::view::ViewMode;

use super::{App, AppMode, AppScreen};

impl App {
    pub(crate) fn execute_find_char(&mut self, action: Action, ch: char, viewport_height: usize) {
        let editor = &mut self.buffers[self.active_buffer].editor;
        editor.pre_move(false);
        match action {
            Action::FindCharForward => {
                editor.find_char_forward(ch);
            }
            Action::FindCharBackward => {
                editor.find_char_backward(ch);
            }
            Action::TillCharForward => {
                editor.till_char_forward(ch);
            }
            Action::TillCharBackward => {
                editor.till_char_backward(ch);
            }
            _ => {}
        }
        self.ensure_cursor_visible(viewport_height);
    }

    pub(crate) fn dispatch_command(
        &mut self,
        cmd_input: &str,
        viewport_height: usize,
        content_width: usize,
    ) {
        if let Some((action, args)) = self.registry.resolve(cmd_input) {
            if action == Action::Reload
                && let Some(path_arg) = args.as_deref().map(str::trim).filter(|s| !s.is_empty())
            {
                self.edit_path(path_arg);
                return;
            }
            if action == Action::SetMaxLineWidth {
                let arg = args.as_deref().map(str::trim).unwrap_or("");
                match arg.parse::<usize>() {
                    Ok(n) => self.set_max_line_width(n),
                    Err(_) => {
                        self.command_error = format!("set-width: expected number, got '{arg}'");
                    }
                }
                return;
            }
            if action == Action::ClaudeAttach {
                let arg = args.as_deref().map(str::trim).unwrap_or("");
                self.attach_claude_channel(arg);
                return;
            }
            if action == Action::ClaudeAcpAttach {
                let arg = args.as_deref().map(str::trim).unwrap_or("");
                self.attach_acp_channel(arg);
                return;
            }
            self.execute_action(action, viewport_height, content_width);
        } else {
            self.command_error = format!("Unknown command: {}", cmd_input);
        }
    }

    pub(crate) fn goto_line(&mut self, line: usize, viewport_height: usize) {
        let doc = self.buffers[self.active_buffer].editor.document();
        let max_line = doc.line_count().saturating_sub(1);
        let target = line.min(max_line);
        self.buffers[self.active_buffer].editor.cursor_mut().line = target;
        self.buffers[self.active_buffer].editor.cursor_mut().col = 0;
        self.ensure_cursor_visible(viewport_height);
    }

    pub(crate) fn execute_action(
        &mut self,
        action: Action,
        viewport_height: usize,
        content_width: usize,
    ) {
        match action {
            Action::Quit => {
                if self.buffers[self.active_buffer]
                    .editor
                    .document()
                    .is_modified()
                {
                    self.command_error =
                        "No write since last change (add ! to override)".to_string();
                } else {
                    self.compose_textbox = None;
                    self.should_quit = true;
                }
            }
            Action::MoveDown => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_next();
                    } else {
                        let total = self.buffers[self.active_buffer].viewport.total_lines;
                        if self.buffers[self.active_buffer].rendered_cursor_row + 1 < total {
                            self.buffers[self.active_buffer].rendered_cursor_row += 1;
                        }
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.pre_move(false);
                    self.buffers[self.active_buffer].editor.move_down(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
            Action::MoveUp => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if self.buffers[self.active_buffer].nav_mode != NavMode::Character {
                        self.nav_move_prev();
                    } else {
                        self.buffers[self.active_buffer].rendered_cursor_row = self.buffers
                            [self.active_buffer]
                            .rendered_cursor_row
                            .saturating_sub(1);
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.pre_move(false);
                    self.buffers[self.active_buffer]
                        .editor
                        .cursor_mut()
                        .move_up();
                    self.buffers[self.active_buffer]
                        .editor
                        .clamp_cursor_col(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
            Action::MoveLeft
            | Action::MoveRight
            | Action::MoveWordForward
            | Action::MoveWordBackward
            | Action::MoveWordEnd
            | Action::MoveLineStart
            | Action::MoveLineEnd => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let nav = self.buffers[self.active_buffer].nav_mode;
                    if nav == NavMode::Link || nav == NavMode::ListItem {
                        match action {
                            Action::MoveLeft => self.nav_move_prev(),
                            Action::MoveRight => self.nav_move_next(),
                            _ => {}
                        }
                        self.ensure_rendered_cursor_visible(viewport_height);
                    } else if nav == NavMode::Character {
                        match action {
                            Action::MoveLeft => {
                                self.buffers[self.active_buffer].rendered_cursor_col = self.buffers
                                    [self.active_buffer]
                                    .rendered_cursor_col
                                    .saturating_sub(1);
                            }
                            Action::MoveRight => {
                                self.buffers[self.active_buffer].rendered_cursor_col += 1;
                            }
                            Action::MoveLineStart => {
                                self.buffers[self.active_buffer].rendered_cursor_col = 0;
                            }
                            _ => {}
                        }
                    }
                    // Heading and CodeBlock modes: h/l are no-ops
                } else {
                    let editor = &mut self.buffers[self.active_buffer].editor;
                    match action {
                        Action::MoveLeft => {
                            editor.pre_move(false);
                            editor.cursor_mut().move_left();
                        }
                        Action::MoveRight => {
                            editor.pre_move(false);
                            editor.move_right_clamped(false);
                        }
                        Action::MoveWordForward => {
                            editor.pre_move(true);
                            editor.move_cursor_word_forward();
                            self.ensure_cursor_visible(viewport_height);
                        }
                        Action::MoveWordBackward => {
                            editor.pre_move(true);
                            editor.move_cursor_word_backward();
                            self.ensure_cursor_visible(viewport_height);
                        }
                        Action::MoveWordEnd => {
                            editor.pre_move(true);
                            editor.move_cursor_word_end();
                        }
                        Action::MoveLineStart => {
                            editor.pre_move(false);
                            editor.cursor_mut().move_line_start();
                        }
                        Action::MoveLineEnd => {
                            editor.pre_move(false);
                            editor.move_cursor_line_end(false);
                        }
                        _ => {}
                    }
                }
            }
            Action::FindCharForward
            | Action::FindCharBackward
            | Action::TillCharForward
            | Action::TillCharBackward => {
                // Only meaningful in raw (Edit) mode against real document text.
                if self.buffers[self.active_buffer].view_mode != ViewMode::Rendered {
                    self.pending_find_char = Some(action);
                }
            }
            Action::InsertMode => {
                self.ensure_raw_for_editing();
                let editor = &mut self.buffers[self.active_buffer].editor;
                // Helix-style: i inserts at selection start
                if let Some(((sl, sc), _)) = editor.selection_range() {
                    editor.cursor_mut().line = sl;
                    editor.cursor_mut().col = sc;
                    editor.clear_selection();
                }
                editor.set_extend_mode(false);
                // If we'd land on a frozen line, auto-open an editable line
                // ABOVE it. The user never wants to type into Claude's prose;
                // their text always goes on its own line between frozen lines.
                let line = editor.cursor().line;
                if editor.is_frozen_line(line) {
                    editor.open_line_above();
                } else {
                    editor.begin_insert();
                }
                self.mode = AppMode::Insert;
            }
            Action::InsertAfter => {
                self.ensure_raw_for_editing();
                let editor = &mut self.buffers[self.active_buffer].editor;
                // Helix-style: a inserts after selection end
                if let Some((_, (el, ec))) = editor.selection_range() {
                    editor.cursor_mut().line = el;
                    editor.cursor_mut().col = ec;
                    let line_len = editor.document().line_len_chars(el);
                    if editor.cursor().col < line_len {
                        editor.cursor_mut().col += 1;
                    }
                    editor.clear_selection();
                } else {
                    editor.move_right_clamped(true);
                }
                editor.set_extend_mode(false);
                // Frozen-line guard: open a new editable line BELOW so the
                // user's typing lands between frozen lines, not on one.
                let line = editor.cursor().line;
                if editor.is_frozen_line(line) {
                    editor.open_line_below();
                } else {
                    editor.begin_insert();
                }
                self.mode = AppMode::Insert;
            }
            Action::OpenLineBelow => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.open_line_below();
                self.mode = AppMode::Insert;
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::OpenLineAbove => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.open_line_above();
                self.mode = AppMode::Insert;
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::DeleteChar => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer]
                    .editor
                    .delete_char_at_cursor();
                self.buffers[self.active_buffer].view_cache_dirty = true;
            }
            Action::DeleteLine => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer]
                    .editor
                    .delete_current_line();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Undo => {
                self.buffers[self.active_buffer].editor.undo();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::Redo => {
                self.buffers[self.active_buffer].editor.redo();
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::EnterCommand => {
                self.command_buffer.clear();
                self.command_error.clear();
                self.mode = AppMode::Command;
            }
            Action::ScrollDown => {
                self.buffers[self.active_buffer]
                    .viewport
                    .scroll_down(1, viewport_height);
            }
            Action::ScrollUp => {
                self.buffers[self.active_buffer].viewport.scroll_up(1);
            }
            Action::HalfPageDown => {
                // vim semantics: scroll AND move the cursor by the same
                // amount, so it stays in the same screen position. (If we
                // only scrolled, the end-of-handle_key ensure_cursor_visible
                // would snap the viewport back to the cursor.)
                self.page_move_cursor(viewport_height / 2, true);
            }
            Action::HalfPageUp => {
                self.page_move_cursor(viewport_height / 2, false);
            }
            Action::FullPageDown => {
                self.page_move_cursor(viewport_height, true);
            }
            Action::FullPageUp => {
                self.page_move_cursor(viewport_height, false);
            }
            Action::JumpTop => {
                self.buffers[self.active_buffer]
                    .editor
                    .cursor_mut()
                    .jump_top();
                self.buffers[self.active_buffer].viewport.jump_top();
            }
            Action::JumpBottom => {
                self.buffers[self.active_buffer].editor.jump_cursor_bottom();
                self.buffers[self.active_buffer]
                    .viewport
                    .jump_bottom(viewport_height);
            }
            Action::NextHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered
                    && let Some(y) = self.find_next_heading(None)
                {
                    self.buffers[self.active_buffer].viewport.scroll_offset = y;
                }
            }
            Action::PrevHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered
                    && let Some(y) = self.find_prev_heading(None)
                {
                    self.buffers[self.active_buffer].viewport.scroll_offset = y;
                }
            }
            Action::NextHeadingSameLevel => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let current_level = self.heading_level_at_offset();
                    if let Some(y) = self.find_next_heading(current_level) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::PrevHeadingSameLevel => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    let current_level = self.heading_level_at_offset();
                    if let Some(y) = self.find_prev_heading(current_level) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::SearchForward | Action::SearchBackward => {
                self.search_input_mode = true;
                self.search_input_buffer.clear();
            }
            Action::SearchNext => {
                if !self.search_matches.is_empty() {
                    self.search_match_index =
                        (self.search_match_index + 1) % self.search_matches.len();
                    self.jump_to_match(viewport_height);
                }
            }
            Action::SearchPrev => {
                if !self.search_matches.is_empty() {
                    self.search_match_index = if self.search_match_index == 0 {
                        self.search_matches.len() - 1
                    } else {
                        self.search_match_index - 1
                    };
                    self.jump_to_match(viewport_height);
                }
            }
            Action::OpenLink => {
                if let Some(url) = self.link_under_rendered_cursor() {
                    self.open_link(&url);
                }
            }
            Action::YankLine => {
                let buf = &self.buffers[self.active_buffer];
                let line_text = buf.editor.document().line_text(buf.editor.cursor().line);
                let text = line_text.trim_end_matches('\n');
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
                    && let Some(mut stdin) = child.stdin.take()
                {
                    let _ = stdin.write_all(text.as_bytes());
                }
            }
            Action::OpenMenu => {
                self.menu_state.open();
                self.mode = AppMode::Menu;
            }
            Action::OpenFileBrowser => {
                self.open_file_browser();
            }
            Action::Save => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                }
            }
            Action::SaveAs => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                }
            }
            Action::ForceQuit => {
                // Discard compose textbox without inserting.
                self.compose_textbox = None;
                self.should_quit = true;
            }
            Action::SaveQuit => {
                if let Err(e) = self.buffers[self.active_buffer].editor.save() {
                    self.command_error = format!("Error writing file: {}", e);
                } else {
                    self.should_quit = true;
                }
            }
            Action::ToggleView => {
                let buf = &mut self.buffers[self.active_buffer];
                buf.view_mode = match buf.view_mode {
                    ViewMode::Rendered => ViewMode::Raw,
                    ViewMode::Raw => ViewMode::Rendered,
                };
                buf.view_cache_dirty = true;
            }
            Action::NextBuffer => {
                if self.buffers.len() > 1 {
                    // Close compose before switching — it belongs to the current buffer.
                    if self.compose_textbox.is_some() {
                        self.compose_toggle();
                    }
                    self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
                }
            }
            Action::PrevBuffer => {
                if self.buffers.len() > 1 {
                    if self.compose_textbox.is_some() {
                        self.compose_toggle();
                    }
                    self.active_buffer = if self.active_buffer == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.active_buffer - 1
                    };
                }
            }
            Action::BufferList => {
                self.buffer_list_selected = self.active_buffer;
                self.buffer_list_filter_mode = false;
                self.buffer_list_filter_text.clear();
                self.screen = AppScreen::BufferList;
            }
            Action::CloseBuffer => {
                self.close_current_buffer();
            }
            Action::Reload => {
                self.reload_current_buffer();
            }
            Action::Outline => {
                self.outline_filter_mode = false;
                self.outline_filter_text.clear();
                self.outline_stack.clear();
                self.outline_saved_scroll = self.buffers[self.active_buffer].viewport.scroll_offset;
                self.mode = AppMode::Outline;
                // Select the heading closest to current scroll position
                let scroll = self.buffers[self.active_buffer].viewport.scroll_offset;
                let entries = self.filtered_outline_entries();
                self.outline_selected = entries
                    .iter()
                    .enumerate()
                    .rfind(|(_, e)| e.y_offset <= scroll)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.scroll_to_outline_entry();
            }
            Action::OpenFileBrowserFull => {
                self.open_file_browser_full(false);
            }
            Action::None
            | Action::FileBrowserDown
            | Action::FileBrowserUp
            | Action::FileBrowserEnter
            | Action::FileBrowserParentDir
            | Action::FileBrowserFilter
            | Action::FileBrowserClose => {}
            Action::NavCycle => {
                let current = self.buffers[self.active_buffer].nav_mode;
                let next = current.next();
                self.enter_nav_mode(next, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCharacter => {
                self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            }
            Action::NavLinks => {
                self.enter_nav_mode(NavMode::Link, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavHeadings => {
                self.enter_nav_mode(NavMode::Heading, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavListItems => {
                self.enter_nav_mode(NavMode::ListItem, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavCodeBlocks => {
                self.enter_nav_mode(NavMode::CodeBlock, content_width);
                self.ensure_rendered_cursor_visible(viewport_height);
            }
            Action::NavActivate => {
                self.nav_activate();
            }
            Action::SetHeading1 => self.set_heading_level(1),
            Action::SetHeading2 => self.set_heading_level(2),
            Action::SetHeading3 => self.set_heading_level(3),
            Action::SetHeading4 => self.set_heading_level(4),
            Action::SetHeading5 => self.set_heading_level(5),
            Action::SetHeading6 => self.set_heading_level(6),
            Action::ClearHeading => self.set_heading_level(0),
            Action::SetMaxLineWidth => {
                // No argument supplied — tell the user how to use it.
                self.command_error = "Usage: :set-width <n>  (0 = full terminal)".to_string();
            }
            Action::DeleteSelection => {
                self.ensure_raw_for_editing();
                let editor = &mut self.buffers[self.active_buffer].editor;
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.ensure_cursor_visible(viewport_height);
            }
            Action::ChangeSelection => {
                self.ensure_raw_for_editing();
                let editor = &mut self.buffers[self.active_buffer].editor;
                if editor.selection_anchor().is_some() {
                    editor.delete_selection();
                } else {
                    editor.delete_char_at_cursor();
                }
                self.buffers[self.active_buffer].view_cache_dirty = true;
                self.buffers[self.active_buffer].editor.begin_insert();
                self.mode = AppMode::Insert;
            }
            Action::YankSelection => {
                let buf = &self.buffers[self.active_buffer];
                let text = match buf.editor.yank_selection() {
                    Some(t) if !t.is_empty() => t,
                    _ => {
                        // Fallback: yank current line
                        let line_text = buf.editor.document().line_text(buf.editor.cursor().line);
                        line_text.trim_end_matches('\n').to_string()
                    }
                };
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
                    && let Some(mut stdin) = child.stdin.take()
                {
                    let _ = stdin.write_all(text.as_bytes());
                }
            }
            Action::CollapseSelection => {
                self.buffers[self.active_buffer].editor.collapse_selection();
            }
            Action::FlipSelection => {
                self.buffers[self.active_buffer].editor.flip_selection();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::SelectAll => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.select_all();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::ExtendByLine => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.extend_by_line();
                self.ensure_cursor_visible(viewport_height);
            }
            Action::ToggleExtendMode => {
                let editor = &mut self.buffers[self.active_buffer].editor;
                editor.toggle_extend_mode();
                if editor.extend_mode() && editor.selection_anchor().is_none() {
                    editor.anchor_at_cursor();
                }
            }
            Action::ClaudeAttach => {
                // Bare invocation (no arg) → connect to default socket path.
                self.attach_claude_channel("");
            }
            Action::ClaudeDetach => {
                let was = self.claude_channel.take().is_some();
                self.command_error = if was {
                    "Detached from Claude channel".to_string()
                } else {
                    "Not attached".to_string()
                };
            }
            Action::ClaudeSend => {
                self.claude_send_buffer();
            }
            Action::ClaudeSendSelection => {
                self.claude_send_selection();
            }
            Action::ClaudeStatus => {
                self.command_error = match &self.claude_channel {
                    Some(c) => {
                        let live = if c.is_connected() {
                            "alive"
                        } else {
                            "STALE — re-run :claude-attach"
                        };
                        format!("Claude channel: {} ({})", c.socket_path().display(), live)
                    }
                    None => format!(
                        "Not attached. Default socket: {}",
                        ChannelClient::default_socket_path().display()
                    ),
                };
            }
            Action::ClaudeTest => {
                // Inject a synthetic reply to verify the local *claude*-buffer
                // path independent of Claude / yalda-channel.
                self.append_to_claude_buffer(
                    "Hello from :claude-test.\n\nThis is paragraph two.\n\nThis is paragraph three.",
                );
                self.command_error = "Injected synthetic Claude reply into *claude* buffer.".into();
            }
            Action::ClaudeAcpAttach => {
                // Bare keybinding invocation (no arg) → use default command.
                self.attach_acp_channel("");
            }
            Action::ClaudeAcpDetach => {
                let was = self.acp_channel.take().is_some();
                self.command_error = if was {
                    "Detached from ACP agent (subprocess terminated)".to_string()
                } else {
                    "No ACP agent attached".to_string()
                };
            }
            Action::ClaudeAcpSend => {
                self.acp_send_buffer();
            }
            Action::ClaudeAcpSendSelection => {
                self.acp_send_selection();
            }
            Action::ClaudeAcpStatus => {
                self.command_error = match &self.acp_channel {
                    Some(c) => {
                        let live = if c.is_connected() {
                            "alive"
                        } else {
                            "dead — re-run :claude-acp-attach"
                        };
                        format!("{} ({live})", c.description())
                    }
                    None => format!(
                        "No ACP agent. Default cmd: {}",
                        yalda::acp_channel::DEFAULT_AGENT_COMMAND
                    ),
                };
            }
            Action::ComposeToggle => {
                self.compose_toggle();
            }
            Action::ComposeSend => {
                self.compose_send();
            }
        }
    }

    /// Update the soft right-margin wrap width. `0` disables the cap (use full terminal width).
    pub(crate) fn set_max_line_width(&mut self, n: usize) {
        self.max_line_width = n;
        for buf in &mut self.buffers {
            buf.viewport.max_line_width = n;
            buf.view_cache_dirty = true;
        }
    }

    /// Rewrite the current line with `level` hashes (0 = remove heading markers).
    pub(crate) fn set_heading_level(&mut self, level: u8) {
        self.ensure_raw_for_editing();
        let buf = &mut self.buffers[self.active_buffer];
        let line_idx = buf.editor.cursor().line;
        let existing = buf.editor.document().line_text(line_idx);
        let existing = existing.strip_suffix('\n').unwrap_or(&existing);

        // Strip any existing leading `#`s + single space.
        let trimmed = {
            let without_hashes = existing.trim_start_matches('#');
            if without_hashes.len() != existing.len() {
                without_hashes.strip_prefix(' ').unwrap_or(without_hashes)
            } else {
                existing
            }
        };

        let new_line = if level == 0 {
            trimmed.to_string()
        } else {
            let hashes: String = "#".repeat(level as usize);
            if trimmed.is_empty() {
                format!("{hashes} ")
            } else {
                format!("{hashes} {trimmed}")
            }
        };

        let cursor_col_before = buf.editor.cursor().col;
        let frozen_snapshot: Vec<(usize, usize)> = buf.editor.frozen_lines().to_vec();
        let lockable_snapshot = buf.editor.lockable_through_line();
        buf.editor.document_mut().begin_undo_group(
            line_idx,
            cursor_col_before,
            &frozen_snapshot,
            lockable_snapshot,
        );
        buf.editor
            .document_mut()
            .replace_line_text(line_idx, &new_line);

        // Keep cursor on the same line, clamp to new length.
        let new_len = buf.editor.document().line_len_chars(line_idx);
        let new_col = cursor_col_before.min(new_len);
        buf.editor.cursor_mut().line = line_idx;
        buf.editor.cursor_mut().col = new_col;
        buf.editor.document_mut().end_undo_group(line_idx, new_col);
        buf.view_cache_dirty = true;
    }

    pub(crate) fn execute_command(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
        let cmd_name = parts[0];

        // Special case: `:w filename` → save-as with filename
        if cmd_name == "w" && parts.len() > 1 {
            let path = std::path::Path::new(parts[1]);
            if let Err(e) = self.buffers[self.active_buffer].editor.save_to(path) {
                self.command_error = format!("Error writing file: {}", e);
            }
            return;
        }

        // Resolve via registry (parses args) so commands like `:edit <path>` work.
        if self.registry.resolve(cmd).is_some() {
            self.dispatch_command(cmd, 40, 80);
        } else {
            self.command_error = format!("Not an editor command: {}", cmd);
        }
    }

    pub(crate) fn enter_nav_mode(&mut self, mode: NavMode, content_width: usize) {
        let buf = &mut self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return;
        }
        buf.nav_mode = mode;
        if mode == NavMode::Character {
            return;
        }
        buf.rebuild_nav_objects(&self.theme, content_width);
        let current_row = buf.rendered_cursor_row;
        if let Some(idx) = buf.nearest_object_index(current_row) {
            buf.nav_object_index = idx;
            let obj = &buf.nav_objects[idx];
            buf.rendered_cursor_row = obj.rendered_row;
            buf.rendered_cursor_col = obj.col_start;
        }
    }

    pub(crate) fn nav_activate(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return;
        }
        // In Character mode, try to open a link under the cursor
        if buf.nav_mode == NavMode::Character {
            if let Some(url) = self.link_under_rendered_cursor() {
                self.open_link(&url);
            }
            return;
        }
        let obj = match buf.nav_objects.get(buf.nav_object_index) {
            Some(o) => o.clone(),
            None => return,
        };
        match obj.kind {
            NavMode::Link => {
                self.open_link(&obj.action_data);
            }
            NavMode::Heading => {
                let buf = &mut self.buffers[self.active_buffer];
                buf.rendered_cursor_row = obj.rendered_row;
                buf.nav_mode = NavMode::Character;
            }
            NavMode::CodeBlock => {
                use std::io::Write;
                use std::process::{Command, Stdio};
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
                    && let Some(mut stdin) = child.stdin.take()
                {
                    let _ = stdin.write_all(obj.action_data.as_bytes());
                }
            }
            NavMode::ListItem => {
                // No-op for now
            }
            NavMode::Character => {}
        }
    }

    pub(crate) fn nav_move_next(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf
            .nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let next_pos = (pos + 1) % filtered.len();
        let next_idx = filtered[next_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = next_idx;
        let obj = &buf.nav_objects[next_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    pub(crate) fn nav_move_prev(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf
            .nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == mode)
            .map(|(i, _)| i)
            .collect();
        if filtered.is_empty() {
            return;
        }
        let current_idx = buf.nav_object_index;
        let pos = filtered.iter().position(|&i| i == current_idx).unwrap_or(0);
        let prev_pos = if pos == 0 {
            filtered.len() - 1
        } else {
            pos - 1
        };
        let prev_idx = filtered[prev_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = prev_idx;
        let obj = &buf.nav_objects[prev_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    pub(crate) fn open_link(&mut self, url: &str) {
        // Check if it's a local file path (relative or absolute, no scheme)
        let is_url = url.contains("://");
        if !is_url {
            // Resolve relative to the current file's directory
            let base_dir = self.buffers[self.active_buffer]
                .file_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let path = base_dir.join(url);
            if path.exists() {
                self.open_buffer(path);
                return;
            }
        }
        // External URL or file not found — open with system handler
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    /// Find the link URL under the rendered cursor, if any.
    pub(crate) fn link_under_rendered_cursor(&self) -> Option<String> {
        let buf = &self.buffers[self.active_buffer];
        if buf.view_mode != ViewMode::Rendered {
            return None;
        }
        let content_width = buf.viewport.content_width(200);
        let target_row = buf.rendered_cursor_row;
        let target_col = buf.rendered_cursor_col;

        let mut row = 0;
        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if row + h > target_row {
                // Target is in this block — get rendered lines
                let lines = yalda::view::render_block_to_lines(block, content_width, &self.theme);
                let line_idx = target_row - row;
                if let Some(line) = lines.get(line_idx) {
                    // Walk spans to find which one the cursor col falls in
                    let mut col = 0;
                    for span in &line.spans {
                        let span_len = span.text.chars().count();
                        if target_col >= col && target_col < col + span_len {
                            return span.link.clone();
                        }
                        col += span_len;
                    }
                }
                return None;
            }
            row += h;
        }
        None
    }

    pub(crate) fn ensure_rendered_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        let row = buf.rendered_cursor_row;
        self.buffers[self.active_buffer]
            .viewport
            .ensure_cursor_visible(row, viewport_height);
    }

    /// Move the cursor by `n` rows (down if `down`, else up), clamped to
    /// the visible content. Used by ctrl-d / ctrl-u / page motions —
    /// viewport scrolling then follows naturally via ensure_cursor_visible
    /// at the end of handle_key.
    ///
    /// Rendered and Raw modes track cursors in different coordinate spaces,
    /// so we move the appropriate one. Mixing them up (e.g. moving the doc
    /// cursor while in Rendered mode) leaves the visible cursor unchanged
    /// and ctrl-d/u looks like a no-op.
    pub(crate) fn page_move_cursor(&mut self, n: usize, down: bool) {
        let buf = &mut self.buffers[self.active_buffer];
        match buf.view_mode {
            ViewMode::Raw => {
                let editor = &mut buf.editor;
                let cur_line = editor.cursor().line;
                let max_line = editor.document().line_count().saturating_sub(1);
                let new_line = if down {
                    (cur_line + n).min(max_line)
                } else {
                    cur_line.saturating_sub(n)
                };
                editor.pre_move(false);
                editor.cursor_mut().line = new_line;
                editor.clamp_cursor_col(false);
            }
            ViewMode::Rendered => {
                let max_row = buf.viewport.total_lines.saturating_sub(1);
                let cur = buf.rendered_cursor_row;
                buf.rendered_cursor_row = if down {
                    (cur + n).min(max_row)
                } else {
                    cur.saturating_sub(n)
                };
            }
        }
    }

    pub(crate) fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        // Rendered mode tracks its cursor in its OWN coordinate space
        // (`rendered_cursor_row`); the doc cursor doesn't move on j/k/page
        // motions there. Translating doc line → rendered y is lossy and
        // contradicts whatever the rendered handlers just computed, so the
        // outer auto-pin would scroll the viewport back and leave the
        // rendered cursor off-screen.
        let rendered_y = match buf.view_mode {
            ViewMode::Raw => {
                yalda::buffer::raw_cursor_visual_row(&buf.editor, self.last_wrap_width.max(1))
            }
            ViewMode::Rendered => buf.rendered_cursor_row,
        };
        self.buffers[self.active_buffer]
            .viewport
            .ensure_cursor_visible(rendered_y, viewport_height);
    }

    pub(crate) fn perform_search(&mut self) {
        self.search_matches.clear();
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            return;
        }

        let buf = &self.buffers[self.active_buffer];
        let line_count = buf.editor.document().line_count();
        for line_idx in 0..line_count {
            let line_text = buf.editor.document().line_text(line_idx);
            let line_lower = line_text.to_lowercase();
            let mut start = 0;
            while let Some(pos) = line_lower[start..].find(&query) {
                self.search_matches.push((line_idx, start + pos));
                start += pos + query.len();
            }
        }

        // Jump to the first match at or after the current cursor position
        let buf = &self.buffers[self.active_buffer];
        let cursor_line = buf.editor.cursor().line;
        let cursor_col = buf.editor.cursor().col;
        self.search_match_index = self
            .search_matches
            .iter()
            .position(|&(line, col)| {
                line > cursor_line || (line == cursor_line && col >= cursor_col)
            })
            .unwrap_or(0);
    }

    pub(crate) fn jump_to_match(&mut self, viewport_height: usize) {
        if let Some(&(line_idx, col_idx)) = self.search_matches.get(self.search_match_index) {
            let buf = &mut self.buffers[self.active_buffer];
            buf.editor.cursor_mut().line = line_idx;
            buf.editor.cursor_mut().col = col_idx;
            if buf.view_mode == ViewMode::Rendered {
                // Find rendered position by counting matches (same as view does)
                if let Some((row, col)) = self.find_rendered_match_position(self.search_match_index)
                {
                    self.buffers[self.active_buffer].rendered_cursor_row = row;
                    self.buffers[self.active_buffer].rendered_cursor_col = col;
                }
                self.ensure_rendered_cursor_visible(viewport_height);
            } else {
                self.ensure_cursor_visible(viewport_height);
            }
        }
    }

    /// Find the rendered (row, col) of the Nth search match by scanning
    /// rendered lines the same way the view does.
    pub(crate) fn find_rendered_match_position(
        &self,
        match_index: usize,
    ) -> Option<(usize, usize)> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let query: Vec<char> = self.search_query.to_lowercase().chars().collect();
        let qlen = query.len();
        if qlen == 0 {
            return None;
        }

        let mut counter = 0;
        let mut row = 0;
        for block in &buf.rendered_cache {
            let lines = yalda::view::render_block_to_lines(block, content_width, &self.theme);
            for line in &lines {
                let lower: Vec<char> = line.text_content().to_lowercase().chars().collect();
                let mut ci = 0;
                while ci + qlen <= lower.len() {
                    if lower[ci..ci + qlen] == query[..] {
                        if counter == match_index {
                            return Some((row, ci));
                        }
                        counter += 1;
                        ci += qlen;
                    } else {
                        ci += 1;
                    }
                }
                row += 1;
            }
        }
        None
    }

    /// Find the y offset of the next heading after current scroll position.
    /// If `level_filter` is Some, only match headings at that level.
    pub(crate) fn find_next_heading(&self, level_filter: Option<u8>) -> Option<usize> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y > current
                && let RenderedBlock::Heading { level, .. } = block
                && (level_filter.is_none() || level_filter == Some(*level))
            {
                return Some(y);
            }
            y += h;
        }
        None
    }

    /// Find the y offset of the previous heading before current scroll position.
    /// If `level_filter` is Some, only match headings at that level.
    pub(crate) fn find_prev_heading(&self, level_filter: Option<u8>) -> Option<usize> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;
        let mut last_match = None;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y >= current {
                break;
            }
            if let RenderedBlock::Heading { level, .. } = block
                && (level_filter.is_none() || level_filter == Some(*level))
            {
                last_match = Some(y);
            }
            y += h;
        }
        last_match
    }

    pub(crate) fn heading_level_at_offset(&self) -> Option<u8> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y == current
                && let RenderedBlock::Heading { level, .. } = block
            {
                return Some(*level);
            }
            if y > current {
                break;
            }
            y += h;
        }
        None
    }
}
