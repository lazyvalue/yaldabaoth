//! Slash-command parsing for the external chat bridge (spec §4).
//!
//! A pure `(text) → Command` mapper — no transport, no driver, no I/O — so the
//! whole control-plane grammar is table-testable. Dispatch (which command is
//! meaningful in which topic) lives in [`super::handle_inbound`]; this module
//! only decides *what the user typed*.
//!
//! Grammar (verb + mode word are case-insensitive, whitespace-tolerant):
//! - `/new <label…>` — start a session. An optional working directory may be
//!   given either as `--cwd <path>` or as a trailing absolute path
//!   (`/new fix the bug /Users/me/proj`). We keep it deliberately simple: the
//!   `--cwd` form is explicit; otherwise a *trailing* token that looks like an
//!   absolute path (starts with `/`) is taken as the cwd and the rest is the
//!   label. Anything else is all label.
//! - `/sessions` · `/stop` · `/status`
//! - `/mode <read-only|auto-edit|yolo|ask>` (aliases per `PermissionMode::parse`)
//! - a bare `/verb` we don't know → [`Command::Unknown`]
//! - text not starting with `/` → [`Command::Message`] (a turn to inject)

use std::path::PathBuf;

use yalda::acp_channel::PermissionMode;

/// A parsed control-plane command. Locale (which are honored where) is decided
/// by the caller; this is purely the surface grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `/new <label> [--cwd <path> | trailing /abs/path]`.
    New {
        label: String,
        cwd: Option<PathBuf>,
    },
    /// `/sessions` — list the roster + topic mapping.
    Sessions,
    /// `/stop` — cancel the in-topic session's turn.
    Stop,
    /// `/mode <m>` — set the in-topic session's permission mode.
    Mode(PermissionMode),
    /// `/status` — report the in-topic session's label/mode/state.
    Status,
    /// Plain text (no leading `/`) — inject as a prompt in a session topic.
    Message(String),
    /// A `/verb` we don't recognize (carries the original trimmed text).
    Unknown(String),
}

/// Parse one inbound message body into a [`Command`]. Total (never fails):
/// unrecognized `/verbs` and bad `/mode` words fall through to
/// [`Command::Unknown`]; non-slash text is a [`Command::Message`].
pub fn parse(text: &str) -> Command {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return Command::Message(trimmed.to_string());
    }

    // Safe: `trimmed` starts with '/', so there's at least one token.
    let verb = trimmed.split_whitespace().next().unwrap_or("");
    match verb.to_lowercase().as_str() {
        "/new" => parse_new(&trimmed[verb.len()..]),
        "/sessions" => Command::Sessions,
        "/stop" => Command::Stop,
        "/status" => Command::Status,
        "/mode" => match trimmed.split_whitespace().nth(1) {
            Some(word) => match PermissionMode::parse(word) {
                Some(mode) => Command::Mode(mode),
                None => Command::Unknown(trimmed.to_string()),
            },
            None => Command::Unknown(trimmed.to_string()),
        },
        _ => Command::Unknown(trimmed.to_string()),
    }
}

/// Parse the argument tail of `/new` (everything after the verb).
fn parse_new(rest: &str) -> Command {
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut cwd = None;

    if let Some(pos) = tokens.iter().position(|t| *t == "--cwd") {
        // Explicit `--cwd <path>`: the label is everything before the flag.
        if let Some(path) = tokens.get(pos + 1) {
            cwd = Some(PathBuf::from(*path));
        }
        tokens.truncate(pos);
    } else if let Some(last) = tokens.last() {
        // Otherwise a trailing absolute path is taken as the cwd.
        if last.starts_with('/') {
            cwd = Some(PathBuf::from(*last));
            tokens.pop();
        }
    }

    Command::New {
        label: tokens.join(" "),
        cwd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_a_message_trimmed() {
        assert_eq!(parse("  hello there  "), Command::Message("hello there".into()));
        assert_eq!(parse("do the thing"), Command::Message("do the thing".into()));
    }

    #[test]
    fn empty_is_an_empty_message() {
        assert_eq!(parse("   "), Command::Message(String::new()));
    }

    #[test]
    fn sessions_stop_status_verbs() {
        assert_eq!(parse("/sessions"), Command::Sessions);
        assert_eq!(parse("  /stop  "), Command::Stop);
        assert_eq!(parse("/status"), Command::Status);
    }

    #[test]
    fn verb_is_case_insensitive() {
        assert_eq!(parse("/STOP"), Command::Stop);
        assert_eq!(parse("/Sessions"), Command::Sessions);
    }

    #[test]
    fn new_label_only() {
        assert_eq!(
            parse("/new Build the feature"),
            Command::New { label: "Build the feature".into(), cwd: None }
        );
    }

    #[test]
    fn new_bare_has_empty_label() {
        assert_eq!(parse("/new"), Command::New { label: String::new(), cwd: None });
        assert_eq!(parse("/new   "), Command::New { label: String::new(), cwd: None });
    }

    #[test]
    fn new_trailing_absolute_path_is_cwd() {
        assert_eq!(
            parse("/new fix bug /Users/me/proj"),
            Command::New {
                label: "fix bug".into(),
                cwd: Some(PathBuf::from("/Users/me/proj")),
            }
        );
    }

    #[test]
    fn new_explicit_cwd_flag() {
        assert_eq!(
            parse("/new fix bug --cwd /srv/app"),
            Command::New {
                label: "fix bug".into(),
                cwd: Some(PathBuf::from("/srv/app")),
            }
        );
    }

    #[test]
    fn new_relative_trailing_token_is_label_not_cwd() {
        // A trailing token that is NOT absolute stays part of the label.
        assert_eq!(
            parse("/new tidy up docs"),
            Command::New { label: "tidy up docs".into(), cwd: None }
        );
    }

    #[test]
    fn mode_words_map_to_permission_modes() {
        assert_eq!(parse("/mode read-only"), Command::Mode(PermissionMode::ReadOnly));
        assert_eq!(parse("/mode auto-edit"), Command::Mode(PermissionMode::AutoEdit));
        assert_eq!(parse("/mode yolo"), Command::Mode(PermissionMode::Yolo));
        assert_eq!(parse("/mode ask"), Command::Mode(PermissionMode::AskEachTime));
    }

    #[test]
    fn mode_word_is_case_insensitive() {
        assert_eq!(parse("/MODE Yolo"), Command::Mode(PermissionMode::Yolo));
    }

    #[test]
    fn bad_mode_word_is_unknown() {
        assert_eq!(parse("/mode wat"), Command::Unknown("/mode wat".into()));
        assert_eq!(parse("/mode"), Command::Unknown("/mode".into()));
    }

    #[test]
    fn unknown_verb() {
        assert_eq!(parse("/frobnicate now"), Command::Unknown("/frobnicate now".into()));
        assert_eq!(parse("/x"), Command::Unknown("/x".into()));
    }
}
