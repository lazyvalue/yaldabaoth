use crate::blocks::RenderedBlock;
use crate::editor::Editor;
use crate::highlight::Highlighter;
use crate::render;
use crate::theme::Theme;
use crate::view;
use crate::view::ViewMode;
use crate::viewport::Viewport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    Character,
    Link,
    Heading,
    ListItem,
    CodeBlock,
}

impl NavMode {
    pub fn next(self) -> Self {
        match self {
            NavMode::Character => NavMode::Link,
            NavMode::Link => NavMode::Heading,
            NavMode::Heading => NavMode::ListItem,
            NavMode::ListItem => NavMode::CodeBlock,
            NavMode::CodeBlock => NavMode::Character,
        }
    }

    pub fn label(&self) -> Option<&'static str> {
        match self {
            NavMode::Character => None,
            NavMode::Link => Some("LINKS"),
            NavMode::Heading => Some("HEADINGS"),
            NavMode::ListItem => Some("LIST ITEMS"),
            NavMode::CodeBlock => Some("CODE BLOCKS"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NavObject {
    pub rendered_row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub kind: NavMode,
    pub action_data: String,
}

pub struct Buffer {
    pub editor: Editor,
    pub viewport: Viewport,
    pub view_mode: ViewMode,
    pub highlighter: Highlighter,
    pub rendered_cache: Vec<RenderedBlock>,
    pub view_cache_dirty: bool,
    /// Cursor position in rendered view coordinates (row, col in rendered lines).
    pub rendered_cursor_row: usize,
    pub rendered_cursor_col: usize,
    pub nav_mode: NavMode,
    pub nav_objects: Vec<NavObject>,
    pub nav_object_index: usize,
}

impl Buffer {
    pub fn new(filename: String, content: String, max_line_width: usize, theme: &Theme) -> Self {
        let editor = Editor::new(content, std::path::PathBuf::from(&filename));
        let viewport = Viewport::new(max_line_width);
        let syntect_theme = theme.name.syntect_theme();
        Self {
            editor,
            viewport,
            view_mode: ViewMode::Rendered,
            highlighter: Highlighter::with_syntect_theme(syntect_theme),
            rendered_cache: Vec::new(),
            view_cache_dirty: true,
            rendered_cursor_row: 0,
            rendered_cursor_col: 0,
            nav_mode: NavMode::Character,
            nav_objects: Vec::new(),
            nav_object_index: 0,
        }
    }

    pub fn rebuild_render_cache(&mut self, theme: &Theme) {
        let text = self.editor.document().full_text();
        self.rendered_cache = render::render_with_highlighter(&text, theme, &self.highlighter);
    }

    pub fn update_total_lines(&mut self, content_width: usize) {
        match self.view_mode {
            ViewMode::Rendered => {
                self.viewport.total_lines = self
                    .rendered_cache
                    .iter()
                    .map(|b| self.viewport.block_height(b, content_width))
                    .sum();
            }
            ViewMode::Raw => {
                self.viewport.total_lines = self.editor.document().line_count();
            }
        }
    }

    pub fn file_path(&self) -> &std::path::Path {
        &self.editor.document().file_path
    }

    pub fn rebuild_nav_objects(&mut self, theme: &Theme) {
        self.nav_objects.clear();
        let content_width = self.viewport.content_width(200);
        let mut rendered_row = 0;

        for block in &self.rendered_cache {
            let lines = view::render_block_to_lines(block, content_width, theme);

            match block {
                RenderedBlock::Heading { .. } => {
                    if let Some(line) = lines.first() {
                        let text = line.text_content();
                        let char_len = text.chars().count();
                        if char_len > 0 {
                            self.nav_objects.push(NavObject {
                                rendered_row,
                                col_start: 0,
                                col_end: char_len,
                                kind: NavMode::Heading,
                                action_data: String::new(),
                            });
                        }
                    }
                }
                RenderedBlock::CodeBlock { lines: code_lines, .. } => {
                    let code_text: String = code_lines
                        .iter()
                        .map(|l| l.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Some(line) = lines.first() {
                        let text = line.text_content();
                        let char_len = text.chars().count();
                        self.nav_objects.push(NavObject {
                            rendered_row,
                            col_start: 0,
                            col_end: char_len.max(1),
                            kind: NavMode::CodeBlock,
                            action_data: code_text,
                        });
                    }
                }
                RenderedBlock::List { items, .. } => {
                    // Each list item gets a NavObject on the line where it starts
                    let mut item_line = 0;
                    for item in items {
                        let marker_text = if let Some(checked) = item.checked {
                            if checked {
                                format!("{} [x] ", item.marker)
                            } else {
                                format!("{} [ ] ", item.marker)
                            }
                        } else {
                            format!("{} ", item.marker)
                        };
                        // Each item's first line in the rendered output
                        if item_line < lines.len() {
                            let text = lines[item_line].text_content();
                            let char_len = text.chars().count();
                            if char_len > 0 {
                                self.nav_objects.push(NavObject {
                                    rendered_row: rendered_row + item_line,
                                    col_start: 0,
                                    col_end: char_len,
                                    kind: NavMode::ListItem,
                                    action_data: String::new(),
                                });
                            }
                        }
                        // Advance past this item's rendered lines
                        for content_block in &item.content {
                            item_line += self.viewport.block_height(content_block, content_width.saturating_sub(marker_text.len()));
                        }
                    }
                }
                _ => {}
            }

            // Scan all lines for links
            for (line_idx, line) in lines.iter().enumerate() {
                let mut col = 0;
                for span in &line.spans {
                    let span_chars = span.text.chars().count();
                    if let Some(ref url) = span.link {
                        if span_chars > 0 {
                            self.nav_objects.push(NavObject {
                                rendered_row: rendered_row + line_idx,
                                col_start: col,
                                col_end: col + span_chars,
                                kind: NavMode::Link,
                                action_data: url.clone(),
                            });
                        }
                    }
                    col += span_chars;
                }
            }

            rendered_row += lines.len();
        }

        self.nav_objects.sort_by(|a, b| {
            a.rendered_row.cmp(&b.rendered_row)
                .then(a.col_start.cmp(&b.col_start))
        });
    }

    pub fn objects_for_current_mode(&self) -> Vec<(usize, &NavObject)> {
        self.nav_objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.kind == self.nav_mode)
            .collect()
    }

    pub fn nearest_object_index(&self, rendered_row: usize) -> Option<usize> {
        let filtered: Vec<(usize, &NavObject)> = self.objects_for_current_mode();
        if filtered.is_empty() {
            return None;
        }
        let (best_filtered_idx, _) = filtered
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, o))| {
                (o.rendered_row as isize - rendered_row as isize).unsigned_abs()
            })?;
        Some(filtered[best_filtered_idx].0)
    }
}
