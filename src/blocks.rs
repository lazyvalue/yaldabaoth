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
