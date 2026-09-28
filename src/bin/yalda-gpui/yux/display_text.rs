//! Display-text projection of an editor [`Document`] — the ONE place a source
//! line becomes the string a text surface renders, and the ONE raw→display
//! column mapper (text-editing review C2 / C4).
//!
//! Every text surface (the buffer Edit view in Code + WP, the agent transcript,
//! the chatbox compose, the inline You-block) renders a line with its trailing
//! `\n` trimmed and each TAB expanded to [`TAB_DISPLAY_WIDTH`] spaces. The
//! editor's caret and selection, however, are in RAW char columns (a tab is one
//! column). Painting a raw column onto the expanded string drifts the caret /
//! selection left by `TAB_DISPLAY_WIDTH - 1` per preceding tab — so every
//! surface that expands tabs MUST project its caret + selection through
//! [`display_col`] / [`display_selection`] before handing them to the painter.

use yalda::document::Document;

/// Columns one TAB occupies on screen (fixed expansion, not tab stops — the
/// historical `replace('\t', "    ")` rule every surface already used).
pub(crate) const TAB_DISPLAY_WIDTH: usize = 4;

/// Push `ch` onto `out` in display form (TAB → spaces).
#[inline]
fn push_display_char(out: &mut String, ch: char) {
    if ch == '\t' {
        for _ in 0..TAB_DISPLAY_WIDTH {
            out.push(' ');
        }
    } else {
        out.push(ch);
    }
}

/// Display form of one raw line string (may carry its trailing `\n`).
pub(crate) fn display_text_of(raw_line: &str) -> String {
    let t = raw_line.trim_end_matches('\n');
    let mut out = String::with_capacity(t.len());
    for ch in t.chars() {
        push_display_char(&mut out, ch);
    }
    out
}

