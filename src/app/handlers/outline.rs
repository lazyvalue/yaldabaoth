use sketch::blocks::RenderedBlock;
use sketch::keys::{Key, KeyPress};

use crate::app::{App, AppMode, fuzzy_match};

#[derive(Debug, Clone)]
pub(crate) struct OutlineEntry {
    pub(crate) level: u8,
    pub(crate) title: String,
    pub(crate) y_offset: usize,
}

impl App {
    pub(crate) fn handle_outline_key(&mut self, key: KeyPress, _viewport_height: usize, _content_width: usize) {
        if self.outline_filter_mode {
            match key.key {
                Key::Esc => {
                    // Exit outline entirely, restore scroll
                    self.buffers[self.active_buffer].viewport.scroll_offset =
                        self.outline_saved_scroll;
                    self.mode = AppMode::Normal;
                    return;
                }
                Key::Enter => {
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
                Key::Backspace => {
                    self.outline_filter_text.pop();
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                Key::Char(c) => {
                    self.outline_filter_text.push(c);
                    self.outline_selected = 0;
                    self.scroll_to_outline_entry();
                }
                _ => {}
            }
            return;
        }

        match key.key {
            Key::Esc | Key::Char('q') => {
                // Restore saved scroll position
                self.buffers[self.active_buffer].viewport.scroll_offset =
                    self.outline_saved_scroll;
                self.mode = AppMode::Normal;
            }
            Key::Char('j') | Key::Down => {
                let count = self.filtered_outline_entries().len();
                if count > 0 {
                    self.outline_selected = (self.outline_selected + 1) % count;
                    self.scroll_to_outline_entry();
                }
            }
            Key::Char('k') | Key::Up => {
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
            Key::Enter => {
                let entries = self.filtered_outline_entries();
                if let Some(entry) = entries.get(self.outline_selected) {
                    self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
                    self.mode = AppMode::Normal;
                }
            }
            Key::Char('l') | Key::Right => {
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
            Key::Char('h') | Key::Left => {
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
            Key::Char('/') => {
                self.outline_filter_mode = true;
                self.outline_filter_text.clear();
                self.outline_selected = 0;
            }
            _ => {}
        }
    }

    /// Scroll the document to the currently selected outline entry.
    pub(crate) fn scroll_to_outline_entry(&mut self) {
        let entries = self.filtered_outline_entries();
        if let Some(entry) = entries.get(self.outline_selected) {
            self.buffers[self.active_buffer].viewport.scroll_offset = entry.y_offset;
        }
    }

    /// Get all headings with their rendered y offsets.
    pub(crate) fn outline_entries(&self) -> Vec<OutlineEntry> {
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
    pub(crate) fn filtered_outline_entries(&self) -> Vec<OutlineEntry> {
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
    pub(crate) fn outline_breadcrumb(&self) -> Option<String> {
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
