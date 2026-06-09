use sketch::keys::{Key, KeyPress, Modifiers};
use sketch::view::ViewMode;

use crate::app::{App, AppMode};

impl App {
    pub(crate) fn handle_insert_key(&mut self, key: KeyPress, viewport_height: usize) {
        match key.key {
            Key::Esc => {
                self.leave_insert_mode();
            }
            Key::Enter => {
                let indent = self.current_line_indent();
                self.buffers[self.active_buffer].editor.insert_char('\n');
                for _ in 0..indent.len() {
                    self.buffers[self.active_buffer].editor.insert_char(' ');
                }
            }
            Key::Tab => {
                for _ in 0..2 {
                    self.buffers[self.active_buffer].editor.insert_char(' ');
                }
            }
            Key::BackTab => {
                self.dedent_at_cursor();
            }
            Key::Backspace => {
                self.backspace_smart();
            }
            Key::Char(c) => {
                if key.modifiers.contains(Modifiers::ALT) {
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
                } else if key.modifiers.contains(Modifiers::CONTROL) && c == 'v' {
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
            Key::Left => {
                self.buffers[self.active_buffer]
                    .editor
                    .cursor_mut()
                    .move_left();
            }
            Key::Right => {
                self.buffers[self.active_buffer]
                    .editor
                    .move_right_clamped(true);
            }
            Key::Up => {
                self.buffers[self.active_buffer]
                    .editor
                    .cursor_mut()
                    .move_up();
                self.buffers[self.active_buffer]
                    .editor
                    .clamp_cursor_col(true);
            }
            Key::Down => {
                self.buffers[self.active_buffer].editor.move_down(true);
            }
            _ => {}
        }
        self.ensure_cursor_visible(viewport_height);
    }

    pub(crate) fn leave_insert_mode(&mut self) {
        self.buffers[self.active_buffer].editor.end_insert();
        self.mode = AppMode::Normal;
        self.buffers[self.active_buffer].view_cache_dirty = true;
        if self.buffers[self.active_buffer].editor.cursor().col > 0 {
            self.buffers[self.active_buffer]
                .editor
                .cursor_mut()
                .move_left();
        }
    }

    pub(crate) fn backspace_smart(&mut self) {
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
            let remove = if col.is_multiple_of(2) { 2 } else { 1 };
            let remove = remove.min(col);
            for _ in 0..remove {
                self.buffers[self.active_buffer].editor.backspace();
            }
        } else {
            self.buffers[self.active_buffer].editor.backspace();
        }
    }

    pub(crate) fn dedent_at_cursor(&mut self) {
        let editor = &self.buffers[self.active_buffer].editor;
        let line = editor.document().line_text(editor.cursor().line);
        let indent_len = line.chars().take_while(|c| *c == ' ').count();
        let remove = if indent_len % 2 == 0 {
            2.min(indent_len)
        } else {
            1
        };
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
}
