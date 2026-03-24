use pulldown_cmark::{Event, Options, Parser};

/// Parse markdown text into a pulldown-cmark event iterator.
/// Enables all CommonMark extensions we support.
pub fn parse(markdown: &str) -> impl Iterator<Item = Event<'_>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    Parser::new_ext(markdown, options)
}
