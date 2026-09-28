use crate::style::Style;

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledLine {
    pub spans: Vec<StyledSpan>,
}

/// Where a top-level [`RenderedBlock`] came from in its source text.
/// `bytes` is the element's byte range; `lines` the 0-based, half-open range of
/// source lines it occupies. Lets views map between a rendered block and the
/// raw text (view⇄edit position, outline, checkbox toggles).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub bytes: std::ops::Range<usize>,
    pub lines: std::ops::Range<usize>,
}

/// Top-level rendered blocks plus a parallel `spans[i]` for `blocks[i]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rendered {
    pub blocks: Vec<RenderedBlock>,
    pub spans: Vec<SourceSpan>,
}

impl From<Vec<RenderedBlock>> for Rendered {
    /// Unmapped blocks (no source text, e.g. string-backed docs): no spans.
    fn from(blocks: Vec<RenderedBlock>) -> Self {
        Self {
            blocks,
            spans: Vec::new(),
        }
    }
}

impl Rendered {
    /// True when every block carries a span (the text these blocks came from
    /// is known).
    pub fn is_mapped(&self) -> bool {
        !self.blocks.is_empty() && self.spans.len() == self.blocks.len()
    }

    /// Index of the block whose source lines contain `line`, else the last
    /// block starting at or before it (blank lines between blocks map to the
    /// preceding block). `None` when there are no blocks.
    pub fn block_at_line(&self, line: usize) -> Option<usize> {
        if self.spans.is_empty() {
            return None;
        }
        let after = self.spans.partition_point(|s| s.lines.start <= line);
        Some(after.saturating_sub(1))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub marker: String,
    pub checked: Option<bool>,
    pub content: Vec<RenderedBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderedBlock {
    Heading {
        level: u8,
        content: StyledLine,
    },
    Paragraph {
        lines: Vec<StyledLine>,
    },
    CodeBlock {
        language: Option<String>,
        lines: Vec<StyledLine>,
        /// True when this block represents source-file content opened
        /// directly (`.rs`, `.py`, etc.) rather than a fenced code block
        /// inside markdown. Renderers should skip container chrome
        /// (background, padding, rounded corners) for source-file blocks.
        /// Source files are split into one block per line so block-based
        /// scrolling and list virtualization work line-by-line.
        source_file: bool,
        /// 0-based index of `lines[0]` within the originating source file.
        /// Drives line-number gutters when a file is split across blocks.
        /// Always 0 for fenced markdown code blocks.
        start_line: usize,
    },
    BlockQuote {
        blocks: Vec<RenderedBlock>,
    },
    List {
        ordered: bool,
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Table {
        headers: Vec<StyledLine>,
        rows: Vec<Vec<StyledLine>>,
        alignments: Vec<ColumnAlignment>,
    },
    HorizontalRule,
    /// A ` ```mermaid ` fenced block. `source` is the exact fence body (the
    /// mermaid text), rendered off-thread to a diagram image by the frontend;
    /// `lines` are the syntax-highlighted source lines used as the placeholder
    /// (until the image is ready) and the fallback (when the renderer is
    /// absent or errors). Structurally distinct from `CodeBlock` so renderers
    /// can paint an image and opt out of per-line code hit-testing.
    /// See `UXI-Diagram-1` (`docs/components/common/diagram.md`).
    Diagram {
        source: String,
        lines: Vec<StyledLine>,
    },
    Image {
        alt: String,
        url: String,
    },
    /// A document's leading frontmatter (`---` … `---`, or `+++` … `+++`), one
    /// `StyledLine` per source line. Structurally distinct from a paragraph so
    /// renderers can de-emphasize it: it is metadata ABOUT the document, not the
    /// document's own prose, and it must never read as the title (bug-0014).
    Metadata {
        lines: Vec<StyledLine>,
    },
}

impl StyledSpan {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn with_link(mut self, url: impl Into<String>) -> Self {
        self.link = Some(url.into());
        self
    }
}

impl StyledLine {
    pub fn new(spans: Vec<StyledSpan>) -> Self {
        Self { spans }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![StyledSpan::new(text, Style::default())],
        }
    }

    pub fn text_content(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}
