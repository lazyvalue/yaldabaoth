use sketch::buffer::NavMode;
use sketch::keys::{Key, KeyPress, Modifiers};
use sketch::view::ViewMode;

use crate::app::App;

impl App {
    pub(crate) fn handle_normal_key(&mut self, key: KeyPress, viewport_height: usize, content_width: usize) {
        if self.search_input_mode {
            match key.key {
                Key::Enter => {
                    self.search_query = self.search_input_buffer.clone();
                    self.search_input_mode = false;
                    self.perform_search();
                    self.jump_to_match(viewport_height);
                }
                Key::Esc => {
                    self.search_input_mode = false;
                    self.search_input_buffer.clear();
                }
                Key::Backspace => {
                    self.search_input_buffer.pop();
                }
                Key::Char(c) => {
                    self.search_input_buffer.push(c);
                }
                _ => {}
            }
            return;
        }

        // Pending f/F/t/T — consume the next character as the target.
        if let Some(pending) = self.pending_find_char.take() {
            if let Key::Char(c) = key.key {
                self.execute_find_char(pending, c, viewport_height);
                return;
            }
            // Any non-Char key cancels the pending find and falls through.
        }

        if key.key == Key::Esc {
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
        if let Key::Char(c) = key.key {
            if key.modifiers.is_empty() || key.modifiers == Modifiers::SHIFT {
                if c.is_ascii_digit() && (self.pending_count.is_some() || c != '0') {
                    let digit = c as usize - '0' as usize;
                    self.pending_count = Some(self.pending_count.unwrap_or(0) * 10 + digit);
                    return;
                }
            }
        }

        // G with a count = go to line N
        if let Key::Char('G') = key.key {
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
}
