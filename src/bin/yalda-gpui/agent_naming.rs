//! Session autonaming (`UXI-AgentTile-27`).
//!
//! When a session's FIRST agent turn completes, the opening exchange is sent to
//! a cheap model (Haiku) which returns a two-to-three-word name and a
//! two-sentence summary. The name replaces the placeholder `claude-N` label
//! everywhere the session is listed; the summary renders under it in the jump
//! panel.
//!
//! Everything in this module except [`request_session_name`] is **pure** — the
//! prompt builder, the reply parser, and the two sanitizers are unit-testable
//! without a network or an API key, which is where the real risk lives (a model
//! that ignores the format instruction must never produce a garbage label).
//!
//! This deliberately does NOT reuse the recap facet's throwaway ACP subprocess
//! (`UXI-AgentTile-15`): a multi-paragraph recap earns a whole agent
//! subprocess, two words and two sentences do not.

/// The model that derives the name. Haiku is the cheapest current model and the
/// task is trivial — see `docs/components/agent-tile/naming.md`.
const NAMING_MODEL: &str = "claude-haiku-4-5";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// How much of the opening exchange to send. The naming call only ever sees the
/// HEAD of the transcript (the first turn), so a small budget is plenty and
/// keeps the call cheap.
const NAMING_TRANSCRIPT_BUDGET: usize = 4000;

/// Hard caps on what we will install, whatever the model returns.
pub(crate) const MAX_NAME_CHARS: usize = 28;
pub(crate) const MAX_SUMMARY_CHARS: usize = 240;

/// Where a session's `label` came from.
///
/// This exists because once autonames are a thing, "is this label
/// auto-generated?" stops being answerable by looking at the string —
/// `payments refactor` is indistinguishable from a name the user typed. The
/// legacy `is_auto_claude_label` regex could only recognise `claude-N`, so
/// keeping it as the gate would let autonaming overwrite real user names (the
/// bug-0016 class). `User` is a **latch**: once set it is never unset.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NameOrigin {
    /// Placeholder (`claude-N`) or an installed autoname. Autonaming may fire.
    #[default]
    Auto,
    /// The user renamed this session. Autonaming may NEVER fire, and an
    /// in-flight autoname result that lands afterwards is dropped.
    User,
}

/// One-shot lifecycle of a session's autoname (`UXI-AgentTile-27` property 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum AutonameState {
    /// Fresh session, first turn not yet finished — autoname is still owed.
    #[default]
    Pending,
    /// The worker is in flight (or the flag has been picked up). Never re-armed.
    Requested,
    /// Terminal: a name landed, the call failed, or the session was never
    /// eligible (restored from a previous launch — property 1's "no retro-name").
    Done,
}

/// The model's answer, post-sanitization. Either field may be `None` when the
/// model returned something unusable for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionNaming {
    pub(crate) name: Option<String>,
    pub(crate) summary: Option<String>,
}

impl SessionNaming {
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none() && self.summary.is_none()
    }
}

/// Trim the opening exchange to the naming budget. Unlike the recap prompt
/// (which keeps the TAIL — the recent state), naming keeps the **head**: the
/// first thing the user said is what the session is about.
pub(crate) fn trim_opening(transcript: &str) -> String {
    if transcript.len() <= NAMING_TRANSCRIPT_BUDGET {
        return transcript.to_string();
    }
    // Back off to the previous line boundary so we don't cut mid-line.
    let end = transcript[..NAMING_TRANSCRIPT_BUDGET]
        .rfind('\n')
        .unwrap_or(NAMING_TRANSCRIPT_BUDGET);
    format!("{}\n…(rest of conversation elided)…", &transcript[..end])
}

/// The system prompt. Kept separate from the conversation text so the model
/// can't be talked out of the output contract by the transcript itself.
pub(crate) fn naming_system_prompt() -> String {
    "You name coding-assistant sessions. Given the opening of a conversation, \
     reply with ONLY a JSON object of exactly two string fields:\n\
     {\"name\": \"...\", \"summary\": \"...\"}\n\
     - \"name\": 2-3 lowercase words, space separated, at most 28 characters, \
     naming the concrete thing being worked on (e.g. \"payments refactor\", \
     \"flaky test hunt\"). No punctuation, no quotes, no file extensions.\n\
     - \"summary\": at most two short sentences on what this session is about.\n\
     Output the JSON object and nothing else — no preamble, no code fence."
        .to_string()
}

/// The user-turn content: the opening exchange, fenced so the model can tell
/// the conversation apart from its instructions.
pub(crate) fn build_naming_prompt(transcript: &str) -> String {
    format!(
        "<conversation>\n{}\n</conversation>",
        trim_opening(transcript)
    )
}

/// Strip one layer of markdown code fence, if the model wrapped its JSON.
fn strip_code_fence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    // Drop the info string (```json) up to the first newline.
    let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

