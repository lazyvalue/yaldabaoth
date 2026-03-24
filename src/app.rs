use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use ratatui::DefaultTerminal;

use sketch::blocks::RenderedBlock;
use sketch::config::Config;
use sketch::keybind::{Action, KeybindManager};
use sketch::render;
use sketch::theme::Theme;
use sketch::view::{self, ViewState};
use sketch::viewport::Viewport;

pub struct App {
    filename: String,
    blocks: Vec<RenderedBlock>,
    viewport: Viewport,
    theme: Theme,
    keybinds: KeybindManager,
    should_quit: bool,
}

impl App {
    pub fn new(filename: String, markdown: String, config: &Config) -> Self {
        let theme = Theme::dark();
        let blocks = render::render(&markdown, &theme);
        let viewport = Viewport::new(config.max_line_width);
        let keybinds = KeybindManager::default();

        Self {
            filename,
            blocks,
            viewport,
            theme,
            keybinds,
            should_quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // Calculate initial dimensions
        let size = terminal.size()?;
        let content_width = self.viewport.content_width(size.width as usize);
        self.viewport.calculate_total_lines(&self.blocks, content_width);

        loop {
            terminal.draw(|frame| {
                let state = ViewState {
                    filename: &self.filename,
                    blocks: &self.blocks,
                    viewport: &self.viewport,
                    theme: &self.theme,
                    mode_label: "NORMAL",
                };
                view::draw(frame, &state);
            })?;

            if self.should_quit {
                break;
            }

            // Poll for events with a timeout (for multi-key sequence timeout)
            let timeout = if self.keybinds.has_pending() {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key_event) => self.handle_key(key_event, terminal)?,
                    Event::Resize(w, _h) => {
                        let cw = self.viewport.content_width(w as usize);
                        self.viewport.calculate_total_lines(&self.blocks, cw);
                    }
                    _ => {}
                }
            } else if self.keybinds.has_pending() {
                // Timeout with pending keys — reset
                self.keybinds.reset_pending();
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent, terminal: &DefaultTerminal) -> io::Result<()> {
        let size = terminal.size()?;
        let viewport_height = (size.height as usize).saturating_sub(2);
        let content_width = self.viewport.content_width(size.width as usize);

        if let Some(action) = self.keybinds.process_key(key) {
            match action {
                Action::Quit => self.should_quit = true,
                Action::ScrollDown => self.viewport.scroll_down(1, viewport_height),
                Action::ScrollUp => self.viewport.scroll_up(1),
                Action::HalfPageDown => self.viewport.scroll_down(viewport_height / 2, viewport_height),
                Action::HalfPageUp => self.viewport.scroll_up(viewport_height / 2),
                Action::FullPageDown => self.viewport.scroll_down(viewport_height, viewport_height),
                Action::FullPageUp => self.viewport.scroll_up(viewport_height),
                Action::JumpTop => self.viewport.jump_top(),
                Action::JumpBottom => self.viewport.jump_bottom(viewport_height),
                Action::NextHeading => {
                    self.jump_to_next_heading(content_width, viewport_height);
                }
                Action::PrevHeading => {
                    self.jump_to_prev_heading(content_width, viewport_height);
                }
                Action::NextHeadingSameLevel => {
                    self.jump_to_heading_same_level(content_width, viewport_height, true);
                }
                Action::PrevHeadingSameLevel => {
                    self.jump_to_heading_same_level(content_width, viewport_height, false);
                }
                Action::SearchForward | Action::SearchBackward
                | Action::SearchNext | Action::SearchPrev => {
                    // Search — implement in a later task
                }
                Action::OpenLink | Action::YankLine => {
                    // Implement in a later task
                }
                Action::None => {}
            }
        }

        Ok(())
    }

    fn jump_to_next_heading(&mut self, width: usize, viewport_height: usize) {
        let mut y = 0;

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if y > self.viewport.scroll_offset && matches!(block, RenderedBlock::Heading { .. }) {
                self.viewport.scroll_offset = y.saturating_sub(viewport_height / 3);
                return;
            }
            y += h;
        }
    }

    fn jump_to_prev_heading(&mut self, width: usize, viewport_height: usize) {
        let mut positions = Vec::new();
        let mut y = 0;

        for block in &self.blocks {
            if matches!(block, RenderedBlock::Heading { .. }) {
                positions.push(y);
            }
            y += self.viewport.block_height(block, width);
        }

        // Find the last heading position before current scroll
        if let Some(&pos) = positions.iter().rev().find(|&&p| p < self.viewport.scroll_offset) {
            self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
        }
    }

    fn jump_to_heading_same_level(&mut self, width: usize, viewport_height: usize, forward: bool) {
        // Find the current heading level (most recently passed heading)
        let mut current_level = None;
        let mut y = 0;
        let mut headings: Vec<(usize, u8)> = Vec::new(); // (y_offset, level)

        for block in &self.blocks {
            let h = self.viewport.block_height(block, width);
            if let RenderedBlock::Heading { level, .. } = block {
                if y <= self.viewport.scroll_offset {
                    current_level = Some(*level);
                }
                headings.push((y, *level));
            }
            y += h;
        }

        let target_level = match current_level {
            Some(l) => l,
            None => return,
        };

        if forward {
            if let Some(&(pos, _)) = headings.iter().find(|(y, l)| *y > self.viewport.scroll_offset && *l == target_level) {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        } else {
            if let Some(&(pos, _)) = headings.iter().rev().find(|(y, l)| *y < self.viewport.scroll_offset && *l == target_level) {
                self.viewport.scroll_offset = pos.saturating_sub(viewport_height / 3);
            }
        }
    }
}
