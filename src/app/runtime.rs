use std::io;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use sketch::buffer::NavMode;
use sketch::keys::KeyPress;
use sketch::view::{self, ViewMode, ViewState};

use super::{App, AppMode, AppScreen};

impl App {
    /// When SKETCH_DEBUG=1, append one line of JSON to ~/.cache/sketch/debug.log
    /// for routine frames (1-in-5) plus EVERY frame where the cursor was off
    /// screen, plus EVERY frame where off-screen status flipped. Use to chase
    /// viewport mismatches:
    ///
    ///   tail -f ~/.cache/sketch/debug.log | jq .
    ///
    /// Compare `cursor_screen_y` (where it was painted, or null = off-screen)
    /// against `expected_cursor_visual_row + content_area_y`.
    pub(crate) fn write_debug_log(
        &mut self,
        report: &view::DrawReport,
        term_size: ratatui::prelude::Size,
    ) {
        if std::env::var("SKETCH_DEBUG").ok().as_deref() != Some("1") {
            return;
        }
        // Splash mode intentionally doesn't paint a cursor — never treat it
        // as "off-screen". Skip it entirely (don't even sample) so the log
        // isn't dominated by startup frames before the user opens a file.
        if report.is_splash {
            return;
        }
        let off_screen_now = report.cursor_screen_y.is_none();
        let flipped = self.debug_last_off_screen != off_screen_now;
        self.debug_last_off_screen = off_screen_now;
        // Build a cheap signature of the state-of-interest. Skip the write if
        // nothing actionable changed since last frame — but always log when
        // cursor is off-screen, or when off-screen status flipped.
        let buf = &self.buffers[self.active_buffer];
        let cursor_line = buf.editor.cursor().line;
        let cursor_col = buf.editor.cursor().col;
        let scroll_offset = buf.viewport.scroll_offset;
        let total_lines = buf.viewport.total_lines;
        let mut sig = 0u64;
        sig ^= (term_size.width as u64).wrapping_mul(0x9E3779B97F4A7C15);
        sig ^= (term_size.height as u64).wrapping_mul(0xBF58476D1CE4E5B9);
        sig ^= (scroll_offset as u64).wrapping_mul(0x94D049BB133111EB);
        sig ^= (cursor_line as u64).wrapping_mul(0xD6E8FEB86659FD93);
        sig ^= (cursor_col as u64).wrapping_mul(0x165667B19E3779F9);
        sig ^= (total_lines as u64).wrapping_mul(0x85EBCA77C2B2AE63);
        sig ^= (report.cursor_screen_y.unwrap_or(u16::MAX) as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
        sig ^= (report.painted_rows as u64).wrapping_mul(0x27D4EB2F165667C5);
        let force = off_screen_now || flipped;
        if !force && sig == self.debug_last_signature {
            return;
        }
        self.debug_last_signature = sig;
        let expected_visual_row = match buf.view_mode {
            ViewMode::Raw => {
                sketch::buffer::raw_cursor_visual_row(&buf.editor, self.last_wrap_width.max(1))
            }
            ViewMode::Rendered => buf.rendered_cursor_row,
        };
        let off_screen = off_screen_now;
        let cursor_y_str = match report.cursor_screen_y {
            Some(y) => y.to_string(),
            None => "null".to_string(),
        };
        let mode_str = format!("{:?}", self.mode);
        let view_mode_str = format!("{:?}", buf.view_mode);
        let line = format!(
            "{{\"ts\":{},\"term_w\":{},\"term_h\":{},\"computed_vh\":{},\"content_area_h\":{},\"scroll_offset\":{},\"total_lines\":{},\"cursor_line\":{},\"cursor_col\":{},\"expected_visual_row\":{},\"cursor_screen_y\":{},\"first_visible_doc_line\":{},\"last_visible_doc_line\":{},\"painted_rows\":{},\"off_screen\":{},\"mode\":\"{}\",\"view_mode\":\"{}\",\"frozen_lines\":{},\"lockable_through_line\":{}}}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            term_size.width,
            term_size.height,
            self.last_viewport_height,
            report.content_area_height,
            scroll_offset,
            total_lines,
            cursor_line,
            cursor_col,
            expected_visual_row,
            cursor_y_str,
            report
                .first_visible_doc_line
                .map(|n| n.to_string())
                .unwrap_or_else(|| "null".to_string()),
            report
                .last_visible_doc_line
                .map(|n| n.to_string())
                .unwrap_or_else(|| "null".to_string()),
            report.painted_rows,
            off_screen,
            mode_str,
            view_mode_str,
            buf.editor.frozen_lines().len(),
            buf.editor.lockable_through_line(),
        );
        // Best-effort write; never panic during render.
        let log_path = match dirs::cache_dir() {
            Some(d) => d.join("sketch").join("debug.log"),
            None => return,
        };
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Compute the content-area height the same way `view::draw` computes
    /// it. Returning the wrong value here is the single biggest cause of
    /// "cursor off the bottom" bugs — `ensure_cursor_visible` runs against
    /// this height with SCROLLOFF margin, so any mismatch with what the
    /// renderer actually paints lets the cursor sit outside the visible area.
    pub(crate) fn compute_viewport_height(&self, total_height: usize) -> usize {
        let top_bar = 1usize;
        let needs_bottom_bar = self.mode == AppMode::Command
            || self.search_input_mode
            || !self.command_error.is_empty();
        let bottom_bar = if needs_bottom_bar { 1 } else { 0 };
        // Buffer list panel
        let buffer_list = if self.mode == AppMode::FileBrowser {
            // BufferList isn't shown in file browser mode; treat as 0.
            0
        } else {
            0 // We never render buffer_list inline currently (full-screen variant).
        };
        // File browser inline panel
        let file_browser = if self.file_browser.is_some() {
            let max_height = total_height / 2;
            let header_rows = 1;
            let filter_rows = 0; // approximation; off in normal flow
            let entry_rows = self
                .file_browser
                .as_ref()
                .map(|fb| fb.entries().len())
                .unwrap_or(0);
            (header_rows + filter_rows + entry_rows)
                .min(max_height)
                .max(1)
        } else {
            0
        };
        // Outline inline panel
        let outline = if self.mode == AppMode::Outline {
            let max_height = total_height / 3;
            let header_rows = 1; // breadcrumb (approximation)
            let filter_rows = if self.outline_filter_mode { 1 } else { 0 };
            let entry_rows = self.filtered_outline_entries().len().max(1);
            (header_rows + filter_rows + entry_rows)
                .min(max_height)
                .max(1)
        } else {
            0
        };
        // Compose textbox panel
        let compose = if let Some(compose_textbox) = &self.compose_textbox {
            let lines = compose_textbox.editor.document().line_count().max(1);
            let capped = lines.min(total_height / 3).clamp(3, 12);
            capped + 1 // +1 for separator line
        } else {
            0
        };
        total_height
            .saturating_sub(top_bar)
            .saturating_sub(buffer_list)
            .saturating_sub(file_browser)
            .saturating_sub(outline)
            .saturating_sub(compose)
            .saturating_sub(bottom_bar)
    }

    /// Compute the wrap width currently used for raw-mode rendering. Mirrors
    /// the formula in `view::draw_content_raw`: terminal width minus the
    /// gutter (line numbers + space) and capped to `max_line_width`.
    pub(crate) fn compute_wrap_width(&self, terminal_width: usize) -> usize {
        let buf = &self.buffers[self.active_buffer];
        let total = buf.editor.document().line_count().max(1);
        let line_num_digits = total.ilog10() as usize + 1;
        let gutter_width = line_num_digits + 2;
        let text_area_width = terminal_width.saturating_sub(gutter_width + 1);
        buf.viewport.content_width(text_area_width).max(1)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let content_width = self.effective_content_width(size.width as usize);
        self.buffers[self.active_buffer].rebuild_render_cache(&self.theme);
        self.buffers[self.active_buffer].update_total_lines(content_width);

        let mut last_draw_report = view::DrawReport::default();

        loop {
            // Refresh viewport_height/wrap_width each tick (survives resize).
            let cur_size = terminal.size()?;
            let viewport_height = self.compute_viewport_height(cur_size.height as usize);
            self.last_viewport_height = viewport_height;
            self.last_wrap_width = self.compute_wrap_width(cur_size.width as usize);

            // Drain any pending Claude replies into *claude* buffer.
            self.pump_claude_replies(viewport_height);
            self.pump_acp_replies(viewport_height);

            // Rebuild render cache if needed
            if self.buffers[self.active_buffer].view_cache_dirty {
                self.buffers[self.active_buffer].rebuild_render_cache(&self.theme);
                self.buffers[self.active_buffer].view_cache_dirty = false;
            }

            // Build raw lines for raw mode (cheap — just reads from rope)
            let raw_lines: Vec<String> =
                if self.buffers[self.active_buffer].view_mode == ViewMode::Raw {
                    let doc = self.buffers[self.active_buffer].editor.document();
                    (0..doc.line_count())
                        .map(|i| {
                            let mut s = doc.line_text(i);
                            if s.ends_with('\n') {
                                s.pop();
                            }
                            s.replace('\t', "    ")
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            let raw_highlights: Vec<Vec<(String, sketch::style::Style)>> =
                if self.buffers[self.active_buffer].view_mode == ViewMode::Raw {
                    sketch::md_highlight::highlight_markdown_lines(&raw_lines, &self.theme)
                } else {
                    Vec::new()
                };

            terminal.draw(|frame| {
                let menu_nodes: Vec<(String, String, sketch::menu::MenuNodeKind)> =
                    if self.menu_state.is_active() {
                        self.menu_state
                            .current_nodes(&self.menu_tree)
                            .iter()
                            .map(|n| {
                                let key_display = sketch::keys::format_key_sequence(&n.key);
                                (key_display, n.label.clone(), n.kind())
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };

                let (fb_open, fb_dir, fb_entries, fb_filter_mode, fb_filter_text) = if self.mode
                    == AppMode::FileBrowser
                    && let Some(browser) = &self.file_browser
                {
                    let entries: Vec<(String, bool, bool)> = browser
                        .visible_entries()
                        .iter()
                        .enumerate()
                        .map(|(i, e)| (e.name.clone(), e.is_dir, i == browser.selected()))
                        .collect();
                    (
                        true,
                        browser.current_dir().display().to_string(),
                        entries,
                        browser.filter_mode,
                        browser.filter_text().to_string(),
                    )
                } else {
                    (false, String::new(), Vec::new(), false, String::new())
                };

                let full_buffer_list_state = if self.screen == AppScreen::BufferList {
                    let filtered = self.filtered_buffer_indices();
                    let entries: Vec<view::FullBufferListEntry> = filtered
                        .iter()
                        .enumerate()
                        .map(|(i, &buf_idx)| view::FullBufferListEntry {
                            path: self.buffers[buf_idx].file_path().display().to_string(),
                            is_modified: self.buffers[buf_idx].editor.document().is_modified(),
                            is_active: buf_idx == self.active_buffer,
                            is_selected: i == self.buffer_list_selected,
                        })
                        .collect();
                    Some(view::FullBufferListViewState {
                        entries,
                        filter_mode: self.buffer_list_filter_mode,
                        filter_text: self.buffer_list_filter_text.clone(),
                        total_count: self.buffers.len(),
                    })
                } else {
                    None
                };

                let full_browser_state =
                    if let AppScreen::FileBrowser { came_from_dropdown } = self.screen {
                        if let Some(browser) = &self.file_browser {
                            let entries: Vec<view::FullBrowserEntry> = browser
                                .visible_entries()
                                .iter()
                                .enumerate()
                                .map(|(i, e)| view::FullBrowserEntry {
                                    name: e.name.clone(),
                                    is_dir: e.is_dir,
                                    is_selected: i == browser.selected(),
                                    size: e.size,
                                    modified: e.modified,
                                })
                                .collect();
                            Some(view::FullBrowserViewState {
                                dir: browser.current_dir().display().to_string(),
                                entries,
                                filter_mode: browser.filter_mode,
                                filter_text: browser.filter_text().to_string(),
                                came_from_dropdown,
                                sort_label: browser.sort_order.label().to_string(),
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                let buf = &self.buffers[self.active_buffer];
                let filename_display = buf.editor.document().file_path.display().to_string();

                let compose_active = self.compose_textbox.is_some();
                let compose_lines: Vec<String> = if let Some(tb) = &self.compose_textbox {
                    let doc = tb.editor.document();
                    (0..doc.line_count())
                        .map(|i| {
                            let mut s = doc.line_text(i);
                            if s.ends_with('\n') {
                                s.pop();
                            }
                            s
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let compose_cursor_line = self
                    .compose_textbox
                    .as_ref()
                    .map(|tb| tb.editor.cursor().line)
                    .unwrap_or(0);
                let compose_cursor_col = self
                    .compose_textbox
                    .as_ref()
                    .map(|tb| tb.editor.cursor().col)
                    .unwrap_or(0);
                let compose_insert_mode = self
                    .compose_textbox
                    .as_ref()
                    .map(|tb| tb.mode == AppMode::Insert)
                    .unwrap_or(false);

                let state = ViewState {
                    filename: &filename_display,
                    modified: buf.editor.document().is_modified(),
                    view_mode: buf.view_mode,
                    rendered_blocks: &buf.rendered_cache,
                    raw_lines: &raw_lines,
                    raw_highlights: &raw_highlights,
                    viewport: &buf.viewport,
                    theme: &self.theme,
                    mode_label: if compose_active {
                        if compose_insert_mode {
                            "COMPOSE INSERT"
                        } else {
                            "COMPOSE"
                        }
                    } else {
                        match self.mode {
                            AppMode::Normal => match buf.view_mode {
                                ViewMode::Rendered => "NORMAL",
                                ViewMode::Raw => "RAW",
                            },
                            AppMode::Insert => "INSERT",
                            AppMode::Command => "NORMAL",
                            AppMode::Menu => "NORMAL",
                            AppMode::FileBrowser => "NORMAL",
                            AppMode::Outline => "OUTLINE",
                        }
                    },
                    cursor_line: buf.editor.cursor().line,
                    cursor_col: buf.editor.cursor().col,
                    show_block_cursor: self.mode != AppMode::Insert,
                    search_query: &self.search_query,
                    search_input_mode: self.search_input_mode,
                    search_input_buffer: &self.search_input_buffer,
                    search_match_count: self.search_matches.len(),
                    search_matches: &self.search_matches,
                    search_current_match: self.search_match_index,
                    rendered_cursor_row: self.buffers[self.active_buffer].rendered_cursor_row,
                    rendered_cursor_col: self.buffers[self.active_buffer].rendered_cursor_col,
                    menu_active: self.menu_state.is_active(),
                    menu_nodes,
                    menu_label: self.menu_state.current_label(&self.menu_tree),
                    file_browser_open: fb_open,
                    file_browser_dir: fb_dir,
                    file_browser_entries: fb_entries,
                    file_browser_filter_mode: fb_filter_mode,
                    file_browser_filter_text: fb_filter_text,
                    command_mode: self.mode == AppMode::Command,
                    command_buffer: &self.command_buffer,
                    command_error: &self.command_error,
                    buffer_list_open: false,
                    buffer_list_entries: Vec::new(),
                    buffer_list_filter_mode: false,
                    buffer_list_filter_text: String::new(),
                    buffer_count: self.buffers.len(),
                    active_buffer_index: self.active_buffer,
                    outline_open: self.mode == AppMode::Outline,
                    outline_entries: if self.mode == AppMode::Outline {
                        let entries = self.filtered_outline_entries();
                        entries
                            .iter()
                            .enumerate()
                            .map(|(i, e)| (e.title.clone(), e.level, i == self.outline_selected))
                            .collect()
                    } else {
                        Vec::new()
                    },
                    outline_filter_mode: self.outline_filter_mode,
                    outline_filter_text: self.outline_filter_text.clone(),
                    outline_breadcrumb: self.outline_breadcrumb(),
                    nav_mode_label: self.buffers[self.active_buffer]
                        .nav_mode
                        .label()
                        .map(|s| s.to_string()),
                    nav_highlight: {
                        let buf = &self.buffers[self.active_buffer];
                        if buf.nav_mode != NavMode::Character {
                            buf.nav_objects
                                .get(buf.nav_object_index)
                                .map(|obj| (obj.rendered_row, obj.col_start, obj.col_end))
                        } else {
                            None
                        }
                    },
                    full_browser: full_browser_state,
                    full_buffer_list: full_buffer_list_state,
                    selection: self.buffers[self.active_buffer].editor.selection_range(),
                    extend_mode: self.buffers[self.active_buffer].editor.extend_mode(),
                    frozen_ranges: self.buffers[self.active_buffer]
                        .editor
                        .frozen_ranges()
                        .to_vec(),
                    lockable_through_char: self.buffers[self.active_buffer]
                        .editor
                        .lockable_through_char(),
                    compose_active,
                    compose_lines,
                    compose_cursor_line,
                    compose_cursor_col,
                    compose_insert_mode,
                };
                let mut report = view::DrawReport::default();
                view::draw(frame, &state, &mut report);
                last_draw_report = report;
            })?;

            self.write_debug_log(&last_draw_report, cur_size);

            if self.should_quit {
                break;
            }

            let timeout = Duration::from_millis(100);

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key_event) => {
                        self.handle_key(KeyPress::from(key_event), terminal)?;
                    }
                    Event::Resize(w, h) => {
                        let cw = self.effective_content_width(w as usize);
                        // Refresh cached dims and re-flow every buffer (not
                        // just the active one — others get stale totals
                        // otherwise, breaking scroll math when the user
                        // switches to them).
                        let new_viewport_height = self.compute_viewport_height(h as usize);
                        self.last_viewport_height = new_viewport_height;
                        self.last_wrap_width = self.compute_wrap_width(w as usize);
                        for buf in self.buffers.iter_mut() {
                            buf.update_total_lines(cw);
                        }
                        // Re-pin the cursor inside the (possibly smaller)
                        // viewport so it doesn't end up off-screen.
                        self.ensure_cursor_visible(new_viewport_height);
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyPress, terminal: &DefaultTerminal) -> io::Result<()> {
        if !self.command_error.is_empty() && self.mode != AppMode::Command {
            self.command_error.clear();
        }

        let size = terminal.size()?;
        let viewport_height = self.compute_viewport_height(size.height as usize);
        let content_width = self.effective_content_width(size.width as usize);
        self.last_viewport_height = viewport_height;
        self.last_wrap_width = self.compute_wrap_width(size.width as usize);

        if let AppScreen::FileBrowser { .. } = self.screen {
            self.handle_full_browser_key(key, size.width, viewport_height, content_width);
            self.buffers[self.active_buffer].update_total_lines(content_width);
            return Ok(());
        }
        if let AppScreen::BufferList = self.screen {
            self.handle_full_buffer_list_key(key, viewport_height, content_width);
            self.buffers[self.active_buffer].update_total_lines(content_width);
            return Ok(());
        }

        // Compose textbox intercept: when open, all keys go to the compose
        // handler except Command mode (`:` opens command from compose normal).
        if self.compose_textbox.is_some() && self.mode != AppMode::Command {
            let is_insert = self.compose_textbox.as_ref().unwrap().mode == AppMode::Insert;
            if is_insert {
                self.handle_compose_insert_key(key, viewport_height);
            } else {
                self.handle_compose_normal_key(key, viewport_height);
            }
            self.buffers[self.active_buffer].update_total_lines(content_width);
            return Ok(());
        }

        match self.mode {
            AppMode::Normal => self.handle_normal_key(key, viewport_height, content_width),
            AppMode::Insert => self.handle_insert_key(key, viewport_height),
            AppMode::Command => self.handle_command_key(key),
            AppMode::Menu => self.handle_menu_key(key, viewport_height, content_width),
            AppMode::FileBrowser => {
                self.handle_file_browser_key(key, size.width, viewport_height, content_width)
            }
            AppMode::Outline => self.handle_outline_key(key, viewport_height, content_width),
        }

        // Recompute the active buffer's wrapped row total after the edit
        // so the scroll math sees the fresh state, then re-pin the cursor.
        // This is the structural guarantee that the cursor stays on-screen
        // after EVERY key — individual handlers used to call
        // ensure_cursor_visible inline, which was easy to miss
        // (handle_insert_key, leave_insert_mode, etc. didn't, and that's
        // how Enter / typing past the wrap edge ended up off the bottom).
        self.buffers[self.active_buffer].update_total_lines(content_width);
        // Only meaningful when there's a real document on screen — file
        // browser / buffer list / outline modes manage their own scroll.
        match self.mode {
            AppMode::Normal | AppMode::Insert | AppMode::Command => {
                self.ensure_cursor_visible(viewport_height);
            }
            _ => {}
        }
        Ok(())
    }
}
