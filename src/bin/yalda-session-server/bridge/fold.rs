//! Per-session outbound event fold (spec-external-chat-bridge.md §5).
//!
//! [`EventFolder`] is a **pure, per-session stateful** reducer over a session's
//! canonical [`AgentEventKind`] stream. It coalesces streamed assistant prose
//! into a single live-edited chat message, appends compact status lines for tool
//! calls, and finalizes at the turn boundary — emitting transport-agnostic
//! [`ChatOp`]s the bridge loop turns into `send`/`edit` calls on the session's
//! topic. Keeping it pure (no transport, no async) makes the coalescing fully
//! unit-testable.
//!
//! Only [`AgentEventKind`] facts drive the fold — the caller feeds
//! `Notification::Agent { event }.kind` and IGNORES the legacy
//! `ReplyEvent`/`TurnEnded`/`UserPrompt` `Notification` variants, so the same
//! turn is never rendered twice.

use yalda::acp_channel::{ToolCall, ToolKind};
use yalda::agent_event::{AgentEventKind, ChunkRole};

/// A transport-agnostic instruction the bridge loop executes against a topic.
///
/// - [`ChatOp::Post`] — create the running message with this FULL text.
/// - [`ChatOp::Edit`] — set the running message to this FULL text.
/// - [`ChatOp::Finalize`] — stop editing; the turn is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatOp {
    Post(String),
    Edit(String),
    Finalize,
}

/// Per-session fold state. One per bound session; reset at each turn boundary so
/// a new turn starts a fresh [`ChatOp::Post`] (a new chat message) rather than
/// editing the previous turn's message.
#[derive(Debug, Default)]
pub struct EventFolder {
    /// Coalesced assistant prose for the current turn (assistant `Chunk`s only).
    prose: String,
    /// Compact one-line tool statuses for the current turn.
    tools: Vec<String>,
    /// Whether the running message has been Posted this turn — the first content
    /// Posts, everything after Edits.
    posted: bool,
    /// The last text rendered, so a no-op repaint is suppressed (Telegram
    /// rejects an edit to identical text).
    last_render: String,
}

impl EventFolder {
    /// Fold one canonical agent fact, returning the chat ops it implies (often
    /// empty). Non-content kinds are ignored (spec §5 keeps the mirror minimal).
    pub fn on_event(&mut self, kind: &AgentEventKind) -> Vec<ChatOp> {
        match kind {
            // Assistant prose accumulates; thoughts are not mirrored.
            AgentEventKind::Chunk {
                text,
                role: ChunkRole::Message,
            } => {
                self.prose.push_str(text);
                self.emit_content()
            }
            // A tool call becomes a compact status line under the prose.
            AgentEventKind::ToolCallStarted(tc) => {
                self.tools.push(tool_line(tc));
                self.emit_content()
            }
            // The turn boundary flushes any final change and closes the message.
            AgentEventKind::TurnEnded { .. } => self.finalize_turn(),
            _ => Vec::new(),
        }
    }

    /// Close out the current turn: emit a final [`ChatOp::Edit`] if the content
    /// changed since the last op, then [`ChatOp::Finalize`], then reset per-turn
    /// state so the next turn opens a fresh [`ChatOp::Post`].
    pub fn finalize_turn(&mut self) -> Vec<ChatOp> {
        let mut ops = Vec::new();
        let render = self.render();
        if self.posted && !render.is_empty() && render != self.last_render {
            self.last_render = render.clone();
            ops.push(ChatOp::Edit(render));
        }
        ops.push(ChatOp::Finalize);
        self.prose.clear();
        self.tools.clear();
        self.posted = false;
        self.last_render.clear();
        ops
    }

    /// Emit the op for freshly-changed content: the first non-empty content
    /// Posts the running message; subsequent content Edits it. A render equal to
    /// the last one (or empty) yields nothing, so no redundant transport call.
    fn emit_content(&mut self) -> Vec<ChatOp> {
        let render = self.render();
        if render.is_empty() || render == self.last_render {
            return Vec::new();
        }
        self.last_render = render.clone();
        if self.posted {
            vec![ChatOp::Edit(render)]
        } else {
            self.posted = true;
            vec![ChatOp::Post(render)]
        }
    }

    /// Render = the prose paragraph, then (blank line) the tool status lines.
    fn render(&self) -> String {
        let prose = self.prose.trim();
        let mut out = String::new();
        if !prose.is_empty() {
            out.push_str(prose);
        }
        if !self.tools.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&self.tools.join("\n"));
        }
        out
    }
}

