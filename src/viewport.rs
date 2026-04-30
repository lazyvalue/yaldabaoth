use crate::blocks::*;

pub const SCROLLOFF: usize = 3;

pub struct Viewport {
    pub scroll_offset: usize,
    pub cursor_line: usize,
    pub total_lines: usize,
    pub max_line_width: usize,
}

pub struct PositionedBlock<'a> {
    pub block: &'a RenderedBlock,
    pub y_offset: usize,
    pub height: usize,
}

impl Viewport {
    pub fn new(max_line_width: usize) -> Self {
        Self {
            scroll_offset: 0,
            cursor_line: 0,
            total_lines: 0,
            max_line_width,
        }
    }

    pub fn content_width(&self, terminal_width: usize) -> usize {
        if self.max_line_width > 0 {
            self.max_line_width.min(terminal_width)
        } else {
            terminal_width
        }
    }

    pub fn content_offset(&self, terminal_width: usize) -> usize {
        const LEFT_PAD: usize = 4;
        if terminal_width > LEFT_PAD {
            LEFT_PAD
        } else {
            0
        }
    }

    pub fn block_height(&self, block: &RenderedBlock, width: usize) -> usize {
        match block {
            RenderedBlock::Heading { level, .. } => {
                if *level == 1 {
                    3
                } else {
                    2
                }
            }
            RenderedBlock::Paragraph { lines } => {
                let text_lines: usize = lines
                    .iter()
                    .map(|l| self.wrapped_line_count(l, width))
                    .sum();
                text_lines + 1
            }
            RenderedBlock::CodeBlock { lines, .. } => lines.len() + 2,
            RenderedBlock::BlockQuote { blocks } => {
                let inner: usize = blocks
                    .iter()
                    .map(|b| self.block_height(b, width.saturating_sub(4)))
                    .sum();
                inner + 1
            }
            RenderedBlock::List { items, .. } => {
                let item_lines: usize = items
                    .iter()
                    .map(|item| {
                        item.content
                            .iter()
                            .map(|b| self.block_height(b, width.saturating_sub(4)))
                            .sum::<usize>()
                            .max(1)
                    })
                    .sum();
                item_lines + 1
            }
            RenderedBlock::Table { headers, rows, .. } => {
                let col_widths = crate::view::table_column_widths(headers, rows, width);
                let wrap_height = |cells: &[StyledLine]| -> usize {
                    cells.iter().enumerate().map(|(i, c)| {
                        let cw = col_widths.get(i).copied().unwrap_or(5);
                        let len = c.text_content().len();
                        if cw == 0 || len <= cw { 1 } else { len.div_ceil(cw) }
                    }).max().unwrap_or(1)
                };
                let header_h = wrap_height(headers);
                let rows_h: usize = rows.iter().map(|r| wrap_height(r)).sum();
                header_h + 1 + rows_h + 1 // header + separator + rows + blank
            }
            RenderedBlock::HorizontalRule => 2,
            RenderedBlock::Image { .. } => 2,
        }
    }

    fn wrapped_line_count(&self, line: &StyledLine, width: usize) -> usize {
        if width == 0 {
            return 1;
        }
        let len = line.text_content().len();
        if len == 0 {
            return 1;
        }
        len.div_ceil(width)
    }

    pub fn calculate_total_lines(&mut self, blocks: &[RenderedBlock], width: usize) {
        self.total_lines = blocks.iter().map(|b| self.block_height(b, width)).sum();
    }

    pub fn scroll_down(&mut self, n: usize, viewport_height: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(self.max_scroll(viewport_height));
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn jump_top(&mut self) {
        self.scroll_offset = 0;
        self.cursor_line = 0;
    }

    pub fn jump_bottom(&mut self, viewport_height: usize) {
        self.scroll_offset = self.max_scroll(viewport_height);
    }

    /// Maximum permissible `scroll_offset`. Reserves `SCROLLOFF` rows of
    /// whitespace below the last content row, so jumping/paging to the end of
    /// the buffer still leaves a small gutter at the bottom of the viewport.
    fn max_scroll(&self, viewport_height: usize) -> usize {
        if self.total_lines + SCROLLOFF > viewport_height {
            self.total_lines + SCROLLOFF - viewport_height
        } else {
            0
        }
    }

    pub fn visible_blocks<'a>(
        &self,
        blocks: &'a [RenderedBlock],
        width: usize,
        viewport_height: usize,
    ) -> Vec<PositionedBlock<'a>> {
        let mut result = Vec::new();
        let mut y = 0;
        let view_start = self.scroll_offset;
        let view_end = self.scroll_offset + viewport_height;
        for block in blocks {
            let h = self.block_height(block, width);
            let block_end = y + h;
            if block_end > view_start && y < view_end {
                result.push(PositionedBlock {
                    block,
                    y_offset: y,
                    height: h,
                });
            }
            if y >= view_end {
                break;
            }
            y += h;
        }
        result
    }

    /// Ensure the cursor line is visible with scrolloff margin.
    pub fn ensure_cursor_visible(&mut self, cursor_line: usize, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if cursor_line < self.scroll_offset + SCROLLOFF {
            self.scroll_offset = cursor_line.saturating_sub(SCROLLOFF);
        } else if cursor_line >= self.scroll_offset + viewport_height.saturating_sub(SCROLLOFF) {
            self.scroll_offset = cursor_line + SCROLLOFF + 1 - viewport_height;
        }
    }

    pub fn find_heading_offset(
        &self,
        blocks: &[RenderedBlock],
        n: usize,
        width: usize,
    ) -> Option<usize> {
        let mut y = 0;
        let mut count = 0;
        for block in blocks {
            if matches!(block, RenderedBlock::Heading { .. }) {
                if count == n {
                    return Some(y);
                }
                count += 1;
            }
            y += self.block_height(block, width);
        }
        None
    }
}
