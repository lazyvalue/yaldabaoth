use crate::highlight::Highlighter;
use crate::style::Style;
use crate::theme::Theme;

/// A highlighted chunk of text: owned string + style.
pub type Segment = (String, Style);

/// Running state across lines for fenced code blocks.
#[derive(Clone, Debug, Default)]
pub struct FenceState {
    pub in_fence: bool,
    pub lang: Option<String>,
}

impl FenceState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Highlight raw markdown source line-by-line.
///
/// Tracks fenced code block state across lines. Returns one `Vec<Segment>` per input line.
pub fn highlight_markdown_lines(lines: &[String], theme: &Theme) -> Vec<Vec<Segment>> {
    highlight_markdown_lines_inner(lines, theme, false, None)
}

/// Like `highlight_markdown_lines`, but strips inline delimiters (`**`, `` ` ``,
/// `~~`, `#` prefixes, link syntax) so the output reads as clean prose with
/// styling only. Used by the agent pane where raw markup is noise.
pub fn highlight_markdown_lines_stripped(lines: &[String], theme: &Theme) -> Vec<Vec<Segment>> {
    highlight_markdown_lines_inner(lines, theme, true, None)
}

/// Highlight with syntect-based code block coloring.
pub fn highlight_markdown_lines_syn(
    lines: &[String],
    theme: &Theme,
    hl: &Highlighter,
) -> Vec<Vec<Segment>> {
    highlight_markdown_lines_inner(lines, theme, false, Some(hl))
}

/// Highlight stripped with syntect-based code block coloring.
pub fn highlight_markdown_lines_stripped_syn(
    lines: &[String],
    theme: &Theme,
    hl: &Highlighter,
) -> Vec<Vec<Segment>> {
    highlight_markdown_lines_inner(lines, theme, true, Some(hl))
}

fn highlight_markdown_lines_inner(
    lines: &[String],
    theme: &Theme,
    strip: bool,
    hl: Option<&Highlighter>,
) -> Vec<Vec<Segment>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut fence = FenceState::new();
    for raw in lines {
        let (segs, next_fence) = highlight_one_line(raw, &fence, theme, strip, hl);
        fence = next_fence;
        out.push(segs);
    }
    out
}

/// Highlight a single line given the running fenced-code-block state on entry.
///
/// Returns the line's segments plus the fence state *after* this line, so a
/// caller doing incremental/cached highlighting can carry the state forward.
/// This is the single source of truth for per-line highlighting;
/// `highlight_markdown_lines_inner` is just this folded over a slice. Keeping
/// both on the same path guarantees cached output is byte-identical to the
/// batch path.
pub fn highlight_one_line(
    line: &str,
    fence: &FenceState,
    theme: &Theme,
    strip: bool,
    hl: Option<&Highlighter>,
) -> (Vec<Segment>, FenceState) {
    let trimmed_start = line.trim_start();
    let opens_or_closes_fence =
        trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~");
    let code_bg = theme.code_block_bg;

    if fence.in_fence {
        if opens_or_closes_fence {
            // Closing fence
            let next = FenceState {
                in_fence: false,
                lang: None,
            };
            let segs = if strip {
                vec![(String::new(), code_bg.patch(theme.code_inline))]
            } else {
                vec![(line.to_string(), code_bg.patch(theme.code_inline))]
            };
            return (segs, next);
        }
        // Inside a fenced code block — try syntect highlighting.
        let segs = if let (Some(hl), Some(lang)) = (hl, fence.lang.as_deref()) {
            if let Some(highlighted) = hl.highlight_line_stateless(lang, line, code_bg) {
                highlighted
            } else {
                vec![(line.to_string(), code_bg.patch(theme.paragraph))]
            }
        } else {
            vec![(line.to_string(), code_bg.patch(theme.paragraph))]
        };
        let next = FenceState {
            in_fence: true,
            lang: fence.lang.clone(),
        };
        return (segs, next);
    }

    if opens_or_closes_fence {
        // Opening fence — extract language tag.
        let marker_char = trimmed_start.as_bytes()[0];
        let marker_end = trimmed_start
            .bytes()
            .take_while(|&b| b == marker_char)
            .count();
        let info = trimmed_start[marker_end..].trim();
        // The info string may contain spaces (e.g. "rust,no_run"); take the
        // first word as the language token, matching pulldown-cmark.
        let lang_token = info.split_whitespace().next().unwrap_or("");
        let lang = if lang_token.is_empty() {
            None
        } else {
            Some(lang_token.to_string())
        };
        let next = FenceState {
            in_fence: true,
            lang,
        };
        let segs = if strip {
            vec![(String::new(), code_bg.patch(theme.code_inline))]
        } else {
            vec![(line.to_string(), code_bg.patch(theme.code_inline))]
        };
        return (segs, next);
    }

    // Not in a fence, not a fence marker — normal markdown line.
    let segs = highlight_source_line(line, theme, strip);
    let next = FenceState {
        in_fence: false,
        lang: None,
    };
    (segs, next)
}