/// Display form of document line `line` (empty string past the end). Walks the
/// rope slice directly — one allocation, no intermediate `line_text` copy.
pub(crate) fn display_line(doc: &Document, line: usize) -> String {
    if line >= doc.line_count() {
        return String::new();
    }
    let slice = doc.rope().line(line);
    let mut out = String::with_capacity(slice.len_bytes());
    for ch in slice.chars() {
        push_display_char(&mut out, ch);
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Display form of every line of `doc` (always ≥ 1 entry, so an empty document
/// still yields one empty row for the caret).
pub(crate) fn display_lines(doc: &Document) -> Vec<String> {
    (0..doc.line_count().max(1))
        .map(|i| display_line(doc, i))
        .collect()
}

/// RAW → DISPLAY column over a line's chars (TABs expanded). Columns past the
/// line end (the EOL caret) map 1:1 past the expanded end.
fn display_col_of_chars(chars: impl Iterator<Item = char>, raw_col: usize) -> usize {
    let mut seen = 0usize;
    let mut disp = 0usize;
    for ch in chars {
        if seen >= raw_col || ch == '\n' {
            break;
        }
        disp += if ch == '\t' { TAB_DISPLAY_WIDTH } else { 1 };
        seen += 1;
    }
    disp + raw_col.saturating_sub(seen)
}

/// Map a RAW `(line, col)` caret column on document line `line` to its DISPLAY
/// column. O(col) — only the caret / selection endpoints are ever mapped.
pub(crate) fn display_col(doc: &Document, line: usize, raw_col: usize) -> usize {
    if line >= doc.line_count() {
        return raw_col;
    }
    display_col_of_chars(doc.rope().line(line).chars(), raw_col)
}

/// DISPLAY → RAW column over a line's chars — the inverse of
/// [`display_col_of_chars`]. A display column that falls INSIDE a tab's
/// expansion snaps to the nearer raw boundary (before the tab for the left
/// half, after it for the right). Columns past the expanded end map 1:1 past
/// the raw end (callers clamp to the line length).
fn raw_col_of_display_chars(chars: impl Iterator<Item = char>, display_col: usize) -> usize {
    let mut raw = 0usize;
    let mut disp = 0usize;
    for ch in chars {
        if ch == '\n' || disp >= display_col {
            break;
        }
        let w = if ch == '\t' { TAB_DISPLAY_WIDTH } else { 1 };
        if disp + w > display_col {
            // Inside this char's cells (only possible for a TAB).
            return if display_col - disp <= w / 2 { raw } else { raw + 1 };
        }
        disp += w;
        raw += 1;
    }
    raw + display_col.saturating_sub(disp)
}

/// Map a DISPLAY column on document line `line` (e.g. a mouse hit-test on the
/// tab-expanded row) back to the RAW editor column. O(col). Every pointer →
/// caret conversion on a tab-expanding surface MUST go through this, or a
/// click after a TAB lands `TAB_DISPLAY_WIDTH - 1` columns too far right.
pub(crate) fn raw_col_from_display(doc: &Document, line: usize, display_col: usize) -> usize {
    if line >= doc.line_count() {
        return display_col;
    }
    raw_col_of_display_chars(doc.rope().line(line).chars(), display_col)
}

/// Project an editor selection (`((start_line, start_col), (end_line,
/// end_col))`, raw columns) into DISPLAY columns so it lines up with the
/// tab-expanded rows.
pub(crate) fn display_selection(
    doc: &Document,
    sel: Option<((usize, usize), (usize, usize))>,
) -> Option<((usize, usize), (usize, usize))> {
    sel.map(|((sl, sc), (el, ec))| {
        (
            (sl, display_col(doc, sl, sc)),
            (el, display_col(doc, el, ec)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_line_trims_newline_and_expands_tabs() {
        let doc = Document::from_text("\tfoo\nbar\t\n".into(), "t".into());
        assert_eq!(display_line(&doc, 0), "    foo");
        assert_eq!(display_line(&doc, 1), "bar    ");
        assert_eq!(display_line(&doc, 2), "");
        assert_eq!(display_line(&doc, 9), "");
        assert_eq!(display_lines(&doc), vec!["    foo", "bar    ", ""]);
        assert_eq!(display_text_of("\ta\n"), "    a");
    }

    #[test]
    fn display_col_expands_preceding_tabs_only() {
        let doc = Document::from_text("\t\tab\n".into(), "t".into());
        assert_eq!(display_col(&doc, 0, 0), 0);
        assert_eq!(display_col(&doc, 0, 1), 4);
        assert_eq!(display_col(&doc, 0, 2), 8);
        assert_eq!(display_col(&doc, 0, 3), 9);
        // EOL caret (one past the last char) maps past the expanded end.
        assert_eq!(display_col(&doc, 0, 4), 10);
        assert_eq!(display_col_of_chars("x\ty".chars(), 2), 5);
        assert_eq!(
            display_selection(&doc, Some(((0, 1), (0, 3)))),
            Some(((0, 4), (0, 9)))
        );
    }

    #[test]
    fn raw_col_from_display_inverts_display_col() {
        let doc = Document::from_text("\t\tab\nx\ty\n".into(), "t".into());
        // Round-trip every raw column (incl. EOL and past it).
        for line in 0..2 {
            for raw in 0..8 {
                let d = display_col(&doc, line, raw);
                assert_eq!(raw_col_from_display(&doc, line, d), raw, "line {line} raw {raw}");
            }
        }
        // Inside a tab's expansion: left half → before, right half → after.
        assert_eq!(raw_col_from_display(&doc, 0, 1), 0);
        assert_eq!(raw_col_from_display(&doc, 0, 2), 0);
        assert_eq!(raw_col_from_display(&doc, 0, 3), 1);
        assert_eq!(raw_col_from_display(&doc, 1, 2), 1);
        assert_eq!(raw_col_from_display(&doc, 1, 4), 2);
        assert_eq!(raw_col_from_display(&doc, 1, 5), 2, "x + tab = 5 cols → before y");
        assert_eq!(raw_col_from_display(&doc, 9, 3), 3, "past the doc: identity");
    }
}
