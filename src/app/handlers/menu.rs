use sketch::keys::{Key, KeyPress};

use crate::app::{App, AppMode};

impl App {
    pub(crate) fn handle_menu_key(&mut self, key: KeyPress, viewport_height: usize, content_width: usize) {
        match key.key {
            Key::Esc => {
                self.menu_state.handle_escape();
                if !self.menu_state.is_active() {
                    self.mode = AppMode::Normal;
                }
            }
            _ => {
                if let Some(cmd_string) = self.menu_state.process_key(key, &self.menu_tree) {
                    self.mode = AppMode::Normal;
                    self.dispatch_command(&cmd_string, viewport_height, content_width);
                }
            }
        }
    }
}
