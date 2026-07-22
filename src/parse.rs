use pulldown_cmark::{Event, Options, Parser};

/// Parse markdown text into a pulldown-cmark event iterator.
/// Enables all CommonMark extensions we support.
pub fn parse(markdown: &str) -> impl Iterator<Item = Event<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    // bug-0014: without these, a leading `---` … `---` frontmatter block parses as
    // CommonMark intends — thematic break, then a paragraph the CLOSING `---`
    // promotes to a setext `<h2>` — so every `.claude/agents/*.md` opened as one
    // enormous run-on heading. With them, the block arrives as
    // `Tag::MetadataBlock` and `render.rs` gives it its own de-emphasized
    // `RenderedBlock::Metadata`.
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    options.insert(Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS);
    Parser::new_ext(markdown, options)
}
