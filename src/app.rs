use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use sketch::acp_channel::AcpChannelClient;
use sketch::blocks::RenderedBlock;
use sketch::buffer::Buffer;
use sketch::claude_channel::ChannelClient;
use sketch::command::CommandRegistry;
use sketch::config::Config;
use sketch::file_browser::FileBrowser;
use sketch::keybind::{Action, KeybindManager};
use sketch::menu::{self, MenuNode, MenuState};
use sketch::theme::Theme;
use sketch::buffer::NavMode;
use sketch::view::{self, ViewMode, ViewState};

#[derive(Debug, PartialEq)]
enum AppScreen {
    Editor,
    FileBrowser { came_from_dropdown: bool },
    BufferList,
}

#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Insert,
    Command,
    Menu,
    FileBrowser,
    Outline,
}

pub struct App {
    buffers: Vec<Buffer>,
    active_buffer: usize,
    max_line_width: usize,
    theme: Theme,
    keybinds: KeybindManager,
    registry: CommandRegistry,
    should_quit: bool,
    screen: AppScreen,
    search_query: String,
    search_input_mode: bool,
    search_input_buffer: String,
    search_matches: Vec<(usize, usize)>,
    search_match_index: usize,
    mode: AppMode,
    menu_state: MenuState,
    menu_tree: Vec<MenuNode>,
    file_browser: Option<FileBrowser>,
    command_buffer: String,
    command_error: String,
    buffer_list_selected: usize,
    buffer_list_filter_mode: bool,
    buffer_list_filter_text: String,
    outline_selected: usize,
    outline_filter_mode: bool,
    outline_filter_text: String,
    /// Stack of (heading_level, y_offset) for descended headings.
    /// Empty = top-level view. Last entry = current parent.
    outline_stack: Vec<(u8, usize)>,
    /// Saved scroll offset to restore if Esc without selecting
    outline_saved_scroll: usize,
    full_browser_pending_g: bool,
    pending_count: Option<usize>,
    /// Set after pressing f/F/t/T — the next keypress is consumed as the
    /// target character and the corresponding find motion is executed.
    pending_find_char: Option<Action>,
    /// SKETCH_DEBUG=1 state: dedupe identical frames so the log only grows
    /// when something changes (or when off-screen, which is always logged).
    debug_last_off_screen: bool,
    debug_last_signature: u64,
    /// Cached viewport height from the most recent input/draw cycle, so helper
    /// methods that don't take it as a parameter (like programmatic edits to
    /// the *claude* buffer) can still scroll the viewport.
    last_viewport_height: usize,
    /// Cached raw-mode wrap width (terminal width minus the gutter and
    /// max_line_width cap). Used by visual-row cursor math.
    last_wrap_width: usize,
    /// Live MCP channel connection to a `sketch-channel` server. When attached,
    /// `:claude-send` and `:claude-send-selection` push payloads to the server,
    /// which forwards them to Claude Code as `notifications/claude/channel`.
    /// Replies come back via the `reply` MCP tool and are appended to a
    /// `*claude*` buffer.
    claude_channel: Option<ChannelClient>,
    /// Alternative path: a Claude (or any ACP-compliant) agent spawned as a
    /// local subprocess and driven over the Agent Client Protocol over stdio.
    /// Coexists with `claude_channel` — both write into the same `*claude*`
    /// buffer; the user picks which one to attach. Replies are streamed in
    /// chunks and spliced via `append_to_claude_buffer`.
    acp_channel: Option<AcpChannelClient>,
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::from_name(config.theme);
        // Build keybinds from config
        let mut keybinds = if let Some(kb_config) = &config.keybinds {
            if kb_config.reset_defaults {
                KeybindManager::new(
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                )
            } else {
                KeybindManager::default()
            }
        } else {
            KeybindManager::default()
        };
        if let Some(kb_config) = &config.keybinds {
            keybinds.apply_bindings(&kb_config.bindings);
        }

        // Build menu from config
        let menu_tree = if let Some(menu_config) = &config.menu {
            if menu_config.reset_defaults {
                menu_config.nodes.clone()
            } else {
                merge_menu(menu::default_menu(), &menu_config.nodes)
            }
        } else {
            menu::default_menu()
        };

        let registry = CommandRegistry::default_registry();
        let buffer = Buffer::new(filename, markdown, config.max_line_width, &theme);

