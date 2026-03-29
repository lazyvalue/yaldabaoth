use crate::blocks::RenderedBlock;
use crate::editor::Editor;
use crate::highlight::Highlighter;
use crate::render;
use crate::theme::Theme;
use crate::view::ViewMode;
use crate::viewport::Viewport;

pub struct Buffer {
    pub editor: Editor,
    pub viewport: Viewport,
    pub view_mode: ViewMode,
    pub highlighter: Highlighter,
    pub rendered_cache: Vec<RenderedBlock>,
    pub view_cache_dirty: bool,
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
}