fn highlight_source_line(line: &str, theme: &Theme, strip: bool) -> Vec<Segment> {
    if line.is_empty() {
        return vec![(String::new(), theme.paragraph)];
    }

    // Whole-line constructs:
    if let Some(seg) = try_heading(line, theme, strip) {
        return seg;
    }
    if is_horizontal_rule(line) {
        return vec![(line.to_string(), theme.horizontal_rule)];
    }

    // Line-prefix constructs (quote / list), then inline-tokenize the rest.
    let mut segs = Vec::new();

    let leading_ws_len = line.len() - line.trim_start().len();
    if leading_ws_len > 0 {
        segs.push((line[..leading_ws_len].to_string(), theme.paragraph));
    }
    let rest = &line[leading_ws_len..];

    // Blockquote: consume a leading '>' marker (possibly repeated).
    let (quote_prefix, after_quote) = split_quote_prefix(rest);
    if !quote_prefix.is_empty() {
        if !strip {
            segs.push((quote_prefix.to_string(), theme.blockquote_bar));
        }
        tokenize_inline(after_quote, theme.blockquote_text, theme, &mut segs, strip);
        return segs;
    }

    // List marker: `-`, `*`, `+` followed by space — or `N.` / `N)`.
    if let Some(marker_end) = list_marker_len(rest) {
        if strip {
            // Replace the raw marker with a bullet for unordered, keep digits for ordered.
            let marker_text = &rest[..marker_end];
            let is_ordered = marker_text.as_bytes()[0].is_ascii_digit();
            if is_ordered {
                segs.push((marker_text.to_string(), theme.list_marker));
            } else {
                segs.push(("\u{2022} ".to_string(), theme.list_marker));
            }
        } else {
            segs.push((rest[..marker_end].to_string(), theme.list_marker));
        }
        tokenize_inline(&rest[marker_end..], theme.paragraph, theme, &mut segs, strip);
        return segs;
    }

    tokenize_inline(rest, theme.paragraph, theme, &mut segs, strip);
    segs
}

fn try_heading(line: &str, theme: &Theme, strip: bool) -> Option<Vec<Segment>> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes_end = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes_end == 0 || hashes_end > 6 {
        return None;
    }
    // Must be followed by space or be the only thing on the line.
    let after = &trimmed[hashes_end..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let style = theme.heading[hashes_end - 1];
    if strip {
        // Drop the `# ` prefix, keep just the heading text.
        let text = after.strip_prefix(' ').unwrap_or(after);
        Some(vec![(text.to_string(), style)])
    } else {
        Some(vec![(line.to_string(), style)])
    }
}

fn is_horizontal_rule(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    let first = t.chars().next().unwrap();
    matches!(first, '-' | '*' | '_') && t.chars().all(|c| c == first || c == ' ')
}

fn split_quote_prefix(s: &str) -> (&str, &str) {
    // Accept `>`, `> `, `>>`, `> >`, etc. at the very start.
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut any = false;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            any = true;
            i += 1;
            if i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
        } else {
            break;
        }
    }
    if any {
        (&s[..i], &s[i..])
    } else {
        ("", s)
    }
}

fn list_marker_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Unordered.
    if matches!(bytes[0], b'-' | b'*' | b'+') && bytes.get(1) == Some(&b' ') {
        return Some(2);
    }
    // Ordered: one or more digits + `.` or `)` + space.
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0
        && i < bytes.len()
        && (bytes[i] == b'.' || bytes[i] == b')')
        && bytes.get(i + 1) == Some(&b' ')
    {
        return Some(i + 2);
    }
    None
}

