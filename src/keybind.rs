use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
    OpenLink,
    YankLine,
    OpenMenu,
    OpenFileBrowser,
    FileBrowserDown,
    FileBrowserUp,
    FileBrowserEnter,
    FileBrowserParentDir,
    FileBrowserFilter,
    FileBrowserClose,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct KeyPress {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl From<KeyEvent> for KeyPress {
    fn from(event: KeyEvent) -> Self {
        Self {
            code: event.code,
            modifiers: event.modifiers & !KeyModifiers::SHIFT,
        }
    }
}

pub struct KeybindManager {
    single: HashMap<KeyPress, Action>,
    multi: HashMap<Vec<KeyPress>, Action>,
    pending: Vec<KeyPress>,
    pending_since: Option<Instant>,
}

impl KeybindManager {
    pub(crate) fn new(
        single: HashMap<KeyPress, Action>,
        multi: HashMap<Vec<KeyPress>, Action>,
    ) -> Self {
        Self {
            single,
            multi,
            pending: Vec::new(),
            pending_since: None,
        }
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<Action> {
        if let Some(since) = self.pending_since
            && since.elapsed() > MULTI_KEY_TIMEOUT
        {
            self.pending.clear();
            self.pending_since = None;
        }

        let press: KeyPress = event.into();
        self.pending.push(press.clone());
        self.pending_since = Some(Instant::now());

        if let Some(&action) = self.multi.get(&self.pending) {
            self.pending.clear();
            self.pending_since = None;
            return Some(action);
        }

        let is_prefix = self
            .multi
            .keys()
            .any(|seq| seq.len() > self.pending.len() && seq.starts_with(&self.pending));

        if is_prefix {
            return None;
        }

        self.pending.clear();
        self.pending_since = None;
        self.single.get(&press).copied()
    }

    pub fn reset_pending(&mut self) {
        self.pending.clear();
        self.pending_since = None;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Default for KeybindManager {
    fn default() -> Self {
        let mut single = HashMap::new();
        let mut multi = HashMap::new();

        single.insert(key('j'), Action::ScrollDown);
        single.insert(key('k'), Action::ScrollUp);
        single.insert(ctrl('d'), Action::HalfPageDown);
        single.insert(ctrl('u'), Action::HalfPageUp);
        single.insert(ctrl('f'), Action::FullPageDown);
        single.insert(ctrl('b'), Action::FullPageUp);
        single.insert(key('G'), Action::JumpBottom);
        single.insert(key('}'), Action::NextHeading);
        single.insert(key('{'), Action::PrevHeading);
        single.insert(key('/'), Action::SearchForward);
        single.insert(key('?'), Action::SearchBackward);
        single.insert(key('n'), Action::SearchNext);
        single.insert(key('N'), Action::SearchPrev);
        single.insert(key('q'), Action::Quit);
        single.insert(key('o'), Action::OpenLink);
        single.insert(key('y'), Action::YankLine);
        single.insert(key(' '), Action::OpenMenu);

        multi.insert(vec![key('g'), key('g')], Action::JumpTop);
        multi.insert(vec![key(']'), key(']')], Action::NextHeadingSameLevel);
        multi.insert(vec![key('['), key('[')], Action::PrevHeadingSameLevel);

        Self::new(single, multi)
    }
}

fn key(c: char) -> KeyPress {
    KeyPress {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::NONE,
    }
}

fn ctrl(c: char) -> KeyPress {
    KeyPress {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::CONTROL,
    }
}