/// Reduce whatever the model said into a well-formed label, or `None`.
///
/// Enforces `UXI-AgentTile-27` property 2 CLIENT-side, so a model that ignores
/// the format instruction can never install a garbage name: lowercased,
/// punctuation and quotes stripped, whitespace collapsed, at most 3 words, at
/// most [`MAX_NAME_CHARS`] characters (truncated on a word boundary).
pub(crate) fn sanitize_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '+' || c == '.' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let mut words: Vec<String> = cleaned
        .split_whitespace()
        .map(|w| w.trim_matches(['-', '.']).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    words.truncate(3);
    if words.is_empty() {
        return None;
    }
    // Drop trailing words until the whole thing fits the cap; a single
    // over-long first word is hard-truncated rather than dropped entirely.
    while words.len() > 1 && words.join(" ").chars().count() > MAX_NAME_CHARS {
        words.pop();
    }
    let mut name = words.join(" ");
    if name.chars().count() > MAX_NAME_CHARS {
        name = name.chars().take(MAX_NAME_CHARS).collect();
        name = name.trim_end().to_string();
    }
    if name.is_empty() { None } else { Some(name) }
}

/// Reduce the model's summary to at most two sentences and
/// [`MAX_SUMMARY_CHARS`] characters. Newlines are collapsed — the jump panel
/// renders this as a single small italic line.
pub(crate) fn sanitize_summary(raw: &str) -> Option<String> {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    // Keep at most two sentences, counting a terminator as part of its sentence.
    let mut kept = String::new();
    let mut sentences = 0;
    for ch in flat.chars() {
        kept.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            sentences += 1;
            if sentences == 2 {
                break;
            }
        }
    }
    let mut summary = kept.trim().to_string();
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        summary = summary.chars().take(MAX_SUMMARY_CHARS).collect();
        // Back off to a word boundary so we don't cut mid-word.
        if let Some(sp) = summary.rfind(' ') {
            summary.truncate(sp);
        }
        summary.push('…');
    }
    if summary.is_empty() { None } else { Some(summary) }
}

/// Parse the model's reply text into a [`SessionNaming`].
///
/// Tolerant by design: the happy path is a bare JSON object, but a model that
/// wraps it in a code fence or bolts on a preamble still parses (we scan for
/// the first `{`…`}` span). A reply with no usable JSON degrades to treating
/// the whole first line as the name rather than failing outright.
pub(crate) fn parse_naming_reply(reply: &str) -> SessionNaming {
    let text = strip_code_fence(reply);
    let json_span = text
        .find('{')
        .and_then(|start| text.rfind('}').map(|end| (start, end)))
        .filter(|(start, end)| end > start)
        .map(|(start, end)| &text[start..=end]);

    if let Some(span) = json_span
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(span)
    {
        return SessionNaming {
            name: v.get("name").and_then(|n| n.as_str()).and_then(sanitize_name),
            summary: v
                .get("summary")
                .and_then(|s| s.as_str())
                .and_then(sanitize_summary),
        };
    }

    // No JSON at all: salvage the first line as a name, the rest as a summary.
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let rest: String = lines.collect::<Vec<_>>().join(" ");
    SessionNaming {
        name: sanitize_name(first),
        summary: sanitize_summary(&rest),
    }
}

/// Read `ANTHROPIC_API_KEY` from the environment. `None` (rather than an error)
/// when unset — autonaming fails silently by design (`UXI-AgentTile-27`
/// property 4).
pub(crate) fn naming_api_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// BLOCKING one-shot Haiku call. Runs on the background executor, never on the
/// paint thread. Returns `Err` with a human-readable reason on any failure; the
/// caller logs it and leaves the session named `claude-N`.
///
/// Rust has no official Anthropic SDK, so this is the documented raw-HTTP shape
/// (`x-api-key` + `anthropic-version`), built on the `ureq` client the Linear
/// app and the Telegram bridge already use.
pub(crate) fn request_session_name(api_key: &str, transcript: &str) -> Result<SessionNaming, String> {
    let body = serde_json::json!({
        "model": NAMING_MODEL,
        "max_tokens": 200,
        "system": naming_system_prompt(),
        "messages": [{ "role": "user", "content": build_naming_prompt(transcript) }],
    });
    let value: serde_json::Value = match ureq::post(MESSAGES_URL)
        .set("content-type", "application/json")
        .set("x-api-key", api_key)
        .set("anthropic-version", ANTHROPIC_VERSION)
        .send_json(body)
    {
        Ok(r) => r
            .into_json()
            .map_err(|e| format!("decoding naming response failed: {e}"))?,
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            let hint = if code == 401 {
                " (is ANTHROPIC_API_KEY valid?)"
            } else {
                ""
            };
            return Err(format!(
                "naming API HTTP {code}{hint}: {}",
                detail.trim()
            ));
        }
        Err(e) => return Err(format!("naming request failed: {e}")),
    };

    // A safety refusal is a normal 200 with `stop_reason: "refusal"` and empty
    // content — treat it as "no name", not as a crash.
    if value.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
        return Err("naming refused".into());
    }
    let text = value
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err("naming reply was empty".into());
    }
    let naming = parse_naming_reply(&text);
    if naming.is_empty() {
        return Err("naming reply was unusable".into());
    }
    Ok(naming)
}
