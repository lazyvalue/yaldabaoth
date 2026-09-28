//! Single-line text input: the shared model behind every query / filter /
//! rename field in the GUI (palettes, pickers, overlays, the file browser).
//!
//! Before this existed each field was a bare `String` edited with
//! `push`/`pop`, re-implemented per site with diverging modifier rules. A
//! `LineInput` owns the text plus a movable caret and interprets the editing
//! keys once; the owning site keeps its own Enter / Esc / Up / Down handling
//! and reacts to [`LineEdit::Edited`] (re-filter, reset selection, notify).
//!
//! Frontend-neutral: driven by [`KeyPress`], so the lib-crate file browser
//! and the GUI overlays share it.

use crate::keys::{Key, KeyPress, Modifiers};

/// What a key did to a [`LineInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEdit {
    /// The text changed — re-filter / reset selection / notify.
    Edited,
    /// Only the caret moved — repaint.
    Moved,
    /// An editing key with nothing to do (Backspace at the start). Consumed.
    Unchanged,
    /// Not an editing key (Enter, Esc, arrows up/down, chords) — the caller
    /// handles it.
    Unhandled,
}

impl LineEdit {
    pub fn edited(self) -> bool {
        self == LineEdit::Edited
    }

    /// True for everything the input consumed (i.e. not `Unhandled`).
    pub fn handled(self) -> bool {
        self != LineEdit::Unhandled
    }
}

/// A single line of editable text with a caret (a byte offset that always
/// sits on a char boundary, `0..=text.len()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    caret: usize,
}