/// Tokenize inline markdown into styled segments. Recognizes:
///   `**bold**`, `*italic*`, `_italic_`, `` `code` ``, `~~strike~~`, `[text](url)`.
/// `base_style` is applied to plain text; emphasis patches over it.
///
/// When `strip` is true, delimiter characters are omitted from the output text
/// (the styling is still applied). This makes output read as clean prose.
fn tokenize_inline(
    text: &str,
    base_style: Style,
    theme: &Theme,
    out: &mut Vec<Segment>,
    strip: bool,
) {
    if text.is_empty() {
        return;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;

    let flush_plain = |out: &mut Vec<Segment>, text: &str, start: usize, end: usize| {
        if start < end {
            out.push((text[start..end].to_string(), base_style));
        }
    };

    while i < bytes.len() {
        let b = bytes[i];

        // `code`
        if b == b'`' {
            if let Some(end) = find_unescaped(bytes, i + 1, b'`') {
                flush_plain(out, text, plain_start, i);
                if strip {
                    out.push((text[i + 1..end].to_string(), theme.code_inline));
                } else {
                    out.push((text[i..=end].to_string(), theme.code_inline));
                }
                i = end + 1;
                plain_start = i;
                continue;
            }
        }

        // **bold**
        if b == b'*' && bytes.get(i + 1) == Some(&b'*') {
            if let Some(end) = find_double(bytes, i + 2, b'*') {
                flush_plain(out, text, plain_start, i);
                if strip {
                    out.push((text[i + 2..end].to_string(), theme.bold));
                } else {
                    out.push((text[i..end + 2].to_string(), theme.bold));
                }
                i = end + 2;
                plain_start = i;
                continue;
            }
        }

        // *italic* or _italic_
        if b == b'*' || b == b'_' {
            if let Some(end) = find_single_emphasis(bytes, i + 1, b) {
                flush_plain(out, text, plain_start, i);
                if strip {
                    out.push((text[i + 1..end].to_string(), theme.italic));
                } else {
                    out.push((text[i..=end].to_string(), theme.italic));
                }
                i = end + 1;
                plain_start = i;
                continue;
            }
        }

        // ~~strike~~
        if b == b'~' && bytes.get(i + 1) == Some(&b'~') {
            if let Some(end) = find_double(bytes, i + 2, b'~') {
                flush_plain(out, text, plain_start, i);
                if strip {
                    out.push((text[i + 2..end].to_string(), theme.strikethrough));
                } else {
                    out.push((text[i..end + 2].to_string(), theme.strikethrough));
                }
                i = end + 2;
                plain_start = i;
                continue;
            }
        }

        // [text](url)
        if b == b'[' {
            if let Some(rb) = find_unescaped(bytes, i + 1, b']') {
                if bytes.get(rb + 1) == Some(&b'(') {
                    if let Some(rp) = find_unescaped(bytes, rb + 2, b')') {
                        flush_plain(out, text, plain_start, i);
                        if strip {
                            // Show just the link text, styled as a link.
                            out.push((text[i + 1..rb].to_string(), theme.link));
                        } else {
                            out.push((text[i..=rp].to_string(), theme.link));
                        }
                        i = rp + 1;
                        plain_start = i;
                        continue;
                    }
                }
            }
        }

        i += 1;
    }

    if plain_start < bytes.len() {
        out.push((text[plain_start..].to_string(), base_style));
    }
}

fn find_unescaped(bytes: &[u8], from: usize, target: u8) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_double(bytes: &[u8], from: usize, target: u8) -> Option<usize> {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == target && bytes[i + 1] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_single_emphasis(bytes: &[u8], from: usize, marker: u8) -> Option<usize> {
    // Don't allow an empty emphasis (*x*), require at least one char, and
    // don't match **...** accidentally (caller handles that case first).
    if from >= bytes.len() {
        return None;
    }
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == marker {
            // Make sure this isn't a doubled marker (would belong to bold/strike).
            if bytes.get(i + 1) == Some(&marker) {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}
