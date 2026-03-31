use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::keys::KeyPress;

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
    MoveLineEnd,
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

    /// Feed a key event. Returns Some(matched_sequence) if a match was found,
    /// None if still accumulating or no match.
    pub fn feed<V>(
        &mut self,
        event: KeyEvent,
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

        let press = KeyPress::from_event(event);
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
}

impl KeybindManager {
    pub fn new(
        single: HashMap<KeyPress, String>,
        multi: HashMap<Vec<KeyPress>, String>,
    ) -> Self {
        Self {
            single,
            multi,
            matcher: KeySequenceMatcher::new(),
        }
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<String> {
        let matched = self.matcher.feed(event, &self.single, &self.multi)?;
        if matched.len() == 1 {
            self.single.get(&matched[0]).cloned()
        } else {
            self.multi.get(&matched).cloned()
        }
    }

    pub fn reset_pending(&mut self) {
        self.matcher.reset();
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
        single.insert(key('0'), "move-line-start".into());
        single.insert(key('$'), "move-line-end".into());
        single.insert(key('i'), "insert-mode".into());
        single.insert(key('a'), "insert-after".into());
        single.insert(key('o'), "open-line-below".into());
        single.insert(key('O'), "open-line-above".into());
        single.insert(key('x'), "delete-char".into());
        single.insert(key('u'), "undo".into());
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
        single.insert(key('y'), "yank-line".into());
        single.insert(key(' '), "open-menu".into());
        single.insert(key('m'), "nav-cycle".into());
        single.insert(
            KeyPress::new(KeyCode::Enter, KeyModifiers::NONE),
            "nav-activate".into(),
        );

        single.insert(
            KeyPress::new(KeyCode::Tab, KeyModifiers::NONE),
            "next-buffer".into(),
        );
        single.insert(
            KeyPress::new(KeyCode::BackTab, KeyModifiers::NONE),
            "prev-buffer".into(),
        );

        multi.insert(vec![key('g'), key('g')], "goto-top".into());
        multi.insert(vec![key('g'), key('x')], "open-link".into());
        multi.insert(vec![key('d'), key('d')], "delete-line".into());
        multi.insert(vec![key(']'), key(']')], "next-heading-same-level".into());
        multi.insert(vec![key('['), key('[')], "prev-heading-same-level".into());
        multi.insert(vec![key('g'), key('l')], "nav-links".into());
        multi.insert(vec![key('g'), key('h')], "nav-headings".into());
        multi.insert(vec![key('g'), key('i')], "nav-list-items".into());
        multi.insert(vec![key('g'), key('c')], "nav-code-blocks".into());

        Self::new(single, multi)
    }
}

fn key(c: char) -> KeyPress {
    KeyPress::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyPress {
    KeyPress::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