/// A compact `🔧 <verb> <arg>` status for a tool call: the verb is derived from
/// the ACP tool kind, the arg from the human-readable title.
fn tool_line(tc: &ToolCall) -> String {
    let verb = tool_verb(&tc.kind);
    let arg = tc.title.trim();
    if arg.is_empty() {
        format!("🔧 {verb}")
    } else {
        format!("🔧 {verb} {arg}")
    }
}

/// Short verb for a tool kind (mirrors the GUI transcript's labels).
fn tool_verb(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Search => "search",
        ToolKind::Execute => "run",
        ToolKind::Move => "move",
        ToolKind::Delete => "delete",
        ToolKind::Fetch => "fetch",
        ToolKind::Think => "think",
        ToolKind::SwitchMode => "mode",
        _ => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yalda::agent_event::TurnOutcome;

    fn chunk(text: &str) -> AgentEventKind {
        AgentEventKind::Chunk {
            text: text.to_string(),
            role: ChunkRole::Message,
        }
    }

    fn read_tool(title: &str) -> AgentEventKind {
        let mut tc = ToolCall::new("t1", title);
        tc.kind = ToolKind::Read;
        AgentEventKind::ToolCallStarted(tc)
    }

    /// The canonical shape: two prose chunks, a tool call, then the turn end —
    /// Post first, then Edits, then Finalize; the last render carries BOTH the
    /// coalesced prose and the tool line.
    #[test]
    fn two_chunks_a_tool_then_turn_end_fold_to_post_edits_finalize() {
        let mut f = EventFolder::default();
        let mut ops = Vec::new();
        ops.extend(f.on_event(&chunk("Hello ")));
        ops.extend(f.on_event(&chunk("world")));
        ops.extend(f.on_event(&read_tool("Read File")));
        ops.extend(f.on_event(&AgentEventKind::TurnEnded {
            outcome: TurnOutcome::Completed,
        }));

        // Exactly one Post, and it is the FIRST content op.
        assert_eq!(
            ops.iter().filter(|o| matches!(o, ChatOp::Post(_))).count(),
            1,
            "one Post: {ops:?}"
        );
        assert!(matches!(ops[0], ChatOp::Post(_)), "Post first: {ops:?}");
        // Finalize is the LAST op.
        assert_eq!(ops.last(), Some(&ChatOp::Finalize), "Finalize last: {ops:?}");
        // Everything between is an Edit.
        assert!(
            ops[1..ops.len() - 1]
                .iter()
                .all(|o| matches!(o, ChatOp::Edit(_))),
            "middle are Edits: {ops:?}"
        );

        // The last rendered text (last Post/Edit) carries prose + tool line.
        let last = ops
            .iter()
            .rev()
            .find_map(|o| match o {
                ChatOp::Post(t) | ChatOp::Edit(t) => Some(t.clone()),
                ChatOp::Finalize => None,
            })
            .expect("some rendered text");
        assert!(last.contains("Hello world"), "prose present: {last:?}");
        assert!(last.contains("🔧 read Read File"), "tool line present: {last:?}");
    }

    /// A no-op event (e.g. usage) yields no ops, and a duplicate render is
    /// suppressed — the fold never emits a redundant edit.
    #[test]
    fn noncontent_and_duplicate_renders_emit_nothing() {
        let mut f = EventFolder::default();
        assert!(f.on_event(&chunk("hi")).len() == 1); // Post
        // A whitespace-only chunk doesn't change the trimmed render → no op.
        assert!(f.on_event(&chunk("")).is_empty(), "empty chunk is a no-op");
        // A plan update (non-mirrored) → no op.
        assert!(
            f.on_event(&AgentEventKind::ModelChanged("x".into())).is_empty(),
            "non-content kind is a no-op"
        );
    }

    /// After a turn boundary the fold resets: the next turn Posts a FRESH message
    /// rather than editing the previous one.
    #[test]
    fn turn_boundary_resets_to_a_fresh_post() {
        let mut f = EventFolder::default();
        let _ = f.on_event(&chunk("first turn"));
        let end = f.on_event(&AgentEventKind::TurnEnded {
            outcome: TurnOutcome::Completed,
        });
        assert_eq!(end.last(), Some(&ChatOp::Finalize));

        let next = f.on_event(&chunk("second turn"));
        assert!(
            matches!(next.as_slice(), [ChatOp::Post(t)] if t == "second turn"),
            "next turn starts a fresh Post: {next:?}"
        );
    }
}