        Self {
            buffers: vec![buffer],
            active_buffer: 0,
            max_line_width: config.max_line_width,
            theme,
            keybinds,
            registry,
            should_quit: false,
            screen: AppScreen::Editor,
            search_query: String::new(),
            search_input_mode: false,
            search_input_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            mode: AppMode::Normal,
            menu_state: MenuState::new(),
            menu_tree,
            file_browser: None,
            command_buffer: String::new(),
            command_error: String::new(),
            buffer_list_selected: 0,
            buffer_list_filter_mode: false,
            buffer_list_filter_text: String::new(),
            outline_selected: 0,
            outline_filter_mode: false,
            outline_filter_text: String::new(),
            outline_stack: Vec::new(),
            outline_saved_scroll: 0,
            full_browser_pending_g: false,
            pending_count: None,
            pending_find_char: None,
            debug_last_off_screen: false,
            debug_last_signature: 0,
            claude_channel: None,
            acp_channel: None,
            last_viewport_height: 24,
            last_wrap_width: 80,
        }
    }

    /// When SKETCH_DEBUG=1, append one line of JSON to ~/.cache/sketch/debug.log
    /// for routine frames (1-in-5) plus EVERY frame where the cursor was off
    /// screen, plus EVERY frame where off-screen status flipped. Use to chase
    /// viewport mismatches:
    ///
    ///   tail -f ~/.cache/sketch/debug.log | jq .
    ///
    /// Compare `cursor_screen_y` (where it was painted, or null = off-screen)
    /// against `expected_cursor_visual_row + content_area_y`.
    fn write_debug_log(&mut self, report: &view::DrawReport, term_size: ratatui::prelude::Size) {
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
        sig ^= (report.cursor_screen_y.unwrap_or(u16::MAX) as u64)
            .wrapping_mul(0xC2B2AE3D27D4EB4F);
        sig ^= (report.painted_rows as u64).wrapping_mul(0x27D4EB2F165667C5);
        let force = off_screen_now || flipped;
        if !force && sig == self.debug_last_signature {
            return;
        }
        self.debug_last_signature = sig;
        let expected_visual_row = match buf.view_mode {
            ViewMode::Raw => sketch::buffer::raw_cursor_visual_row(
                &buf.editor,
                self.last_wrap_width.max(1),
            ),
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
    fn compute_viewport_height(&self, total_height: usize) -> usize {
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
            let entry_rows = self.file_browser.as_ref().map(|fb| fb.entries().len()).unwrap_or(0);
            (header_rows + filter_rows + entry_rows).min(max_height).max(1)
        } else {
            0
        };
        // Outline inline panel
        let outline = if self.mode == AppMode::Outline {
            let max_height = total_height / 3;
            let header_rows = 1; // breadcrumb (approximation)
            let filter_rows = if self.outline_filter_mode { 1 } else { 0 };
            let entry_rows = self.filtered_outline_entries().len().max(1);
            (header_rows + filter_rows + entry_rows).min(max_height).max(1)
        } else {
            0
        };
        total_height
            .saturating_sub(top_bar)
            .saturating_sub(buffer_list)
            .saturating_sub(file_browser)
            .saturating_sub(outline)
            .saturating_sub(bottom_bar)
    }

    /// Compute the wrap width currently used for raw-mode rendering. Mirrors
    /// the formula in `view::draw_content_raw`: terminal width minus the
    /// gutter (line numbers + space) and capped to `max_line_width`.
    fn compute_wrap_width(&self, terminal_width: usize) -> usize {
        let buf = &self.buffers[self.active_buffer];
        let total = buf.editor.document().line_count().max(1);
        let line_num_digits = total.ilog10() as usize + 1;
        let gutter_width = line_num_digits + 2;
        let text_area_width = terminal_width.saturating_sub(gutter_width + 1);
        buf.viewport.content_width(text_area_width).max(1)
    }

    #[allow(dead_code)]
    fn active(&self) -> &Buffer {
        &self.buffers[self.active_buffer]
    }

    #[allow(dead_code)]
    fn active_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_buffer]
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
            let raw_lines: Vec<String> = if self.buffers[self.active_buffer].view_mode == ViewMode::Raw {
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
                let menu_nodes: Vec<(String, String, sketch::menu::MenuNodeKind)> = if self.menu_state.is_active() {
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

                let (fb_open, fb_dir, fb_entries, fb_filter_mode, fb_filter_text) =
                    if self.mode == AppMode::FileBrowser && let Some(browser) = &self.file_browser {
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
                    let entries: Vec<view::FullBufferListEntry> = filtered.iter().enumerate().map(|(i, &buf_idx)| {
                        view::FullBufferListEntry {
                            path: self.buffers[buf_idx].file_path().display().to_string(),
                            is_modified: self.buffers[buf_idx].editor.document().is_modified(),
                            is_active: buf_idx == self.active_buffer,
                            is_selected: i == self.buffer_list_selected,
                        }
                    }).collect();
                    Some(view::FullBufferListViewState {
                        entries,
                        filter_mode: self.buffer_list_filter_mode,
                        filter_text: self.buffer_list_filter_text.clone(),
                        total_count: self.buffers.len(),
                    })
                } else {
                    None
                };

                let full_browser_state = if let AppScreen::FileBrowser { came_from_dropdown } = self.screen {
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

                let state = ViewState {
                    filename: &filename_display,
                    modified: buf.editor.document().is_modified(),
                    view_mode: buf.view_mode,
                    rendered_blocks: &buf.rendered_cache,
                    raw_lines: &raw_lines,
                    raw_highlights: &raw_highlights,
                    viewport: &buf.viewport,
                    theme: &self.theme,
                    mode_label: match self.mode {
                        AppMode::Normal => match buf.view_mode {
                            ViewMode::Rendered => "NORMAL",
                            ViewMode::Raw => "RAW",
                        },
                        AppMode::Insert => "INSERT",
                        AppMode::Command => "NORMAL",
                        AppMode::Menu => "NORMAL",
                        AppMode::FileBrowser => "NORMAL",
                        AppMode::Outline => "OUTLINE",
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
                        entries.iter().enumerate().map(|(i, e)| {
                            (e.title.clone(), e.level, i == self.outline_selected)
                        }).collect()
                    } else {
                        Vec::new()
                    },
                    outline_filter_mode: self.outline_filter_mode,
                    outline_filter_text: self.outline_filter_text.clone(),
                    outline_breadcrumb: self.outline_breadcrumb(),
                    nav_mode_label: self.buffers[self.active_buffer].nav_mode.label()
                        .map(|s| s.to_string()),
                    nav_highlight: {
                        let buf = &self.buffers[self.active_buffer];
                        if buf.nav_mode != NavMode::Character {
                            buf.nav_objects.get(buf.nav_object_index).map(|obj| {
                                (obj.rendered_row, obj.col_start, obj.col_end)
                            })
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
                    Event::Key(key_event) => self.handle_key(key_event, terminal)?,
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


    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
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

    fn handle_normal_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        if self.search_input_mode {
            match key.code {
                KeyCode::Enter => {
                    self.search_query = self.search_input_buffer.clone();
                    self.search_input_mode = false;
                    self.perform_search();
                    self.jump_to_match(viewport_height);
                }
                KeyCode::Esc => {
                    self.search_input_mode = false;
                    self.search_input_buffer.clear();
                }
                KeyCode::Backspace => {
                    self.search_input_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.search_input_buffer.push(c);
                }
                _ => {}
            }
            return;
        }

        // Pending f/F/t/T — consume the next character as the target.
        if let Some(pending) = self.pending_find_char.take() {
            if let KeyCode::Char(c) = key.code {
                self.execute_find_char(pending, c, viewport_height);
                return;
            }
            // Any non-Char key cancels the pending find and falls through.
        }

        if key.code == KeyCode::Esc {
            self.pending_count = None;
            // Clear selection / exit extend mode in raw mode
            let editor = &mut self.buffers[self.active_buffer].editor;
            editor.set_extend_mode(false);
            editor.clear_selection();
            if self.buffers[self.active_buffer].nav_mode != NavMode::Character
                && self.buffers[self.active_buffer].view_mode == ViewMode::Rendered
            {
                self.buffers[self.active_buffer].nav_mode = NavMode::Character;
            }
            return;
        }

        // Accumulate numeric prefix (1-9 start, 0 appends)
        if let KeyCode::Char(c) = key.code {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                if c.is_ascii_digit() && (self.pending_count.is_some() || c != '0') {
                    let digit = c as usize - '0' as usize;
                    self.pending_count = Some(self.pending_count.unwrap_or(0) * 10 + digit);
                    return;
                }
            }
        }

        // G with a count = go to line N
        if let KeyCode::Char('G') = key.code {
            if let Some(n) = self.pending_count.take() {
                let target = n.saturating_sub(1);
                self.goto_line(target, viewport_height);
                self.keybinds.reset_pending();
                return;
            }
        }

        let count = self.pending_count.take();
        let _ = count; // future: pass count to repeated motions

        if let Some(cmd_string) = self.keybinds.process_key(key) {
            self.dispatch_command(&cmd_string, viewport_height, content_width);
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent, viewport_height: usize) {
        match key.code {
            KeyCode::Esc => {
                self.leave_insert_mode();
            }
            KeyCode::Enter => {
                let indent = self.current_line_indent();
                self.buffers[self.active_buffer].editor.insert_char('\n');
                for _ in 0..indent.len() {
                    self.buffers[self.active_buffer].editor.insert_char(' ');
                }
            }
            KeyCode::Tab => {
                for _ in 0..2 {
                    self.buffers[self.active_buffer].editor.insert_char(' ');
                }
            }
            KeyCode::BackTab => {
                self.dedent_at_cursor();
            }
            KeyCode::Backspace => {
                self.backspace_smart();
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    match c {
                        '0' => self.set_heading_level(0),
                        '1' => self.set_heading_level(1),
                        '2' => self.set_heading_level(2),
                        '3' => self.set_heading_level(3),
                        '4' => self.set_heading_level(4),
                        '5' => self.set_heading_level(5),
                        '6' => self.set_heading_level(6),
                        _ => {}
                    }
                } else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'v' {
                    self.leave_insert_mode();
                    let buf = &mut self.buffers[self.active_buffer];
                    buf.view_mode = match buf.view_mode {
                        ViewMode::Rendered => ViewMode::Raw,
                        ViewMode::Raw => ViewMode::Rendered,
                    };
                    self.buffers[self.active_buffer].view_cache_dirty = true;
                } else {
                    self.buffers[self.active_buffer].editor.insert_char(c);
                }
            }
            KeyCode::Left => {
                self.buffers[self.active_buffer].editor.cursor_mut().move_left();
            }
            KeyCode::Right => {
                self.buffers[self.active_buffer].editor.move_right_clamped(true);
            }
            KeyCode::Up => {
                self.buffers[self.active_buffer].editor.cursor_mut().move_up();
                self.buffers[self.active_buffer].editor.clamp_cursor_col(true);
            }
            KeyCode::Down => {
                self.buffers[self.active_buffer].editor.move_down(true);
            }
            _ => {}
        }
        self.ensure_cursor_visible(viewport_height);
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                let cmd = self.command_buffer.clone();
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
                self.execute_command(&cmd);
            }
            KeyCode::Backspace => {
                self.command_buffer.pop();
                if self.command_buffer.is_empty() {
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            }
            _ => {}
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent, viewport_height: usize, content_width: usize) {
        match key.code {
            KeyCode::Esc => {
                self.menu_state.handle_escape();
                if !self.menu_state.is_active() {
                    self.mode = AppMode::Normal;
                }
            }
            _ => {
                if let Some(cmd_string) = self.menu_state.process_key_event(key, &self.menu_tree) {
                    self.mode = AppMode::Normal;
                    self.dispatch_command(&cmd_string, viewport_height, content_width);
                }
            }
        }
    }

    fn handle_file_browser_key(
        &mut self,
        key: KeyEvent,
        term_width: u16,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        let _ = term_width;

        let browser = match &mut self.file_browser {
            Some(b) => b,
            None => {
                self.mode = AppMode::Normal;
                return;
            }
        };

        if browser.filter_mode {
            match key.code {
                KeyCode::Esc => {
                    // Exit file browser entirely
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                    return;
                }
                KeyCode::Enter => {
                    let count = browser.visible_entries().len();
                    if count == 1 {
                        // Single result — open it directly
                        if let Some(path) = browser.enter_selected()
                            && self.open_buffer(path)
                        {
                            self.file_browser = None;
                            self.mode = AppMode::Normal;
                        }
                    } else if count > 0 {
                        // Multiple results — exit filter mode, navigate the list
                        browser.filter_mode = false;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    let mut text = browser.filter_text().to_string();
                    text.pop();
                    browser.set_filter(&text);
                }
                KeyCode::Char(c) => {
                    let mut text = browser.filter_text().to_string();
                    text.push(c);
                    browser.set_filter(&text);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('j') => browser.move_down(),
            KeyCode::Char('k') => browser.move_up(),
            KeyCode::Char('l') | KeyCode::Char(' ') => {
                if let Some(path) = browser.enter_selected()
                    && self.open_buffer(path)
                {
                    self.file_browser = None;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('h') | KeyCode::Backspace => browser.go_parent(),
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Char('/') => browser.filter_mode = true,
            KeyCode::Char('q') | KeyCode::Esc => {
                self.file_browser = None;
                self.mode = AppMode::Normal;
            }
            KeyCode::Tab => {
                self.open_file_browser_full(true);
            }
            _ => {}
        }
    }

    fn handle_full_browser_key(
        &mut self,
        key: KeyEvent,
        _term_width: u16,
        _viewport_height: usize,
        _content_width: usize,
    ) {
        let browser = match &mut self.file_browser {
            Some(b) => b,
            None => {
                self.screen = AppScreen::Editor;
                self.mode = AppMode::Normal;
                return;
            }
        };

        if browser.filter_mode {
            match key.code {
                KeyCode::Esc => {
                    self.close_full_browser();
                    return;
                }
                KeyCode::Enter => {
                    let count = browser.visible_entries().len();
                    if count == 1 {
                        if let Some(path) = browser.enter_selected() {
                            if self.open_buffer(path) {
                                self.screen = AppScreen::Editor;
                                self.mode = AppMode::Normal;
                            }
                        }
                    } else if count > 0 {
                        browser.filter_mode = false;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    let mut text = browser.filter_text().to_string();
                    text.pop();
                    browser.set_filter(&text);
                }
                KeyCode::Char(c) => {
                    let mut text = browser.filter_text().to_string();
                    text.push(c);
                    browser.set_filter(&text);
                }
                _ => {}
            }
            return;
        }

        // Normal mode
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => browser.move_down(),
            KeyCode::Char('k') | KeyCode::Up => browser.move_up(),
            KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(path) = browser.enter_selected() {
                    if self.open_buffer(path) {
                        self.screen = AppScreen::Editor;
                        self.mode = AppMode::Normal;
                    }
                }
            }
            KeyCode::Char('o') => {
                // Open file but stay in browser
                if let Some(path) = browser.enter_selected() {
                    let _ = self.open_buffer(path);
                    // Stay in full browser screen
                }
            }
            KeyCode::Char('h') | KeyCode::Char('-') | KeyCode::Backspace => browser.go_parent(),
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Char('s') => browser.cycle_sort(),
            KeyCode::Char('/') => browser.filter_mode = true,
            KeyCode::Char('G') => {
                let len = browser.visible_entries().len();
                if len > 0 {
                    browser.set_selected(len - 1);
                }
            }
            KeyCode::Char('g') => {
                if self.full_browser_pending_g {
                    // gg — jump to first entry
                    browser.set_selected(0);
                    self.full_browser_pending_g = false;
                } else {
                    self.full_browser_pending_g = true;
                    return; // wait for next key
                }
            }
            KeyCode::Tab => {
                if let AppScreen::FileBrowser { came_from_dropdown: true } = self.screen {
                    self.close_full_browser();
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.close_full_browser();
            }
            _ => {
                self.full_browser_pending_g = false;
            }
        }
    }

    fn close_full_browser(&mut self) {
        match self.screen {
            AppScreen::FileBrowser { came_from_dropdown: true } => {
                self.screen = AppScreen::Editor;
                // file_browser stays Some, mode stays FileBrowser for dropdown
                self.mode = AppMode::FileBrowser;
            }
            AppScreen::FileBrowser { came_from_dropdown: false } => {
                self.screen = AppScreen::Editor;
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    /// Look up a command name in the registry and dispatch its action.
    /// Execute a pending f/F/t/T motion with the captured target character.
    fn execute_find_char(&mut self, action: Action, ch: char, viewport_height: usize) {
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

    fn dispatch_command(
        &mut self,
        cmd_input: &str,
        viewport_height: usize,
        content_width: usize,
    ) {
        if let Some((action, args)) = self.registry.resolve(cmd_input) {
            if action == Action::Reload {
                if let Some(path_arg) = args.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    self.edit_path(path_arg);
                    return;
                }
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

    fn attach_claude_channel(&mut self, path_str: &str) {
        // Always drop any existing connection so re-running :claude-attach is
        // a clean recovery from staleness (e.g. after Claude restart, sketch
        // is talking to a now-dead sketch-channel that the kernel hasn't
        // bubbled up as broken yet).
        let had_existing = self.claude_channel.take().is_some();

        // Empty string → use the default socket path.
        let path = if path_str.is_empty() {
            ChannelClient::default_socket_path()
        } else {
            self.resolve_user_path(path_str)
        };
        match ChannelClient::connect(&path) {
            Ok(client) => {
                self.claude_channel = Some(client);
                let idx = self.or_create_claude_buffer();
                self.active_buffer = idx;
                self.command_error = if had_existing {
                    format!(
                        "Re-attached to Claude channel: {} (replaced previous connection)",
                        path.display()
                    )
                } else {
                    format!("Attached to Claude channel: {}", path.display())
                };
            }
            Err(e) => {
                self.command_error = format!(
                    "claude-attach: connect failed for {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    fn resolve_user_path(&self, path_str: &str) -> std::path::PathBuf {
        let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(rest)
            } else {
                std::path::PathBuf::from(path_str)
            }
        } else {
            std::path::PathBuf::from(path_str)
        };
        if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded))
                .unwrap_or(expanded)
        }
    }

    /// Returns `true` iff the payload reached the channel server.
    fn claude_send_text(&mut self, text: &str, label: &str) -> bool {
        let buffer_path = self.buffers[self.active_buffer]
            .file_path()
            .display()
            .to_string();
        let mut meta = std::collections::HashMap::new();
        // meta keys must be alphanumeric/underscore (Claude Code constraint).
        meta.insert("label".to_string(), label.to_string());
        meta.insert("file".to_string(), buffer_path.clone());
        let len = text.len();
        let outcome = match &mut self.claude_channel {
            Some(client) => match client.send(text, meta) {
                Ok(()) => Ok(len),
                Err(e) => Err(format!("send error: {}", e)),
            },
            None => Err("No Claude channel attached. Use :claude-attach first.".to_string()),
        };
        match outcome {
            Ok(n) => {
                self.command_error =
                    format!("Sent {} ({} chars) to Claude channel", label, n);
                true
            }
            Err(msg) => {
                if msg.contains("send error") {
                    // Drop the broken connection so the user can re-attach.
                    self.claude_channel = None;
                }
                self.command_error = msg;
                false
            }
        }
    }

    fn claude_send_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let is_claude_buffer = buf.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME;

        let payload = if is_claude_buffer {
            // Walk the active region (>= lockable) and collect contiguous runs
            // of editable chars (those NOT inside any frozen range). These are
            // the user's inline insertions — Claude's text is excluded.
            buf.editor.extract_editable_inserts()
        } else {
            buf.editor.document().full_text().trim().to_string()
        };

        if payload.is_empty() {
            self.command_error = if is_claude_buffer {
                "Nothing to send (no inline edits in this turn).".into()
            } else {
                "Nothing to send (buffer is empty).".into()
            };
            return;
        }

        let sent = self.claude_send_text(&payload, "buffer");

        if is_claude_buffer && sent {
            // Lock the turn: append HR and bump lockable to past it.
            self.lock_active_turn();
            self.ensure_cursor_visible(self.last_viewport_height);
        }
    }

    fn claude_send_selection(&mut self) {
        let sel = self.buffers[self.active_buffer]
            .editor
            .selection_text();
        match sel {
            Some(t) if !t.is_empty() => {
                self.claude_send_text(&t, "selection");
            }
            _ => {
                self.command_error =
                    "No selection. Make one first (e.g. `v` then a motion).".to_string();
            }
        }
    }

    /// Drain any pending replies from the Claude channel into the *claude* buffer.
    fn pump_claude_replies(&mut self, viewport_height: usize) {
        // If the reader thread saw EOF since last tick, the server is gone.
        // Drop the handle and surface a one-shot status message so the user
        // knows to `:claude-attach` again.
        let stale = self
            .claude_channel
            .as_ref()
            .map(|c| !c.is_connected())
            .unwrap_or(false);
        if stale {
            self.claude_channel = None;
            self.command_error =
                "Claude channel went stale (server gone). Run :claude-attach to recover."
                    .into();
        }

        let mut received: Vec<String> = Vec::new();
        if let Some(client) = &self.claude_channel {
            while let Some(text) = client.try_recv() {
                received.push(text);
            }
        }
        if received.is_empty() {
            return;
        }
        for text in received {
            self.append_to_claude_buffer(&text);
        }
        // Scroll the *claude* buffer's viewport to follow the new content,
        // even if the user is currently editing a different buffer.
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
        {
            self.ensure_buffer_cursor_visible(idx, viewport_height);
        }
    }

    /// Spawn an ACP agent subprocess and complete the handshake. Mirrors
    /// `attach_claude_channel` for the UNIX-socket path: any existing
    /// connection is dropped first, the *claude* buffer is created/focused
    /// on success, and `command_error` is used as a status line.
    ///
    /// `command_str` is shell-parsed; empty means "use the default agent
    /// (`claude-agent-acp`)". The `SKETCH_ACP_AGENT` env var is honoured if
    /// no command is given.
    fn attach_acp_channel(&mut self, command_str: &str) {
        let had_existing = self.acp_channel.take().is_some();
        let resolved = if command_str.is_empty() {
            std::env::var("SKETCH_ACP_AGENT").unwrap_or_default()
        } else {
            command_str.to_string()
        };
        let cwd = std::env::current_dir().ok();
        match AcpChannelClient::spawn(&resolved, cwd) {
            Ok(client) => {
                let label = client.description();
                self.acp_channel = Some(client);
                let idx = self.or_create_claude_buffer();
                self.active_buffer = idx;
                self.command_error = if had_existing {
                    format!("Re-attached to {label} (replaced previous)")
                } else {
                    format!("Attached to {label}")
                };
            }
            Err(e) => {
                self.command_error = format!("claude-acp-attach failed: {e}");
            }
        }
    }

    /// Send `text` as an ACP prompt. Returns true on success.
    fn acp_send_text(&mut self, text: &str, label: &str) -> bool {
        let outcome = match &mut self.acp_channel {
            Some(client) => match client.send(text) {
                Ok(()) => Ok(text.len()),
                Err(e) => Err(format!("ACP send error: {e}")),
            },
            None => Err(
                "No ACP agent attached. Use :claude-acp-attach first.".to_string(),
            ),
        };
        match outcome {
            Ok(n) => {
                self.command_error =
                    format!("Sent {label} ({n} chars) to ACP agent");
                true
            }
            Err(msg) => {
                if msg.contains("ACP send error") {
                    self.acp_channel = None;
                }
                self.command_error = msg;
                false
            }
        }
    }

    fn acp_send_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let is_claude_buffer = buf.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME;

        let payload = if is_claude_buffer {
            buf.editor.extract_editable_inserts()
        } else {
            buf.editor.document().full_text().trim().to_string()
        };

        if payload.is_empty() {
            self.command_error = if is_claude_buffer {
                "Nothing to send (no inline edits in this turn).".into()
            } else {
                "Nothing to send (buffer is empty).".into()
            };
            return;
        }

        let sent = self.acp_send_text(&payload, "buffer");

        if is_claude_buffer && sent {
            // Same lock-and-advance behaviour as the UNIX-socket path so
            // the user can keep typing while Claude works on the prior turn.
            self.lock_active_turn();
            self.ensure_cursor_visible(self.last_viewport_height);
        }
    }

    fn acp_send_selection(&mut self) {
        let sel = self.buffers[self.active_buffer]
            .editor
            .selection_text();
        match sel {
            Some(t) if !t.is_empty() => {
                self.acp_send_text(&t, "selection");
            }
            _ => {
                self.command_error =
                    "No selection. Make one first (e.g. `v` then a motion).".to_string();
            }
        }
    }

    /// Drain any streamed reply chunks from the ACP worker into the
    /// *claude* buffer. Identical splice behaviour to `pump_claude_replies`.
    fn pump_acp_replies(&mut self, viewport_height: usize) {
        let stale = self
            .acp_channel
            .as_ref()
            .map(|c| !c.is_connected())
            .unwrap_or(false);
        if stale {
            self.acp_channel = None;
            self.command_error =
                "ACP agent went away. Run :claude-acp-attach to recover.".into();
        }

        let mut received: Vec<String> = Vec::new();
        if let Some(client) = &self.acp_channel {
            while let Some(text) = client.try_recv() {
                received.push(text);
            }
        }
        if received.is_empty() {
            return;
        }
        for text in received {
            self.append_to_claude_buffer(&text);
        }
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
        {
            self.ensure_buffer_cursor_visible(idx, viewport_height);
        }
    }

    /// Locate (or lazily create) the special *claude* buffer.
    fn or_create_claude_buffer(&mut self) -> usize {
        if let Some(i) = self
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
        {
            return i;
        }
        let mut buf = Buffer::new(
            CLAUDE_BUFFER_NAME.to_string(),
            String::new(),
            self.max_line_width,
            &self.theme,
        );
        // Default new buffers to Rendered, but the *claude* transcript is a
        // chat-style raw editor — viewport scrolling tracks doc lines there.
        buf.view_mode = ViewMode::Raw;
        self.buffers.push(buf);
        self.buffers.len() - 1
    }

    /// Ensure the cursor of buffer `buf_idx` is visible in its own viewport.
    /// Works for non-active buffers too, so a Claude reply landing in *claude*
    /// while the user is in another buffer still scrolls *claude* into place.
    fn ensure_buffer_cursor_visible(&mut self, buf_idx: usize, viewport_height: usize) {
        let wrap_width = self.last_wrap_width.max(1);
        let buf = &mut self.buffers[buf_idx];
        let rendered_y = match buf.view_mode {
            ViewMode::Raw => sketch::buffer::raw_cursor_visual_row(&buf.editor, wrap_width),
            // Skip for Rendered mode — it uses rendered_cursor_row which we
            // don't update for programmatic edits.
            ViewMode::Rendered => return,
        };
        // Also keep the buffer's own total_lines fresh so the scroll math is
        // correct for buffers that aren't currently active (we only update the
        // active one in the main loop).
        buf.update_total_lines(wrap_width);
        buf.viewport
            .ensure_cursor_visible(rendered_y, viewport_height);
    }

    /// Splice a Claude reply into the *claude* buffer above any pending draft.
    ///
    /// Buffer layout we maintain:
    /// ```text
    /// [turn 1]              ← frozen
    /// \n---\n               ← frozen (the HR line)
    /// [turn 2]              ← frozen
    /// \n---\n               ← frozen
    /// [draft]               ← editable
    /// ```
    /// On reply: take the current draft (everything past the frozen HR), and
    /// rewrite the editable region to `claude_text + HR + draft`. Advance the
    /// frozen line to the new HR. The user's pending draft is preserved.
    /// A new Claude reply landed. Splice it into the *claude* buffer ABOVE
    /// any pending user draft, so the user can keep typing while Claude is
    /// working and have the reply slot in above without disrupting them.
    ///
    /// Layout maintained:
    /// ```text
    /// [prior turns]      ← locked (lockable_through_line + frozen ranges)
    /// [new reply]        ← inserted here, marked frozen
    /// [user draft]       ← preserved verbatim, cursor stays on same char
    /// ```
    /// Streaming-safe: each chunk re-finds the splice point so subsequent
    /// chunks slot in just after the prior chunk, not below the draft.
    fn append_to_claude_buffer(&mut self, text: &str) {
        let trimmed = text.trim_end_matches('\n');
        if trimmed.is_empty() {
            return;
        }

        let buf_idx = self.or_create_claude_buffer();
        let buf = &mut self.buffers[buf_idx];
        let editor = &mut buf.editor;

        let total_len = editor.document().rope().len_chars();
        let buffer_was_empty = total_len == 0;

        // Splice point: end of all locked content. Below this is the user's
        // editable draft (possibly empty). Take the max of lockable_through
        // and the end of the last frozen range — either can be further down.
        let lockable = editor.lockable_through_char();
        let frozen_end_line = editor
            .frozen_lines()
            .iter()
            .map(|&(_, e)| e)
            .max()
            .unwrap_or(0);
        let frozen_end_char = if frozen_end_line == 0 {
            0
        } else if frozen_end_line >= editor.document().line_count() {
            total_len
        } else {
            editor.document().line_col_to_char(frozen_end_line, 0)
        };
        let splice_at = lockable.max(frozen_end_char).min(total_len);

        // Capture the draft and where the cursor sits relative to it.
        let draft_text: String = editor
            .document()
            .rope()
            .slice(splice_at..total_len)
            .to_string();
        let cursor_char = editor
            .document()
            .line_col_to_char(editor.cursor().line, editor.cursor().col);
        let cursor_in_draft = cursor_char.saturating_sub(splice_at);
        let cursor_was_in_draft = cursor_char >= splice_at;

        // Strip the draft so the reply can append at end-of-locked-region;
        // we re-attach it after the reply is in place and frozen.
        if !draft_text.is_empty() {
            editor.programmatic_delete(splice_at, total_len);
        }

        // Pad so the reply starts on its own line(s).
        let pre_len = editor.document().rope().len_chars();
        let pad = if buffer_was_empty || pre_len == 0 {
            String::new()
        } else {
            let s = editor.document().full_text();
            let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
            "\n".repeat(2usize.saturating_sub(trailing_nl))
        };
        let trailing_pad = if trimmed.ends_with('\n') { "" } else { "\n" };
        let payload = format!("{}{}{}", pad, trimmed, trailing_pad);
        editor.programmatic_insert(pre_len, &payload);

        // Freeze whole lines covering the reply.
        let claude_start_char = pre_len + pad.chars().count();
        let claude_end_char = claude_start_char + trimmed.chars().count() + trailing_pad.len();
        let start_line = char_to_line_col(editor.document(), claude_start_char).0;
        let end_line = char_to_line_col(editor.document(), claude_end_char).0;
        editor.add_frozen_lines(start_line, end_line);

        // Re-attach the draft below the freshly-frozen reply and re-pin the
        // cursor onto the same character of the draft it was on before.
        let draft_reattach_at = editor.document().rope().len_chars();
        if !draft_text.is_empty() {
            editor.programmatic_insert(draft_reattach_at, &draft_text);
        }

        if cursor_was_in_draft {
            let new_cursor_char = if draft_text.is_empty() {
                editor.document().rope().len_chars()
            } else {
                draft_reattach_at + cursor_in_draft
            };
            let (cl, cc) = char_to_line_col(editor.document(), new_cursor_char);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = cc;
        }
        // else: cursor was inside locked content (e.g. user navigated up to
        // inline-edit a prior turn). Leave it where it is — programmatic
        // splice happened below the cursor's line.
        editor.clear_selection();
        buf.view_cache_dirty = true;
    }

    /// Lock the active turn: append `\n\n---\n\n` and bump `lockable_through_char`
    /// to the new EOF. After this, the user can't edit the just-sent content.
    fn lock_active_turn(&mut self) {
        let buf_idx = self.active_buffer;
        let buf = &mut self.buffers[buf_idx];
        let editor = &mut buf.editor;

        let pre_len = editor.document().rope().len_chars();
        // Always end with exactly two newlines after the HR for clean spacing.
        let s = editor.document().full_text();
        let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
        let lead = "\n".repeat(2usize.saturating_sub(trailing_nl));
        let separator = format!("{}---\n\n", lead);
        editor.programmatic_insert(pre_len, &separator);

        // Cursor goes to EOF (start of next active turn — empty until reply).
        // Lock everything ABOVE the cursor's line; the cursor's own line stays
        // editable so the user can immediately keep typing.
        let eof = editor.document().rope().len_chars();
        let (cl, cc) = char_to_line_col(editor.document(), eof);
        editor.set_lockable_through_line(cl);
        editor.cursor_mut().line = cl;
        editor.cursor_mut().col = cc;
        editor.clear_selection();
        buf.view_cache_dirty = true;
    }

    fn leave_insert_mode(&mut self) {
        self.buffers[self.active_buffer].editor.end_insert();
        self.mode = AppMode::Normal;
        self.buffers[self.active_buffer].view_cache_dirty = true;
        if self.buffers[self.active_buffer].editor.cursor().col > 0 {
            self.buffers[self.active_buffer].editor.cursor_mut().move_left();
        }
    }

    fn current_line_indent(&self) -> String {
        let editor = &self.buffers[self.active_buffer].editor;
        let line = editor.document().line_text(editor.cursor().line);
        let indent_len = line.len() - line.trim_start().len();
        line[..indent_len].replace('\t', "  ")
    }

    fn backspace_smart(&mut self) {
        let editor = &self.buffers[self.active_buffer].editor;
        let col = editor.cursor().col;
        if col == 0 {
            self.buffers[self.active_buffer].editor.backspace();
            return;
        }
        let line = editor.document().line_text(editor.cursor().line);
        let before_cursor = &line[..col.min(line.len())];
        // If everything before cursor is whitespace, remove up to 4 spaces
        if before_cursor.chars().all(|c| c == ' ') {
            let remove = if col % 2 == 0 { 2 } else { 1 };
            let remove = remove.min(col);
            for _ in 0..remove {
                self.buffers[self.active_buffer].editor.backspace();
            }
        } else {
            self.buffers[self.active_buffer].editor.backspace();
        }
    }

    fn dedent_at_cursor(&mut self) {
        let editor = &self.buffers[self.active_buffer].editor;
        let line = editor.document().line_text(editor.cursor().line);
        let indent_len = line.chars().take_while(|c| *c == ' ').count();
        let remove = if indent_len % 2 == 0 { 2.min(indent_len) } else { 1 };
        if remove == 0 {
            return;
        }
        // Move cursor to the beginning of indent and delete spaces
        let orig_col = editor.cursor().col;
        let editor = &mut self.buffers[self.active_buffer].editor;
        editor.cursor_mut().col = remove;
        for _ in 0..remove {
            editor.backspace();
        }
        editor.cursor_mut().col = orig_col.saturating_sub(remove);
    }

    fn goto_line(&mut self, line: usize, viewport_height: usize) {
        let doc = self.buffers[self.active_buffer].editor.document();
        let max_line = doc.line_count().saturating_sub(1);
        let target = line.min(max_line);
        self.buffers[self.active_buffer].editor.cursor_mut().line = target;
        self.buffers[self.active_buffer].editor.cursor_mut().col = 0;
        self.ensure_cursor_visible(viewport_height);
    }

    fn ensure_raw_for_editing(&mut self) {
        if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
            self.buffers[self.active_buffer].view_mode = ViewMode::Raw;
        }
    }

    fn execute_action(&mut self, action: Action, viewport_height: usize, content_width: usize) {
        match action {
            Action::Quit => {
                if self.buffers[self.active_buffer].editor.document().is_modified() {
                    self.command_error =
                        "No write since last change (add ! to override)".to_string();
                } else {
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
                        self.buffers[self.active_buffer].rendered_cursor_row =
                            self.buffers[self.active_buffer].rendered_cursor_row.saturating_sub(1);
                    }
                    self.ensure_rendered_cursor_visible(viewport_height);
                } else {
                    self.buffers[self.active_buffer].editor.pre_move(false);
                    self.buffers[self.active_buffer].editor.cursor_mut().move_up();
                    self.buffers[self.active_buffer].editor.clamp_cursor_col(false);
                    self.ensure_cursor_visible(viewport_height);
                }
            }
            Action::MoveLeft | Action::MoveRight
            | Action::MoveWordForward | Action::MoveWordBackward | Action::MoveWordEnd
            | Action::MoveLineStart | Action::MoveLineEnd => {
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
                                self.buffers[self.active_buffer].rendered_cursor_col =
                                    self.buffers[self.active_buffer].rendered_cursor_col.saturating_sub(1);
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
                self.buffers[self.active_buffer].editor.delete_char_at_cursor();
                self.buffers[self.active_buffer].view_cache_dirty = true;
            }
            Action::DeleteLine => {
                self.ensure_raw_for_editing();
                self.buffers[self.active_buffer].editor.delete_current_line();
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
                self.buffers[self.active_buffer].viewport.scroll_down(1, viewport_height);
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
                self.buffers[self.active_buffer].editor.cursor_mut().jump_top();
                self.buffers[self.active_buffer].viewport.jump_top();
            }
            Action::JumpBottom => {
                self.buffers[self.active_buffer].editor.jump_cursor_bottom();
                self.buffers[self.active_buffer].viewport.jump_bottom(viewport_height);
            }
            Action::NextHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if let Some(y) = self.find_next_heading(None) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
                }
            }
            Action::PrevHeading => {
                if self.buffers[self.active_buffer].view_mode == ViewMode::Rendered {
                    if let Some(y) = self.find_prev_heading(None) {
                        self.buffers[self.active_buffer].viewport.scroll_offset = y;
                    }
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
                    self.active_buffer = (self.active_buffer + 1) % self.buffers.len();
                }
            }
            Action::PrevBuffer => {
                if self.buffers.len() > 1 {
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
                self.outline_saved_scroll =
                    self.buffers[self.active_buffer].viewport.scroll_offset;
                self.mode = AppMode::Outline;
                // Select the heading closest to current scroll position
                let scroll = self.buffers[self.active_buffer].viewport.scroll_offset;
                let entries = self.filtered_outline_entries();
                self.outline_selected = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.y_offset <= scroll)
                    .last()
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
                        format!(
                            "Claude channel: {} ({})",
                            c.socket_path().display(),
                            live
                        )
                    }
                    None => format!(
                        "Not attached. Default socket: {}",
                        ChannelClient::default_socket_path().display()
                    ),
                };
            }
            Action::ClaudeTest => {
                // Inject a synthetic reply to verify the local *claude*-buffer
                // path independent of Claude / sketch-channel.
                self.append_to_claude_buffer(
                    "Hello from :claude-test.\n\nThis is paragraph two.\n\nThis is paragraph three.",
                );
                self.command_error =
                    "Injected synthetic Claude reply into *claude* buffer.".into();
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
                        sketch::acp_channel::DEFAULT_AGENT_COMMAND
                    ),
                };
            }
        }
    }

    /// Update the soft right-margin wrap width. `0` disables the cap (use full terminal width).
    fn set_max_line_width(&mut self, n: usize) {
        self.max_line_width = n;
        for buf in &mut self.buffers {
            buf.viewport.max_line_width = n;
            buf.view_cache_dirty = true;
        }
    }

    /// Rewrite the current line with `level` hashes (0 = remove heading markers).
    fn set_heading_level(&mut self, level: u8) {
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
        buf.editor
            .document_mut()
            .end_undo_group(line_idx, new_col);
        buf.view_cache_dirty = true;
    }

    fn execute_command(&mut self, cmd: &str) {
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

    fn enter_nav_mode(&mut self, mode: NavMode, content_width: usize) {
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

    fn nav_activate(&mut self) {
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
                if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(obj.action_data.as_bytes());
                    }
                }
            }
            NavMode::ListItem => {
                // No-op for now
            }
            NavMode::Character => {}
        }
    }

    fn nav_move_next(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
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

    fn nav_move_prev(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let mode = buf.nav_mode;
        let filtered: Vec<usize> = buf.nav_objects
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
        let prev_pos = if pos == 0 { filtered.len() - 1 } else { pos - 1 };
        let prev_idx = filtered[prev_pos];
        let buf = &mut self.buffers[self.active_buffer];
        buf.nav_object_index = prev_idx;
        let obj = &buf.nav_objects[prev_idx];
        buf.rendered_cursor_row = obj.rendered_row;
        buf.rendered_cursor_col = obj.col_start;
    }

    fn open_link(&mut self, url: &str) {
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
    fn link_under_rendered_cursor(&self) -> Option<String> {
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
                let lines = sketch::view::render_block_to_lines(block, content_width, &self.theme);
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

    fn ensure_rendered_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        let row = buf.rendered_cursor_row;
        self.buffers[self.active_buffer].viewport
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
    fn page_move_cursor(&mut self, n: usize, down: bool) {
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

    fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let buf = &self.buffers[self.active_buffer];
        // Rendered mode tracks its cursor in its OWN coordinate space
        // (`rendered_cursor_row`); the doc cursor doesn't move on j/k/page
        // motions there. Translating doc line → rendered y is lossy and
        // contradicts whatever the rendered handlers just computed, so the
        // outer auto-pin would scroll the viewport back and leave the
        // rendered cursor off-screen.
        let rendered_y = match buf.view_mode {
            ViewMode::Raw => sketch::buffer::raw_cursor_visual_row(
                &buf.editor,
                self.last_wrap_width.max(1),
            ),
            ViewMode::Rendered => buf.rendered_cursor_row,
        };
        self.buffers[self.active_buffer]
            .viewport
            .ensure_cursor_visible(rendered_y, viewport_height);
    }

    fn open_file_browser(&mut self) {
        let dir = std::env::current_dir().unwrap_or_default();
        self.file_browser = Some(FileBrowser::new(dir));
        self.mode = AppMode::FileBrowser;
    }

    fn open_file_browser_full(&mut self, came_from_dropdown: bool) {
        if self.file_browser.is_none() {
            let dir = std::env::current_dir().unwrap_or_default();
            self.file_browser = Some(FileBrowser::new(dir));
        }
        self.screen = AppScreen::FileBrowser { came_from_dropdown };
    }

    fn open_buffer(&mut self, path: std::path::PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        // Check if already open
        for (i, buf) in self.buffers.iter().enumerate() {
            let buf_path = buf.file_path().canonicalize()
                .unwrap_or_else(|_| buf.file_path().to_path_buf());
            if buf_path == canonical {
                self.active_buffer = i;
                return true;
            }
        }
        // Open new
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => {
                    let buffer = Buffer::new(
                        canonical.display().to_string(),
                        content,
                        self.max_line_width,
                        &self.theme,
                    );
                    self.buffers.push(buffer);
                    self.active_buffer = self.buffers.len() - 1;
                    true
                }
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    fn effective_content_width(&self, terminal_width: usize) -> usize {
        self.buffers[self.active_buffer].viewport.content_width(terminal_width)
    }

    fn perform_search(&mut self) {
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
            .position(|&(line, col)| line > cursor_line || (line == cursor_line && col >= cursor_col))
            .unwrap_or(0);
    }

    fn jump_to_match(&mut self, viewport_height: usize) {
        if let Some(&(line_idx, col_idx)) = self.search_matches.get(self.search_match_index) {
            let buf = &mut self.buffers[self.active_buffer];
            buf.editor.cursor_mut().line = line_idx;
            buf.editor.cursor_mut().col = col_idx;
            if buf.view_mode == ViewMode::Rendered {
                // Find rendered position by counting matches (same as view does)
                if let Some((row, col)) = self.find_rendered_match_position(self.search_match_index) {
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
    fn find_rendered_match_position(&self, match_index: usize) -> Option<(usize, usize)> {
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
            let lines = sketch::view::render_block_to_lines(block, content_width, &self.theme);
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
    fn find_next_heading(&self, level_filter: Option<u8>) -> Option<usize> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y > current {
                if let RenderedBlock::Heading { level, .. } = block {
                    if level_filter.is_none() || level_filter == Some(*level) {
                        return Some(y);
                    }
                }
            }
            y += h;
        }
        None
    }

    /// Find the y offset of the previous heading before current scroll position.
    /// If `level_filter` is Some, only match headings at that level.
    fn find_prev_heading(&self, level_filter: Option<u8>) -> Option<usize> {
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
            if let RenderedBlock::Heading { level, .. } = block {
                if level_filter.is_none() || level_filter == Some(*level) {
                    last_match = Some(y);
                }
            }
            y += h;
        }
        last_match
    }

    /// Open `path` as a buffer, creating a new empty buffer if the file doesn't exist yet.
    fn edit_path(&mut self, path_str: &str) {
        let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(rest)
            } else {
                std::path::PathBuf::from(path_str)
            }
        } else {
            std::path::PathBuf::from(path_str)
        };

        let path = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded))
                .unwrap_or(expanded)
        };

        if path.exists() {
            if !self.open_buffer(path.clone()) {
                self.command_error = format!("Error opening: {}", path.display());
            }
            return;
        }

        // Check parent directory exists before creating an unsaved buffer.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                self.command_error = format!("No such directory: {}", parent.display());
                return;
            }
        }

        // Already open with this target path? Switch to it.
        for (i, buf) in self.buffers.iter().enumerate() {
            if buf.file_path() == path {
                self.active_buffer = i;
                return;
            }
        }

        let buffer = Buffer::new(
            path.display().to_string(),
            String::new(),
            self.max_line_width,
            &self.theme,
        );
        self.buffers.push(buffer);
        self.active_buffer = self.buffers.len() - 1;
    }

    fn reload_current_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        if buf.editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        let path = buf.file_path().to_path_buf();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let new_buf = Buffer::new(
                    path.display().to_string(),
                    content,
                    self.max_line_width,
                    &self.theme,
                );
                self.buffers[self.active_buffer] = new_buf;
            }
            Err(e) => {
                self.command_error = format!("Error reading file: {}", e);
            }
        }
    }

    fn close_current_buffer(&mut self) {
        if self.buffers[self.active_buffer].editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(self.active_buffer);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
    }

    fn handle_full_buffer_list_key(&mut self, key: KeyEvent, _viewport_height: usize, _content_width: usize) {
        if self.buffer_list_filter_mode {
            match key.code {
                KeyCode::Esc => {
                    self.close_buffer_list();
                    return;
                }
                KeyCode::Enter => {
                    let filtered = self.filtered_buffer_indices();
                    if filtered.len() == 1 {
                        self.active_buffer = filtered[0];
                        self.close_buffer_list();
                    } else if !filtered.is_empty() {
                        self.buffer_list_filter_mode = false;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    self.buffer_list_filter_text.pop();
                    self.buffer_list_selected = 0;
                }
                KeyCode::Char(c) => {
                    self.buffer_list_filter_text.push(c);
                    self.buffer_list_selected = 0;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_buffer_list();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = (self.buffer_list_selected + 1) % count;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = if self.buffer_list_selected == 0 {
                        count - 1
                    } else {
                        self.buffer_list_selected - 1
                    };
                }
            }
            KeyCode::Char('g') => {
                self.buffer_list_selected = 0;
            }
            KeyCode::Char('G') => {
                let count = self.filtered_buffer_indices().len();
                if count > 0 {
                    self.buffer_list_selected = count - 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.active_buffer = buf_idx;
                    self.close_buffer_list();
                }
            }
            KeyCode::Char('d') => {
                let filtered = self.filtered_buffer_indices();
                if let Some(&buf_idx) = filtered.get(self.buffer_list_selected) {
                    self.close_buffer_at(buf_idx);
                }
            }
            KeyCode::Char('/') => {
                self.buffer_list_filter_mode = true;
                self.buffer_list_filter_text.clear();
                self.buffer_list_selected = 0;
            }
            _ => {}
        }
    }

    fn close_buffer_list(&mut self) {
        self.screen = AppScreen::Editor;
        self.buffer_list_filter_mode = false;
        self.buffer_list_filter_text.clear();
    }

    fn close_buffer_at(&mut self, index: usize) {
        if self.buffers[index].editor.document().is_modified() {
            self.command_error = "No write since last change (add ! to override)".to_string();
            return;
        }
        if self.buffers.len() == 1 {
            self.should_quit = true;
            return;
        }
        self.buffers.remove(index);
        if self.active_buffer >= self.buffers.len() {
            self.active_buffer = self.buffers.len() - 1;
        }
        let count = self.filtered_buffer_indices().len();
        if self.buffer_list_selected >= count && count > 0 {
            self.buffer_list_selected = count - 1;
        }
    }

    fn filtered_buffer_indices(&self) -> Vec<usize> {
        if self.buffer_list_filter_text.is_empty() {
            return (0..self.buffers.len()).collect();
        }
        let query = self.buffer_list_filter_text.to_lowercase();
        (0..self.buffers.len())
            .filter(|&i| {
                let path = self.buffers[i].file_path().display().to_string().to_lowercase();
                fuzzy_match(&path, &query)
            })
            .collect()
    }

    /// Get the heading level at the current scroll offset, if any.
    fn heading_level_at_offset(&self) -> Option<u8> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let current = buf.viewport.scroll_offset;
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if y == current {
                if let RenderedBlock::Heading { level, .. } = block {
                    return Some(*level);
                }
            }
            if y > current {
                break;
            }
            y += h;
        }
        None
    }

    // --- Outline (TOC) ---

    fn handle_outline_key(&mut self, key: KeyEvent, _viewport_height: usize, _content_width: usize) {
        if self.outline_filter_mode {
            match key.code {
                KeyCode::Esc => {
                    // Exit outline entirely, restore scroll
                    self.buffers[self.active_buffer].viewport.scroll_offset =
                        self.outline_saved_scroll;
                    self.mode = AppMode::Normal;
                    return;
                }
                KeyCode::Enter => {
                    let entries = self.filtered_outline_entries();
                    if entries.len() == 1 {
                        // Single result — jump to it
                        self.buffers[self.active_buffer].viewport.scroll_offset =
                            entries[0].y_offset;
                        self.mode = AppMode::Normal;
                    } else if !entries.is_empty() {
                        // Multiple results — exit filter mode, navigate
                        self.outline_filter_mode = false;
                        self.scroll_to_outline_entry();
                    }
                    return;
                }
                KeyCode::Backspace => {
                    self.outline_filter_text.pop();
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                KeyCode::Char(c) => {
                    self.outline_filter_text.push(c);
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                // Restore saved scroll position
                self.buffers[self.active_buffer].viewport.scroll_offset =
                    self.outline_saved_scroll;
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.filtered_outline_entries().len();
                if count > 0 {
                    self.outline_selected = (self.outline_selected + 1) % count;
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = self.filtered_outline_entries().len();
                if count > 0 {
                    self.outline_selected = if self.outline_selected == 0 {
                        count - 1
                    } else {
                        self.outline_selected - 1
                    };
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Enter => {
                let entries = self.filtered_outline_entries();
                if let Some(entry) = entries.get(self.outline_selected) {
                    self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                // Descend: show children of the selected heading
                let entries = self.filtered_outline_entries();
                if let Some(entry) = entries.get(self.outline_selected) {
                    let new_parent = (entry.level, entry.y_offset);
                    self.outline_stack.push(new_parent);
                    // Check if descent has any children. If not, undo it.
                    if self.filtered_outline_entries().is_empty() {
                        self.outline_stack.pop();
                    } else {
                        self.outline_selected = 0;
                        self.outline_filter_text.clear();
                        self.outline_filter_mode = false;
                        self.scroll_to_outline_entry();
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                // Ascend: pop the stack, restoring the previous level.
                if let Some((_old_level, old_y)) = self.outline_stack.pop() {
                    self.outline_filter_text.clear();
                    self.outline_filter_mode = false;
                    let entries = self.filtered_outline_entries();
                    self.outline_selected = entries
                        .iter()
                        .position(|e| e.y_offset == old_y)
                        .unwrap_or(0);
                    self.scroll_to_outline_entry();
                }
            }
            KeyCode::Char('/') => {
                self.outline_filter_mode = true;
                self.outline_filter_text.clear();
                self.outline_selected = 0;
            }
            _ => {}
        }
    }

    /// Scroll the document to the currently selected outline entry.
    fn scroll_to_outline_entry(&mut self) {
        let entries = self.filtered_outline_entries();
        if let Some(entry) = entries.get(self.outline_selected) {
            self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
        }
    }

    /// Get all headings with their rendered y offsets.
    fn outline_entries(&self) -> Vec<OutlineEntry> {
        let buf = &self.buffers[self.active_buffer];
        let content_width = buf.viewport.content_width(200);
        let mut entries = Vec::new();
        let mut y = 0;

        for block in &buf.rendered_cache {
            let h = buf.viewport.block_height(block, content_width);
            if let RenderedBlock::Heading { level, content } = block {
                entries.push(OutlineEntry {
                    level: *level,
                    title: content.text_content(),
                    y_offset: y,
                });
            }
            y += h;
        }
        entries
    }

    /// Get outline entries filtered by current hierarchy level and search text.
    fn filtered_outline_entries(&self) -> Vec<OutlineEntry> {
        let all = self.outline_entries();

        // Apply hierarchy filter via stack
        let scoped: Vec<OutlineEntry> = if let Some(&(parent_level, parent_y)) = self.outline_stack.last() {
            let child_level = parent_level + 1;
            // Show headings at child_level that come after parent_y
            // and before the next heading at parent_level or above
            all.into_iter()
                .skip_while(|e| e.y_offset <= parent_y)
                .take_while(|e| e.level > parent_level)
                .filter(|e| e.level == child_level)
                .collect()
        } else {
            // Show top-level: find the minimum heading level and show only those
            let min_level = all.iter().map(|e| e.level).min().unwrap_or(1);
            all.into_iter().filter(|e| e.level == min_level).collect()
        };

        // Apply text filter
        if self.outline_filter_text.is_empty() {
            scoped
        } else {
            let query = self.outline_filter_text.to_lowercase();
            scoped
                .into_iter()
                .filter(|e| fuzzy_match(&e.title.to_lowercase(), &query))
                .collect()
        }
    }

    /// Build a breadcrumb showing the descent path (e.g. "A › B › C").
    fn outline_breadcrumb(&self) -> Option<String> {
        if self.outline_stack.is_empty() {
            return None;
        }
        let all = self.outline_entries();
        let parts: Vec<String> = self.outline_stack
            .iter()
            .filter_map(|(_, y)| {
                all.iter().find(|e| e.y_offset == *y).map(|e| e.title.clone())
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" \u{203a} "))
        }
    }
}

#[derive(Debug, Clone)]
struct OutlineEntry {
    level: u8,
    title: String,
    y_offset: usize,
}

/// Special filename used for the in-editor *claude* buffer that holds the
/// transcript with horizontal rules between turns. Recognised by send/reply
/// helpers to apply the inline-reply semantics.
const CLAUDE_BUFFER_NAME: &str = "*claude*";

/// Convert a rope char index to (line, col).
fn char_to_line_col(doc: &sketch::document::Document, char_idx: usize) -> (usize, usize) {
    let rope = doc.rope();
    let len = rope.len_chars();
    let i = char_idx.min(len);
    let line = rope.char_to_line(i);
    let line_start = rope.line_to_char(line);
    (line, i - line_start)
}

fn fuzzy_match(text: &str, query: &str) -> bool {
    let mut text_chars = text.chars();
    for qc in query.chars() {
        loop {
            match text_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Merge user menu nodes on top of defaults.
/// User entries with the same key at the same level replace the default entry.
/// New entries are appended.
fn merge_menu(mut defaults: Vec<MenuNode>, user_nodes: &[MenuNode]) -> Vec<MenuNode> {
    for user_node in user_nodes {
        if user_node.key.is_empty() {
            // Separator or label — just append
            defaults.push(user_node.clone());
            continue;
        }
        if let Some(pos) = defaults.iter().position(|d| d.key == user_node.key) {
            defaults[pos] = user_node.clone();
        } else {
            defaults.push(user_node.clone());
        }
    }
    defaults
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketch::config::Config;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    fn fresh_socket() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "sketch-attach-test-{}-{}.sock",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn attach_creates_and_switches_to_claude_buffer() {
        let sock = fresh_socket();
        let listener = UnixListener::bind(&sock).expect("bind");
        // Accept-and-park so the client connect succeeds without immediate EOF.
        let sock_for_thread = sock.clone();
        let _accept = thread::spawn(move || {
            let _ = listener.accept();
            // Hold the connection open until the test ends.
            thread::sleep(Duration::from_secs(2));
            let _ = std::fs::remove_file(&sock_for_thread);
        });
        thread::sleep(Duration::from_millis(50));

        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        assert_eq!(app.buffers.len(), 1, "starts with one buffer");
        assert_eq!(app.active_buffer, 0);

        app.attach_claude_channel(sock.to_str().unwrap());

        assert!(
            app.command_error.starts_with("Attached to Claude channel:"),
            "expected success status, got: {}",
            app.command_error
        );
        assert_eq!(app.buffers.len(), 2, "claude buffer should be created");
        let claude_idx = app
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
            .expect("claude buffer must exist");
        assert_eq!(
            app.active_buffer, claude_idx,
            "active_buffer must switch to claude buffer"
        );
    }

    /// Build an app with enough rendered content to exercise scrolling.
    fn rendered_app() -> App {
        let md = (0..200)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = Config::default();
        let mut app = App::new("/tmp/scroll.md".into(), md, &cfg);
        app.last_viewport_height = 24;
        app.last_wrap_width = 80;
        app.buffers[app.active_buffer].rebuild_render_cache(&app.theme);
        app.buffers[app.active_buffer].update_total_lines(80);
        assert_eq!(app.buffers[app.active_buffer].view_mode, ViewMode::Rendered);
        app
    }

    /// Regression: in Rendered mode, the post-keystroke auto-pin used to
    /// translate the (stale) doc cursor into a rendered y, snapping
    /// `scroll_offset` back to 0 on every j/k and pushing the visible
    /// cursor off the bottom. It must follow `rendered_cursor_row`.
    #[test]
    fn ensure_cursor_visible_in_rendered_mode_follows_rendered_cursor_row() {
        let mut app = rendered_app();
        // Walk the rendered cursor below the initial viewport.
        app.buffers[app.active_buffer].rendered_cursor_row = 100;
        // Doc cursor untouched — exactly the divergence that triggered the bug.
        assert_eq!(app.buffers[app.active_buffer].editor.cursor().line, 0);

        let vh = 24;
        app.ensure_cursor_visible(vh);

        let off = app.buffers[app.active_buffer].viewport.scroll_offset;
        assert!(
            100 >= off && 100 < off + vh,
            "rendered cursor at row 100 must sit inside viewport [{off}, {}); got scroll_offset {off}",
            off + vh
        );
    }

    /// Regression: ctrl-d / ctrl-u in Rendered mode used to move only
    /// `editor.cursor().line` (the raw-mode doc cursor), which isn't
    /// displayed there — the visible cursor stayed put and the action
    /// looked dead. Must move `rendered_cursor_row` instead.
    #[test]
    fn page_move_cursor_in_rendered_mode_moves_rendered_cursor_row() {
        let mut app = rendered_app();
        let pre_doc = app.buffers[app.active_buffer].editor.cursor().line;
        let pre_row = app.buffers[app.active_buffer].rendered_cursor_row;

        app.page_move_cursor(12, true);

        let post_doc = app.buffers[app.active_buffer].editor.cursor().line;
        let post_row = app.buffers[app.active_buffer].rendered_cursor_row;
        assert_eq!(
            post_doc, pre_doc,
            "doc cursor must stay put in Rendered mode (was {pre_doc}, now {post_doc})"
        );
        assert_eq!(
            post_row,
            pre_row + 12,
            "rendered_cursor_row must advance by N on ctrl-d"
        );

        // ctrl-u walks back.
        app.page_move_cursor(12, false);
        assert_eq!(
            app.buffers[app.active_buffer].rendered_cursor_row, pre_row,
            "ctrl-u must reverse the move"
        );
    }

    /// Raw mode keeps moving the doc cursor — page motions there must NOT
    /// regress to touching `rendered_cursor_row`.
    #[test]
    fn page_move_cursor_in_raw_mode_moves_doc_cursor() {
        let mut app = rendered_app();
        app.buffers[app.active_buffer].view_mode = ViewMode::Raw;
        app.buffers[app.active_buffer].update_total_lines(80);
        let pre_row = app.buffers[app.active_buffer].rendered_cursor_row;

        app.page_move_cursor(12, true);

        assert_eq!(
            app.buffers[app.active_buffer].editor.cursor().line,
            12,
            "doc cursor must advance by N in Raw mode"
        );
        assert_eq!(
            app.buffers[app.active_buffer].rendered_cursor_row, pre_row,
            "rendered_cursor_row must stay put in Raw mode"
        );
    }

    /// Build an App with a *claude* buffer that already contains:
    ///   prior locked turn -> "\n\n---\n\n" -> caret position (lockable_through here)
    /// then drops a user draft into the editable region. Returns (app, buf_idx,
    /// draft_start_char). After this the layout looks like:
    ///   "old turn\n\n---\n\n[draft]"
    /// with everything up through the HR locked.
    fn claude_app_with_draft(draft: &str) -> (App, usize, usize) {
        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        let buf_idx = app.or_create_claude_buffer();
        app.active_buffer = buf_idx;

        // Seed prior turn + HR + lock through it.
        let pre = "old turn\n\n---\n\n";
        {
            let editor = &mut app.buffers[buf_idx].editor;
            editor.programmatic_insert(0, pre);
            let eof = editor.document().rope().len_chars();
            let (cl, _) = char_to_line_col(editor.document(), eof);
            editor.set_lockable_through_line(cl);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = 0;
        }
        // Now type a draft.
        let draft_start = {
            let editor = &mut app.buffers[buf_idx].editor;
            let s = editor.document().rope().len_chars();
            editor.programmatic_insert(s, draft);
            s
        };
        // Cursor at end of draft (most realistic mid-typing position).
        {
            let editor = &mut app.buffers[buf_idx].editor;
            let eof = editor.document().rope().len_chars();
            let (cl, cc) = char_to_line_col(editor.document(), eof);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = cc;
        }
        (app, buf_idx, draft_start)
    }

    /// Regression: a Claude reply landing while the user has a draft typed
    /// must splice ABOVE the draft — not append at EOF below it. This is the
    /// "interleaving" behavior promised by the doc-comment on
    /// `append_to_claude_buffer`.
    #[test]
    fn claude_reply_splices_above_pending_draft() {
        let (mut app, buf_idx, _) = claude_app_with_draft("my draft text");
        app.append_to_claude_buffer("REPLY LINE 1\nREPLY LINE 2");

        let text = app.buffers[buf_idx].editor.document().full_text();
        let reply_pos = text.find("REPLY LINE 1").expect("reply must be present");
        let draft_pos = text.find("my draft text").expect("draft must be preserved");
        assert!(
            reply_pos < draft_pos,
            "reply must land ABOVE the draft\n--- buffer ---\n{text}\n--------------"
        );
    }

    /// Regression: after splicing a reply above the draft, the cursor must
    /// stay at the same character offset within the draft — so the user's
    /// in-progress sentence "follows" the text down rather than getting
    /// stranded inside the new frozen reply.
    #[test]
    fn claude_reply_keeps_cursor_on_same_draft_character() {
        let (mut app, buf_idx, _) = claude_app_with_draft("my draft text");

        // Cursor was placed at end of draft; capture the character it sits on
        // (well, the one just before — end-of-text).
        let before_text = app.buffers[buf_idx].editor.document().full_text();
        let before_cursor_char = {
            let e = &app.buffers[buf_idx].editor;
            e.document().line_col_to_char(e.cursor().line, e.cursor().col)
        };
        assert_eq!(&before_text[before_cursor_char.saturating_sub(4)..before_cursor_char], "text");

        app.append_to_claude_buffer("REPLY");

        let after_text = app.buffers[buf_idx].editor.document().full_text();
        let after_cursor_char = {
            let e = &app.buffers[buf_idx].editor;
            e.document().line_col_to_char(e.cursor().line, e.cursor().col)
        };
        assert_eq!(
            &after_text[after_cursor_char.saturating_sub(4)..after_cursor_char],
            "text",
            "cursor must still be sitting just past 'text' in the draft\n--- buffer ---\n{after_text}\n--------------"
        );
    }

    /// Pre-existing behavior must still work: when there's no draft, the
    /// reply just lands at EOF and the cursor follows.
    #[test]
    fn claude_reply_with_no_draft_lands_at_eof() {
        let (mut app, buf_idx, _) = claude_app_with_draft("");
        app.append_to_claude_buffer("REPLY");
        let text = app.buffers[buf_idx].editor.document().full_text();
        assert!(text.contains("REPLY"));
        assert!(!text.contains("my draft"));
    }
}
