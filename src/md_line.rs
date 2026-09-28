//! Line-prefix markdown markers — the ONE byte-based parser for list bullets,
//! ordered-list numbers, checkboxes and blockquote `>` runs (text-editing
//! review C9).
//!
//! Three surfaces used to re-implement this detection with subtly different
//! char/byte walks: Enter's list continuation (`edit_ui::parse_list_marker`),
//! the WP line classifier (`render_blocks::classify_wp_line`) and the source
//! highlighter (`md_highlight`). They now all call these primitives. Every
//! marker is ASCII, so a returned length is BOTH a byte and a char count.
//!
//! Every function takes the text *after* any leading indent (callers decide
//! what counts as indent) and looks only at its start.

/// `-`, `*` or `+` followed by a space ⇒ the bullet byte. Marker length is 2.
pub fn bullet_marker(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    match (b.first(), b.get(1)) {
        (Some(&c @ (b'-' | b'*' | b'+')), Some(b' ')) => Some(c),
        _ => None,
    }
}

/// One or more ASCII digits, then `.` or `)`, then a space ⇒ `(digit count,
/// separator byte)`. Marker length is `digits + 2`.
pub fn ordered_marker(s: &str) -> Option<(usize, u8)> {
    let b = s.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    match (b.get(digits), b.get(digits + 1)) {
        (Some(&sep @ (b'.' | b')')), Some(b' ')) => Some((digits, sep)),
        _ => None,
    }
}

/// Length of a leading bullet or ordered-list marker (incl. its trailing
/// space), or `None`. Checkboxes are NOT consumed here (see [`checkbox_after_bullet`]).
pub fn list_marker_len(s: &str) -> Option<usize> {
    if bullet_marker(s).is_some() {
        return Some(2);
    }
    ordered_marker(s).map(|(digits, _)| digits + 2)
}

/// A bullet followed by a task box — `- [ ] `, `- [x] `, `- [X] ` — ⇒ the
/// checked state. The whole marker is 6 bytes.
pub fn checkbox_after_bullet(s: &str) -> Option<bool> {
    bullet_marker(s)?;
    let b = s.as_bytes();
    match (b.get(2), b.get(3), b.get(4), b.get(5)) {
        (Some(b'['), Some(&st @ (b' ' | b'x' | b'X')), Some(b']'), Some(b' ')) => Some(st != b' '),
        _ => None,
    }
}

/// Length of a leading blockquote prefix: a run of `>` each optionally
/// followed by ONE space (`>`, `> `, `>>`, `> > `). 0 when `s` doesn't start
/// with `>`.
pub fn quote_prefix_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while b.get(i) == Some(&b'>') {
        i += 1;
        if b.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullets() {
        assert_eq!(bullet_marker("- x"), Some(b'-'));
        assert_eq!(bullet_marker("* x"), Some(b'*'));
        assert_eq!(bullet_marker("+ "), Some(b'+'));
        assert_eq!(bullet_marker("-x"), None);
        assert_eq!(bullet_marker("-"), None);
        assert_eq!(bullet_marker(""), None);
        assert_eq!(bullet_marker("• x"), None, "non-ASCII lead byte is not a bullet");
    }

    #[test]
    fn ordered() {
        assert_eq!(ordered_marker("1. x"), Some((1, b'.')));
        assert_eq!(ordered_marker("12) x"), Some((2, b')')));
        assert_eq!(ordered_marker("1."), None);
        assert_eq!(ordered_marker("1.x"), None);
        assert_eq!(ordered_marker(". x"), None);
        assert_eq!(ordered_marker("a. x"), None);
        assert_eq!(list_marker_len("12. x"), Some(4));
        assert_eq!(list_marker_len("- x"), Some(2));
        assert_eq!(list_marker_len("x"), None);
    }

    #[test]
    fn checkboxes() {
        assert_eq!(checkbox_after_bullet("- [ ] a"), Some(false));
        assert_eq!(checkbox_after_bullet("* [x] a"), Some(true));
        assert_eq!(checkbox_after_bullet("+ [X] "), Some(true));
        assert_eq!(checkbox_after_bullet("- [y] a"), None);
        assert_eq!(checkbox_after_bullet("- [ ]a"), None);
        assert_eq!(checkbox_after_bullet("1. [ ] a"), None);
    }

    fn for_each_corpus(max_len: usize, f: &mut dyn FnMut(&str)) {
        const ALPHA: [char; 12] = ['-', '+', '>', ' ', '1', '.', ')', '[', ']', 'x', 'a', '*'];
        fn go(buf: &mut String, depth: usize, f: &mut dyn FnMut(&str)) {
            f(buf);
            if depth == 0 {
                return;
            }
            for c in ALPHA {
                buf.push(c);
                go(buf, depth - 1, f);
                buf.pop();
            }
        }
        go(&mut String::new(), max_len, f);
    }

    /// C9 behavior pin: the pre-refactor `md_highlight` walkers, verbatim, as
    /// oracles — the shared parser must agree with them on every input.
    #[test]
    fn matches_legacy_highlighter_walkers() {
        fn old_list_marker_len(s: &str) -> Option<usize> {
            let bytes = s.as_bytes();
            if bytes.is_empty() {
                return None;
            }
            if matches!(bytes[0], b'-' | b'*' | b'+') && bytes.get(1) == Some(&b' ') {
                return Some(2);
            }
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
        fn old_split_quote_prefix(s: &str) -> usize {
            let bytes = s.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'>' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            i
        }
        for_each_corpus(6, &mut |s| {
            assert_eq!(list_marker_len(s), old_list_marker_len(s), "list_marker_len({s:?})");
            assert_eq!(quote_prefix_len(s), old_split_quote_prefix(s), "quote({s:?})");
        });
    }

    #[test]
    fn quotes() {
        assert_eq!(quote_prefix_len("> a"), 2);
        assert_eq!(quote_prefix_len(">a"), 1);
        assert_eq!(quote_prefix_len(">> a"), 3);
        assert_eq!(quote_prefix_len("> > a"), 4);
        assert_eq!(quote_prefix_len("a > b"), 0);
        assert_eq!(quote_prefix_len(""), 0);
    }
}
