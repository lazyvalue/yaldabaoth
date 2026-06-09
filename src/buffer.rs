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
                self.viewport.total_lines =
                    raw_visual_row_count(&self.editor, content_width.max(1));
            }
        }
    }

    pub fn file_path(&self) -> &std::path::Path {
        &self.editor.document().file_path
    }

    pub fn rebuild_nav_objects(&mut self, theme: &Theme, content_width: usize) {
        self.nav_objects.clear();
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
                RenderedBlock::CodeBlock {
                    lines: code_lines, ..
                } => {
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
                            item_line += self.viewport.block_height(
                                content_block,
                                content_width.saturating_sub(marker_text.len()),
                            );
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
                    if let Some(ref url) = span.link
                        && span_chars > 0
                    {
                        self.nav_objects.push(NavObject {
                            rendered_row: rendered_row + line_idx,
                            col_start: col,
                            col_end: col + span_chars,
                            kind: NavMode::Link,
                            action_data: url.clone(),
                        });
                    }
                    col += span_chars;
                }
            }

            // Use block_height (not lines.len()) to match the view's row accumulation
            rendered_row += self.viewport.block_height(block, content_width);
        }

        self.nav_objects.sort_by(|a, b| {
            a.rendered_row
                .cmp(&b.rendered_row)
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
        let (best_filtered_idx, _) = filtered.iter().enumerate().min_by_key(|(_, (_, o))| {
            (o.rendered_row as isize - rendered_row as isize).unsigned_abs()
        })?;
        Some(filtered[best_filtered_idx].0)
    }
}

/// True iff the `*claude*`-style line owned-by-the-user heuristic considers
/// this line a "user reply line": at least one non-whitespace char, none of
/// the non-whitespace chars are inside a frozen range, and it isn't the turn
/// delimiter `---`. Mirrors view::is_user_line so total-row counting and
/// rendering agree on which lines wrap at the prefixed (narrower) width.
fn classify_user_line(line_first_char: usize, line_text: &str, frozen: &[(usize, usize)]) -> bool {
    if line_text.trim_end_matches('\n').trim() == "---" {
        return false;
    }
    let mut idx = line_first_char;
    let mut had_non_ws = false;
    for ch in line_text.chars() {
        if !ch.is_whitespace() {
            had_non_ws = true;
            if frozen.iter().any(|&(s, e)| idx >= s && idx < e) {
                return false;
            }
        }
        idx += 1;
    }
    had_non_ws
}

const REPLY_PREFIX_LEN: usize = 2;

fn line_wrap_width(is_user: bool, base: usize) -> usize {
    if is_user {
        base.saturating_sub(REPLY_PREFIX_LEN).max(1)
    } else {
        base
    }
}

// (line_visual_rows replaced by view::wrap_row_count; the renderer's
// word-boundary wrap is the only source of truth.)

/// Total visual rows when rendering `editor`'s document in raw mode at the
/// given `wrap_width`.
/// Build the same line text the renderer sees: trailing newline stripped,
/// tabs expanded to four spaces. The wrap width counts MUST be computed
/// against this exact string or scroll math will desync from the renderer
/// (each tab is 1 doc char but 4 visual cols, so a line with several tabs
/// can wrap to many more rows than `line_len_chars / wrap_width` predicts).
fn render_line_text(doc: &crate::document::Document, line: usize) -> String {
    let mut s = doc.line_text(line);
    if s.ends_with('\n') {
        s.pop();
    }
    s.replace('\t', "    ")
}

/// Adjust `cursor_col` (a doc-char column) to the equivalent column in the
/// tab-expanded render text. Each tab counts as 4 cells, so a doc-col that
/// sits past N tabs maps to render-col cursor_col + 3*N.
fn render_col_for_cursor(doc: &crate::document::Document, line: usize, cursor_col: usize) -> usize {
    let line_text = doc.line_text(line);
    let mut render_col = 0usize;
    for (i, ch) in line_text.chars().enumerate() {
        if i >= cursor_col {
            break;
        }
        if ch == '\t' {
            render_col += 4;
        } else {
            render_col += 1;
        }
    }
    render_col
}

pub fn raw_visual_row_count(editor: &Editor, wrap_width: usize) -> usize {
    let doc = editor.document();
    let frozen = editor.frozen_ranges();
    let mut total = 0usize;
    let mut char_idx = 0usize;
    for l in 0..doc.line_count() {
        let line_text = doc.line_text(l);
        let is_user = classify_user_line(char_idx, &line_text, &frozen);
        let lw = line_wrap_width(is_user, wrap_width);
        let render_text = render_line_text(doc, l);
        // Use the SAME word-boundary wrap the renderer uses.
        total += crate::view::wrap_row_count(&render_text, lw);
        char_idx += line_text.chars().count();
    }
    total
}

/// Compute the cursor's visual-row index in raw mode. Uses the same word-
/// boundary wrapping the renderer applies, against the same tab-expanded
/// text — otherwise the renderer paints the cursor at row R but scroll
/// math thinks it's at row R+N (for some N depending on lines above), and
/// the cursor ends up off-screen.
pub fn raw_cursor_visual_row(editor: &Editor, wrap_width: usize) -> usize {
    let doc = editor.document();
    let frozen = editor.frozen_ranges();
    let cursor = editor.cursor();
    let target_line = cursor.line;
    let target_col = cursor.col;
    let mut total = 0usize;
    let mut char_idx = 0usize;
    for l in 0..doc.line_count() {
        let line_text = doc.line_text(l);
        let is_user = classify_user_line(char_idx, &line_text, &frozen);
        let lw = line_wrap_width(is_user, wrap_width);
        let render_text = render_line_text(doc, l);
        if l == target_line {
            let render_cursor_col = render_col_for_cursor(doc, l, target_col);
            let (_, cursor_row) =
                crate::view::wrap_row_count_with_cursor(&render_text, lw, render_cursor_col);
            return total + cursor_row;
        }
        total += crate::view::wrap_row_count(&render_text, lw);
        char_idx += line_text.chars().count();
    }
    total
}
