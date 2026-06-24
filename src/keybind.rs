use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::keys::{Key, KeyPress, Modifiers};

const MULTI_KEY_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    FullPageDown,
    FullPageUp,
    JumpTop,
    JumpBottom,
    NextHeading,
    PrevHeading,
    NextHeadingSameLevel,
    PrevHeadingSameLevel,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    Quit,
    Save,
    SaveAs,
    ForceQuit,
    SaveQuit,
    ToggleView,
    OpenLink,
    YankLine,
    OpenMenu,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordForward,
    MoveWordBackward,
    MoveWordEnd,
    MoveLineStart,
    MoveLineFirstNonBlank,
    MoveLineEnd,
    FindCharForward,
    FindCharBackward,
    TillCharForward,
    TillCharBackward,
    InsertMode,
    InsertAfter,
    OpenLineBelow,
    OpenLineAbove,
    DeleteChar,
    DeleteLine,
    Undo,
    Redo,
    EnterCommand,
    OpenFileBrowser,
    FileBrowserDown,
    FileBrowserUp,
    FileBrowserEnter,
    FileBrowserParentDir,
    FileBrowserFilter,
    FileBrowserClose,
    Reload,
    Outline,
    NextBuffer,
    PrevBuffer,
    BufferList,
    CloseBuffer,
    NavCycle,
    NavCharacter,
    NavLinks,
    NavHeadings,
    NavListItems,
    NavCodeBlocks,
    NavActivate,
    OpenFileBrowserFull,
    SetHeading1,
    SetHeading2,
    SetHeading3,
    SetHeading4,
    SetHeading5,
    SetHeading6,
    ClearHeading,
    SetMaxLineWidth,
    // --- Helix-style selection actions ---
    DeleteSelection,
    ChangeSelection,
    YankSelection,
    CollapseSelection,
    FlipSelection,
    SelectAll,
    ExtendByLine,
    ToggleExtendMode,
    // --- Claude Code channel actions ---
    ClaudeAttach,
    ClaudeDetach,
    ClaudeSend,
    ClaudeSendSelection,
    ClaudeStatus,
    ClaudeTest,
    // --- Claude Code via ACP (Agent Client Protocol) actions ---
    /// Spawn an ACP agent subprocess (default `claude-agent-acp`) and connect.
    /// Argument (optional) is a shell command to spawn instead.
    ClaudeAcpAttach,
    /// Tear down the ACP agent (kills the subprocess).
    ClaudeAcpDetach,
    /// Send the active buffer / draft to the ACP agent as a prompt.
    ClaudeAcpSend,
    /// Send the current selection to the ACP agent as a prompt.
    ClaudeAcpSendSelection,
    /// Show the current ACP agent attachment.
    ClaudeAcpStatus,
    // --- Compose textbox (stable editing surface in *claude* buffer) ---
    /// Toggle the compose textbox open/closed.
    ComposeToggle,
    /// Send the compose textbox contents and close.
    ComposeSend,
    None,
}

pub struct KeySequenceMatcher {
    pending: Vec<KeyPress>,
    pending_since: Option<Instant>,
}

