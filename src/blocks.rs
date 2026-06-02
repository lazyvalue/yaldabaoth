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
        /// True when this block represents an entire source file opened
        /// directly (`.rs`, `.py`, etc.) rather than a fenced code block
        /// inside markdown. Renderers should skip container chrome
        /// (background, padding, rounded corners) for source-file blocks.
        source_file: bool,
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
