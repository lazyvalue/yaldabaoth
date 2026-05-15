use sketch::keys::{Key, KeyPress, Modifiers};

use crate::app::{App, AppMode};

impl App {
    /// Handle a keypress when the compose textbox is active and in Normal mode.
    pub(crate) fn handle_compose_normal_key(&mut self, key: KeyPress, _viewport_height: usize) {
        if self.compose_textbox.is_none() {
            return;
        }

        // Ctrl-T toggles compose off (closes textbox).
        if key.modifiers.contains(Modifiers::CONTROL) {
            match key.key {
                Key::Char('t') => {
                    self.compose_toggle();
                    return;
                }
                Key::Enter => {
                    self.compose_send();
                    return;
                }
                Key::Up => {
                    self.buffers[self.active_buffer].viewport.scroll_offset =
                        self.buffers[self.active_buffer]
                            .viewport
                            .scroll_offset
                            .saturating_sub(1);
                    return;
                }
                Key::Down => {
                    self.buffers[self.active_buffer].viewport.scroll_offset += 1;
                    return;
                }
                Key::Char('r') => {
                    let tb = self.compose_textbox.as_mut().unwrap();
                    tb.editor.redo();
                    return;
                }
                _ => {}
            }
        }

        // Re-borrow after potential early returns.
        let tb = self.compose_textbox.as_mut().unwrap();

        match key.key {
            Key::Esc => {
                // Already in normal mode within compose — no-op.
            }
            Key::Char('i') if key.modifiers.is_empty() => {
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            Key::Char('a') if key.modifiers.is_empty() => {
                tb.editor.move_right_clamped(true);
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            Key::Char('o') if key.modifiers.is_empty() => {
                tb.editor.open_line_below();
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            Key::Char('O') if key.modifiers.is_empty() => {
                tb.editor.open_line_above();
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            Key::Char('A') if key.modifiers.is_empty() => {
                let line = tb.editor.cursor().line;
                let line_len = tb.editor.document().line_len_chars(line);
                // Subtract 1 for the trailing newline if present
                let text_len = if line_len > 0
                    && tb.editor.document().line_text(line).ends_with('\n')
                {
                    line_len - 1
                } else {
                    line_len
                };
                tb.editor.cursor_mut().col = text_len;
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            Key::Char('I') if key.modifiers.is_empty() => {
                tb.editor.cursor_mut().col = 0;
                tb.editor.begin_insert();
                tb.mode = AppMode::Insert;
            }
            // Movement
            Key::Char('h') | Key::Left if key.modifiers.is_empty() => {
                tb.editor.cursor_mut().move_left();
            }
            Key::Char('l') | Key::Right if key.modifiers.is_empty() => {
                tb.editor.move_right_clamped(false);
            }
            Key::Char('j') | Key::Down if key.modifiers.is_empty() => {
                tb.editor.move_down(false);
            }
            Key::Char('k') | Key::Up if key.modifiers.is_empty() => {
                tb.editor.cursor_mut().move_up();
                tb.editor.clamp_cursor_col(false);
            }
            Key::Char('w') if key.modifiers.is_empty() => {
                tb.editor.move_cursor_word_forward();
            }
            Key::Char('b') if key.modifiers.is_empty() => {
                tb.editor.move_cursor_word_backward();
            }
            Key::Char('e') if key.modifiers.is_empty() => {
                tb.editor.move_cursor_word_end();
            }
            Key::Char('0') if key.modifiers.is_empty() => {
                tb.editor.cursor_mut().col = 0;
            }
            Key::Char('$') if key.modifiers.is_empty() => {
                let line = tb.editor.cursor().line;
                let len = tb.editor.document().line_len_chars(line);
                let text_len = if len > 0
                    && tb.editor.document().line_text(line).ends_with('\n')
                {
                    len - 1
                } else {
                    len
                };
                tb.editor.cursor_mut().col = text_len.saturating_sub(1);
            }
            Key::Char('u') if key.modifiers.is_empty() => {
                tb.editor.undo();
            }
            Key::Char('x') if key.modifiers.is_empty() => {
                tb.editor.delete_char_at_cursor();
            }
            Key::Char(':') if key.modifiers.is_empty() => {
                // Allow entering command mode from compose normal.
                self.mode = AppMode::Command;
            }
            _ => {}
        }
    }

    /// Handle a keypress when the compose textbox is active and in Insert mode.
    pub(crate) fn handle_compose_insert_key(&mut self, key: KeyPress, _viewport_height: usize) {
        if self.compose_textbox.is_none() {
            return;
        }

        // Ctrl-Enter sends from insert mode too.
        if key.modifiers.contains(Modifiers::CONTROL) {
            match key.key {
                Key::Enter => {
                    self.compose_send();
                    return;
                }
                Key::Char('t') => {
                    self.compose_toggle();
                    return;
                }
                _ => {}
            }
        }

        let tb = self.compose_textbox.as_mut().unwrap();

        match key.key {
            Key::Esc => {
                tb.editor.end_insert();
                tb.mode = AppMode::Normal;
                if tb.editor.cursor().col > 0 {
                    tb.editor.cursor_mut().move_left();
                }
            }
            Key::Enter => {
                tb.editor.insert_char('\n');
            }
            Key::Tab => {
                tb.editor.insert_char(' ');
                tb.editor.insert_char(' ');
            }
            Key::Backspace => {
                tb.editor.backspace();
            }
            Key::Char(c) => {
                if !key.modifiers.contains(Modifiers::CONTROL) {
                    tb.editor.insert_char(c);
                }
            }
            Key::Left => {
                tb.editor.cursor_mut().move_left();
            }
            Key::Right => {
                tb.editor.move_right_clamped(true);
            }
            Key::Up => {
                tb.editor.cursor_mut().move_up();
                tb.editor.clamp_cursor_col(true);
            }
            Key::Down => {
                tb.editor.move_down(true);
            }
            _ => {}
        }
    }
}