impl Default for KeySequenceMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl KeySequenceMatcher {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_since: None,
        }
    }

    /// Feed a key press. Returns Some(matched_sequence) if a match was found,
    /// None if still accumulating or no match.
    pub fn feed<V>(
        &mut self,
        press: KeyPress,
        single: &HashMap<KeyPress, V>,
        multi: &HashMap<Vec<KeyPress>, V>,
    ) -> Option<Vec<KeyPress>> {
        // Check timeout, clear if expired
        if let Some(since) = self.pending_since
            && since.elapsed() > MULTI_KEY_TIMEOUT
        {
            self.pending.clear();
            self.pending_since = None;
        }

        self.pending.push(press.clone());
        self.pending_since = Some(Instant::now());

        // Check multi-key match
        if multi.contains_key(&self.pending) {
            let matched = self.pending.clone();
            self.pending.clear();
            self.pending_since = None;
            return Some(matched);
        }

        // Check if prefix of any multi-key sequence
        let is_prefix = multi
            .keys()
            .any(|seq| seq.len() > self.pending.len() && seq.starts_with(&self.pending));

        if is_prefix {
            return Option::None;
        }

        // No multi-key match or prefix — check single
        self.pending.clear();
        self.pending_since = None;
        if single.contains_key(&press) {
            Some(vec![press])
        } else {
            Option::None
        }
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_since = None;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

pub struct KeybindManager {
    single: HashMap<KeyPress, String>,
    multi: HashMap<Vec<KeyPress>, String>,
    matcher: KeySequenceMatcher,
    /// Numeric count prefix accumulated ahead of a motion/action (vim-style:
    /// `42g` jumps to line 42). `None` until the first count digit is typed.
    /// Read-and-cleared by [`take_count`](Self::take_count) once an action
    /// resolves.
    pending_count: Option<usize>,
}

impl KeybindManager {
    pub fn new(single: HashMap<KeyPress, String>, multi: HashMap<Vec<KeyPress>, String>) -> Self {
        Self {
            single,
            multi,
            matcher: KeySequenceMatcher::new(),
            pending_count: None,
        }
    }

    pub fn process_key(&mut self, press: KeyPress) -> Option<String> {
        // Count-prefix accumulation: a bare digit extends the pending count.
        // `0` only counts as a digit once a count is already in progress —
        // a leading `0` stays bound to `move-line-start` (vim semantics).
        if press.modifiers.is_empty()
            && let Key::Char(c) = press.key
            && let Some(d) = c.to_digit(10)
            && !(d == 0 && self.pending_count.is_none())
        {
            let acc = self.pending_count.unwrap_or(0);
            // Saturate rather than overflow on absurd counts.
            self.pending_count = Some(acc.saturating_mul(10).saturating_add(d as usize));
            return None;
        }

        let matched = self.matcher.feed(press, &self.single, &self.multi);
        let matched = match matched {
            Some(m) => m,
            None => return None,
        };
        if matched.len() == 1 {
            self.single.get(&matched[0]).cloned()
        } else {
            self.multi.get(&matched).cloned()
        }
    }

    /// Take the accumulated numeric count prefix, clearing it. Returns `None`
    /// if no digits were typed before the resolved action.
    pub fn take_count(&mut self) -> Option<usize> {
        self.pending_count.take()
    }

    pub fn reset_pending(&mut self) {
        self.matcher.reset();
        self.pending_count = None;
    }

    pub fn has_pending(&self) -> bool {
        self.matcher.has_pending()
    }

    pub fn apply_bindings(&mut self, bindings: &[(Vec<KeyPress>, String)]) {
        for (key_seq, cmd) in bindings {
            if key_seq.len() == 1 {
                self.single.insert(key_seq[0].clone(), cmd.clone());
            } else {
                self.multi.insert(key_seq.clone(), cmd.clone());
            }
        }
    }
}

impl Default for KeybindManager {
    fn default() -> Self {
        let mut single = HashMap::new();
        let mut multi = HashMap::new();

        single.insert(key('h'), "move-left".into());
        single.insert(key('j'), "move-down".into());
        single.insert(key('k'), "move-up".into());
        single.insert(key('l'), "move-right".into());
        single.insert(key('w'), "move-word-forward".into());
        single.insert(key('b'), "move-word-backward".into());
        single.insert(key('e'), "move-word-end".into());
        single.insert(key('f'), "find-char-forward".into());
        single.insert(key('F'), "find-char-backward".into());
        single.insert(key('t'), "till-char-forward".into());
        single.insert(key('T'), "till-char-backward".into());
        single.insert(key('0'), "move-line-start".into());
        single.insert(key('^'), "move-line-first-non-blank".into());
        single.insert(key('$'), "move-line-end".into());
        single.insert(key('i'), "insert-mode".into());
        single.insert(key('a'), "insert-after".into());
        single.insert(key('o'), "open-line-below".into());
        single.insert(key('O'), "open-line-above".into());
        // Helix-style selection bindings
        single.insert(key('d'), "delete-selection".into());
        single.insert(key('c'), "change-selection".into());
        single.insert(key('x'), "extend-line".into());
        single.insert(key(';'), "collapse-selection".into());
        single.insert(key(','), "flip-selection".into());
        single.insert(key('%'), "select-all".into());
        single.insert(key('v'), "toggle-extend-mode".into());
        single.insert(key('u'), "undo".into());
        single.insert(key('p'), "paste".into());
        single.insert(key('P'), "paste-before".into());
        single.insert(key(':'), "enter-command".into());
        single.insert(ctrl('r'), "redo".into());
        single.insert(ctrl('d'), "half-page-down".into());
        single.insert(ctrl('u'), "half-page-up".into());
        single.insert(ctrl('f'), "full-page-down".into());
        single.insert(ctrl('b'), "full-page-up".into());
        single.insert(key('G'), "goto-bottom".into());
        single.insert(key('}'), "goto-heading".into());
        single.insert(key('{'), "prev-heading".into());
        single.insert(key('/'), "search-forward".into());
        single.insert(key('?'), "search-backward".into());
        single.insert(key('n'), "search-next".into());
        single.insert(key('N'), "search-prev".into());
        single.insert(key('y'), "yank-selection".into());
        single.insert(key(' '), "open-menu".into());
        single.insert(key('m'), "nav-cycle".into());
        single.insert(
            KeyPress::new(Key::Enter, Modifiers::NONE),
            "nav-activate".into(),
        );

        single.insert(
            KeyPress::new(Key::Tab, Modifiers::NONE),
            "next-buffer".into(),
        );
        single.insert(
            KeyPress::new(Key::BackTab, Modifiers::NONE),
            "prev-buffer".into(),
        );

        multi.insert(vec![key('g'), key('g')], "goto-top".into());
        multi.insert(vec![key('g'), key('x')], "open-link".into());
        // Helix-style goto bindings on g
        multi.insert(vec![key('g'), key('h')], "move-line-start".into());
        multi.insert(vec![key('g'), key('l')], "move-line-end".into());
        multi.insert(vec![key('g'), key('e')], "goto-bottom".into());
        multi.insert(vec![key(']'), key(']')], "next-heading-same-level".into());
        multi.insert(vec![key('['), key('[')], "prev-heading-same-level".into());

        // Heading level shortcuts (Alt+N)
        let alt = |c: char| KeyPress::new(Key::Char(c), Modifiers::ALT);
        single.insert(alt('1'), "set-heading-1".into());
        single.insert(alt('2'), "set-heading-2".into());
        single.insert(alt('3'), "set-heading-3".into());
        single.insert(alt('4'), "set-heading-4".into());
        single.insert(alt('5'), "set-heading-5".into());
        single.insert(alt('6'), "set-heading-6".into());
        single.insert(alt('0'), "clear-heading".into());

        // Compose textbox
        single.insert(ctrl('t'), "compose".into());

        Self::new(single, multi)
    }
}

fn key(c: char) -> KeyPress {
    KeyPress::new(Key::Char(c), Modifiers::NONE)
}

fn ctrl(c: char) -> KeyPress {
    KeyPress::new(Key::Char(c), Modifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char) -> KeyPress {
        KeyPress::new(Key::Char(c), Modifiers::NONE)
    }

    #[test]
    fn count_prefix_accumulates_multi_digit() {
        let mut km = KeybindManager::default();
        // Digits accumulate and do not resolve to an action.
        assert_eq!(km.process_key(k('4')), None);
        assert_eq!(km.process_key(k('2')), None);
        // The next non-digit resolves; the count is available alongside it.
        assert_eq!(km.process_key(k('G')).as_deref(), Some("goto-bottom"));
        assert_eq!(km.take_count(), Some(42));
        // Count is cleared after being taken.
        assert_eq!(km.take_count(), None);
    }

    #[test]
    fn leading_zero_is_line_start_not_count() {
        let mut km = KeybindManager::default();
        // A bare leading `0` keeps its motion binding rather than starting a count.
        assert_eq!(km.process_key(k('0')).as_deref(), Some("move-line-start"));
        assert_eq!(km.take_count(), None);
    }

    #[test]
    fn zero_extends_an_in_progress_count() {
        let mut km = KeybindManager::default();
        assert_eq!(km.process_key(k('1')), None);
        assert_eq!(km.process_key(k('0')), None);
        assert_eq!(km.process_key(k('G')).as_deref(), Some("goto-bottom"));
        assert_eq!(km.take_count(), Some(10));
    }

    #[test]
    fn no_count_means_none() {
        let mut km = KeybindManager::default();
        assert_eq!(km.process_key(k('G')).as_deref(), Some("goto-bottom"));
        assert_eq!(km.take_count(), None);
    }

    #[test]
    fn reset_pending_clears_count() {
        let mut km = KeybindManager::default();
        assert_eq!(km.process_key(k('9')), None);
        km.reset_pending();
        assert_eq!(km.take_count(), None);
    }

    #[test]
    fn paste_bindings_resolve() {
        let mut km = KeybindManager::default();
        assert_eq!(km.process_key(k('p')).as_deref(), Some("paste"));
        assert_eq!(km.process_key(k('P')).as_deref(), Some("paste-before"));
    }
}