impl LineInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Prefilled input with the caret at the end (rename / cwd overlays).
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = sanitize(text.into());
        let caret = text.len();
        Self { text, caret }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace the whole text; the caret goes to the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        *self = Self::with_text(text);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// Insert at the caret (typing, paste). Newlines become spaces and `\r`
    /// is dropped — this is a single-line field.
    pub fn insert_str(&mut self, s: &str) {
        let s = sanitize(s.to_string());
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
    }

    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut buf));
    }

    /// The text with `glyph` drawn at the caret — the render string every
    /// input used to build as `format!("{text}█")` (identical when the caret
    /// is at the end).
    pub fn with_caret(&self, glyph: char) -> String {
        let mut out = String::with_capacity(self.text.len() + glyph.len_utf8());
        out.push_str(&self.text[..self.caret]);
        out.push(glyph);
        out.push_str(&self.text[self.caret..]);
        out
    }

    /// Interpret one key. Editing keys:
    ///
    /// | key | effect |
    /// |---|---|
    /// | typed char ([`KeyPress::typed_char`]) | insert |
    /// | Backspace / Delete | delete char back / forward |
    /// | Alt- or Ctrl-Backspace / -Delete | delete word back / forward |
    /// | Cmd-Backspace, Ctrl-U | delete to start |
    /// | Ctrl-K | delete to end |
    /// | Left / Right | char; Alt/Ctrl: word; Cmd: start/end |
    /// | Home / End, Ctrl-A / Ctrl-E | start / end |
    ///
    /// Ctrl-W is never consumed (reserved shell prefix).
    pub fn handle(&mut self, press: &KeyPress) -> LineEdit {
        if let Some(c) = press.typed_char() {
            self.insert_char(c);
            return LineEdit::Edited;
        }
        let m = press.modifiers;
        let word = m.contains(Modifiers::ALT) || m.contains(Modifiers::CONTROL);
        let cmd = m.contains(Modifiers::PLATFORM);
        let ctrl_only = m == Modifiers::CONTROL;
        match press.key {
            Key::Backspace if cmd => self.delete_to(0),
            Key::Backspace if word => self.delete_to(self.word_start_before()),
            Key::Backspace => self.delete_to(self.prev_boundary()),
            Key::Delete if word => self.delete_to(self.word_end_after()),
            Key::Delete => self.delete_to(self.next_boundary()),
            Key::Char('u' | 'U') if ctrl_only => self.delete_to(0),
            Key::Char('k' | 'K') if ctrl_only => self.delete_to(self.text.len()),
            Key::Left if cmd => self.move_to(0),
            Key::Right if cmd => self.move_to(self.text.len()),
            Key::Left if word => self.move_to(self.word_start_before()),
            Key::Right if word => self.move_to(self.word_end_after()),
            Key::Left => self.move_to(self.prev_boundary()),
            Key::Right => self.move_to(self.next_boundary()),
            Key::Home => self.move_to(0),
            Key::End => self.move_to(self.text.len()),
            Key::Char('a' | 'A') if ctrl_only => self.move_to(0),
            Key::Char('e' | 'E') if ctrl_only => self.move_to(self.text.len()),
            _ => LineEdit::Unhandled,
        }
    }

    /// Delete between the caret and `to` (either side). Caret lands at the
    /// lower end.
    fn delete_to(&mut self, to: usize) -> LineEdit {
        let (lo, hi) = if to < self.caret { (to, self.caret) } else { (self.caret, to) };
        if lo == hi {
            return LineEdit::Unchanged;
        }
        self.text.replace_range(lo..hi, "");
        self.caret = lo;
        LineEdit::Edited
    }

    fn move_to(&mut self, to: usize) -> LineEdit {
        if to == self.caret {
            return LineEdit::Unchanged;
        }
        self.caret = to;
        LineEdit::Moved
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.caret].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    /// Start of the word before the caret: skip separators, then word chars
    /// (so `foo/bar/|` → `foo/|`, matching native text fields on paths).
    fn word_start_before(&self) -> usize {
        let mut it = self.text[..self.caret].char_indices().rev().peekable();
        while it.next_if(|&(_, c)| !is_word(c)).is_some() {}
        let mut start = it.peek().map_or(0, |&(i, c)| i + c.len_utf8());
        while let Some((i, _)) = it.next_if(|&(_, c)| is_word(c)) {
            start = i;
        }
        start
    }

    /// End of the word after the caret: skip separators, then word chars.
    fn word_end_after(&self) -> usize {
        let rest = &self.text[self.caret..];
        let mut it = rest.char_indices().peekable();
        while it.next_if(|&(_, c)| !is_word(c)).is_some() {}
        while it.next_if(|&(_, c)| is_word(c)).is_some() {}
        self.caret + it.peek().map_or(rest.len(), |&(i, _)| i)
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn sanitize(s: String) -> String {
    if s.contains(['\n', '\r']) {
        s.replace('\r', "").replace('\n', " ")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, m: Modifiers) -> KeyPress {
        KeyPress::new(key, m)
    }
    fn ch(c: char) -> KeyPress {
        press(Key::Char(c), Modifiers::NONE)
    }
    fn typed(s: &str) -> LineInput {
        let mut i = LineInput::new();
        for c in s.chars() {
            assert_eq!(i.handle(&ch(c)), LineEdit::Edited);
        }
        i
    }

    #[test]
    fn typing_appends_and_backspace_pops() {
        let mut i = typed("héllo");
        assert_eq!(i.text(), "héllo");
        assert_eq!(i.caret(), i.text().len());
        assert_eq!(i.handle(&press(Key::Backspace, Modifiers::NONE)), LineEdit::Edited);
        assert_eq!(i.text(), "héll");
        assert_eq!(i.with_caret('█'), "héll█");
    }

    #[test]
    fn backspace_at_start_is_consumed_but_unchanged() {
        let mut i = LineInput::new();
        assert_eq!(i.handle(&press(Key::Backspace, Modifiers::NONE)), LineEdit::Unchanged);
    }

    #[test]
    fn chords_never_type() {
        let mut i = LineInput::new();
        for m in [Modifiers::PLATFORM, Modifiers::CONTROL, Modifiers::ALT] {
            assert_eq!(i.handle(&press(Key::Char('v'), m)), LineEdit::Unhandled, "{m:?}");
        }
        assert!(i.is_empty());
        // Ctrl-W is the reserved shell prefix: never consumed.
        assert_eq!(i.handle(&press(Key::Char('w'), Modifiers::CONTROL)), LineEdit::Unhandled);
    }

    #[test]
    fn alt_composed_symbols_type() {
        let mut i = LineInput::new();
        assert_eq!(i.handle(&press(Key::Char('@'), Modifiers::ALT)), LineEdit::Edited);
        assert_eq!(i.handle(&press(Key::Char('ß'), Modifiers::ALT)), LineEdit::Edited);
        assert_eq!(i.text(), "@ß");
    }

    #[test]
    fn caret_moves_and_inserts_mid_text() {
        let mut i = typed("ac");
        assert_eq!(i.handle(&press(Key::Left, Modifiers::NONE)), LineEdit::Moved);
        i.handle(&ch('b'));
        assert_eq!(i.text(), "abc");
        assert_eq!(i.with_caret('|'), "ab|c");
        assert_eq!(i.handle(&press(Key::Home, Modifiers::NONE)), LineEdit::Moved);
        assert_eq!(i.handle(&press(Key::Left, Modifiers::NONE)), LineEdit::Unchanged);
        assert_eq!(i.handle(&press(Key::Delete, Modifiers::NONE)), LineEdit::Edited);
        assert_eq!(i.text(), "bc");
        i.handle(&press(Key::End, Modifiers::NONE));
        assert_eq!(i.caret(), 2);
    }

    #[test]
    fn word_delete_stops_at_path_separators() {
        let mut i = LineInput::with_text("~/ws/yaldabaoth/");
        assert_eq!(i.handle(&press(Key::Backspace, Modifiers::ALT)), LineEdit::Edited);
        assert_eq!(i.text(), "~/ws/");
        i.handle(&press(Key::Backspace, Modifiers::CONTROL));
        assert_eq!(i.text(), "~/");
    }

    #[test]
    fn word_motion_both_ways() {
        let mut i = LineInput::with_text("foo bar-baz");
        i.handle(&press(Key::Left, Modifiers::ALT));
        assert_eq!(i.with_caret('|'), "foo bar-|baz");
        i.handle(&press(Key::Left, Modifiers::ALT));
        assert_eq!(i.with_caret('|'), "foo |bar-baz");
        i.handle(&press(Key::Right, Modifiers::ALT));
        assert_eq!(i.with_caret('|'), "foo bar|-baz");
        i.handle(&press(Key::Delete, Modifiers::ALT));
        assert_eq!(i.text(), "foo bar");
    }

    #[test]
    fn kill_line_keys() {
        let mut i = LineInput::with_text("hello world");
        i.handle(&press(Key::Left, Modifiers::ALT));
        i.handle(&press(Key::Char('k'), Modifiers::CONTROL));
        assert_eq!(i.text(), "hello ");
        i.handle(&press(Key::Char('u'), Modifiers::CONTROL));
        assert!(i.is_empty());
        let mut j = LineInput::with_text("abc");
        j.handle(&press(Key::Backspace, Modifiers::PLATFORM));
        assert!(j.is_empty());
    }

    #[test]
    fn navigation_keys_are_left_to_the_caller() {
        let mut i = typed("x");
        for k in [Key::Enter, Key::Esc, Key::Up, Key::Down, Key::Tab, Key::PageUp] {
            assert_eq!(i.handle(&press(k, Modifiers::NONE)), LineEdit::Unhandled, "{k:?}");
        }
    }

    #[test]
    fn paste_flattens_newlines() {
        let mut i = LineInput::with_text("a");
        i.insert_str("b\r\nc\nd");
        assert_eq!(i.text(), "ab c d");
        assert_eq!(i.caret(), i.text().len());
    }
}
