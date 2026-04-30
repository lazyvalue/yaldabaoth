use ratatui::style::Style;

use crate::theme::Theme;

/// A highlighted chunk of text: owned string + style.
pub type Segment = (String, Style);

/// Highlight raw markdown source line-by-line.
///
/// Tracks fenced code block state across lines. Returns one `Vec<Segment>` per input line.
pub fn highlight_markdown_lines(lines: &[String], theme: &Theme) -> Vec<Vec<Segment>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    let code_bg = theme.code_block_bg;
    for raw in lines {
        let line = raw.as_str();
        let trimmed_start = line.trim_start();

        let opens_or_closes_fence =
            trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~");

        if in_fence {
            let style = code_bg.patch(theme.paragraph);
            out.push(vec![(line.to_string(), style)]);
            if opens_or_closes_fence {
                in_fence = false;
            }
            continue;
        }

        if opens_or_closes_fence {
            in_fence = true;
            let style = code_bg.patch(theme.code_inline);
            out.push(vec![(line.to_string(), style)]);
            continue;
        }

        out.push(highlight_source_line(line, theme));
    }
    out
}

fn highlight_source_line(line: &str, theme: &Theme) -> Vec<Segment> {
    if line.is_empty() {
        return vec![(String::new(), theme.paragraph)];
    }

    // Whole-line constructs:
    if let Some(seg) = try_heading(line, theme) {
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
        segs.push((quote_prefix.to_string(), theme.blockquote_bar));
        tokenize_inline(after_quote, theme.blockquote_text, theme, &mut segs);
        return segs;
    }

    // List marker: `-`, `*`, `+` followed by space — or `N.` / `N)`.
    if let Some(marker_end) = list_marker_len(rest) {
        segs.push((rest[..marker_end].to_string(), theme.list_marker));
        tokenize_inline(&rest[marker_end..], theme.paragraph, theme, &mut segs);
        return segs;
    }

    tokenize_inline(rest, theme.paragraph, theme, &mut segs);
    segs
}

fn try_heading(line: &str, theme: &Theme) -> Option<Vec<Segment>> {
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
    Some(vec![(line.to_string(), style)])
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
fn tokenize_inline(text: &str, base_style: Style, theme: &Theme, out: &mut Vec<Segment>) {
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
                out.push((text[i..=end].to_string(), theme.code_inline));
                i = end + 1;
                plain_start = i;
                continue;
            }
        }

        // **bold**
        if b == b'*' && bytes.get(i + 1) == Some(&b'*') {
            if let Some(end) = find_double(bytes, i + 2, b'*') {
                flush_plain(out, text, plain_start, i);
                out.push((text[i..end + 2].to_string(), theme.bold));
                i = end + 2;
                plain_start = i;
                continue;
            }
        }

        // *italic* or _italic_
        if b == b'*' || b == b'_' {
            if let Some(end) = find_single_emphasis(bytes, i + 1, b) {
                flush_plain(out, text, plain_start, i);
                out.push((text[i..=end].to_string(), theme.italic));
                i = end + 1;
                plain_start = i;
                continue;
            }
        }

        // ~~strike~~
        if b == b'~' && bytes.get(i + 1) == Some(&b'~') {
            if let Some(end) = find_double(bytes, i + 2, b'~') {
                flush_plain(out, text, plain_start, i);
                out.push((text[i..end + 2].to_string(), theme.strikethrough));
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
                        out.push((text[i..=rp].to_string(), theme.link));
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
