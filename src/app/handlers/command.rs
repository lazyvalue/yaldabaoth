use sketch::keys::{Key, KeyPress};

use crate::app::{App, AppMode};

impl App {
    pub(crate) fn handle_command_key(&mut self, key: KeyPress) {
        match key.key {
            Key::Esc => {
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
            }
            Key::Enter => {
                let cmd = self.command_buffer.clone();
                self.command_buffer.clear();
                self.mode = AppMode::Normal;
                self.execute_command(&cmd);
            }
            Key::Backspace => {
                self.command_buffer.pop();
                if self.command_buffer.is_empty() {
                    self.mode = AppMode::Normal;
                }
            }
            Key::Char(c) => {
                self.command_buffer.push(c);
            }
            _ => {}
        }
    }
}
