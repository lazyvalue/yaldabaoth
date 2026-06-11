//! Agent (Claude) tile data layer: tool-call model + renderers, transcript
//! flat-item view model (S1 cache + rebuild), turn/phase state machine,
//! chatbox, per-tile AgentState, the AgentSession payload + AgentTile view.
//! Extracted verbatim from main.rs (split-gpui-main). Render methods on
//! YaldaGpuiView stay in main.rs this pass.

use super::*;

/// Domain newtype for a tool-call identity (Finding 7, parse-don't-validate).
/// The protocol hands us a typed [`ToolCallId`](yalda::acp_channel::ToolCallId)
/// (`Arc<str>` under the hood); we parse it into this key ONCE at the boundary
/// (`apply_reply_events`) and key every tool map on it — `tool_calls`,
/// `tool_call_order`, `tool_call_anchor_line`, and `FlatItem::ToolGroup.ids`.
///
/// Deliberately NO `Deref` to `String`/`str`: a `ToolCallKey` is not
/// interchangeable with a session id, a label, or an arbitrary string, so a
/// mismatched key is a compile error rather than a silently-missed HashMap
/// lookup. Stringification happens only at the render edge (`as_str` /
/// `to_string`) where a DOM id or display label is needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ToolCallKey(pub(crate) yalda::acp_channel::ToolCallId);

/// The live tool-call state cluster, extracted from `AgentState` so the four
/// maps that must move together have ONE owner instead of four sibling fields
/// (spec-state-architecture A.6: shrink the god-struct; one atomic mutator per
/// coupled invariant). `calls`/`order`/`anchor` are registered together via
/// [`ToolCalls::register`] so they can't drift; `subagents()` derives live from
/// `calls` + `order` (ADR-0006 quick win #1 — no mirror).
#[derive(Default)]
pub(crate) struct ToolCalls {
    /// Live tool calls keyed by `tool_call_id`. Updated in place as the agent
    /// emits `ToolCallUpdate` notifications (status, incremental content).
    pub(crate) calls: std::collections::HashMap<ToolCallKey, yalda::acp_channel::ToolCall>,
    /// Display order — ids in the chronological order first announced. Drives
    /// render order and "render after which line" via `anchor`.
    pub(crate) order: Vec<ToolCallKey>,
    /// Anchors a call to the buffer line that was last frozen when it was
    /// announced; the renderer slots the tool block in just after that line.
    pub(crate) anchor: std::collections::HashMap<ToolCallKey, LineAnchor>,
    /// Ids the user expanded inline (default-collapsed).
    pub(crate) expanded: std::collections::HashSet<String>,
    /// Generation counter bumped on every mutation to `calls` or `expanded`.
    /// The render-side snapshot caches below use this to avoid deep-cloning
    /// the HashMap/HashSet on frames where nothing changed (chatbox keystrokes,
    /// cursor blink, cross-tile notify).
    snap_gen: u64,
    /// Cached `Rc` snapshot of `calls`, rebuilt lazily when `gen` advances.
    calls_snap: std::cell::RefCell<(
        u64,
        std::rc::Rc<std::collections::HashMap<ToolCallKey, yalda::acp_channel::ToolCall>>,
    )>,
    /// Cached `Rc` snapshot of `expanded`, rebuilt lazily when `gen` advances.
    expanded_snap: std::cell::RefCell<(u64, std::rc::Rc<std::collections::HashSet<String>>)>,
}

impl ToolCalls {
    /// Register a newly-announced tool call: append to `order` (if new) and
    /// record its call + anchor — the three maps move together so they cannot
    /// drift out of sync. THE single mutation chokepoint for a tool-call start.
    pub(crate) fn register(
        &mut self,
        key: ToolCallKey,
        call: yalda::acp_channel::ToolCall,
        anchor: LineAnchor,
    ) {
        if !self.calls.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.anchor.insert(key.clone(), anchor);
        self.calls.insert(key, call);
        self.snap_gen += 1;
    }

    /// Toggle a tool call's inline-expanded state (keyed by raw id string).
    pub(crate) fn toggle_expanded(&mut self, id: &str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
        self.snap_gen += 1;
    }

    /// Clear every map at a transcript reset (`reset_for_replay`).
    pub(crate) fn clear(&mut self) {
        self.calls.clear();
        self.order.clear();
        self.anchor.clear();
        self.expanded.clear();
        self.snap_gen += 1;
    }

    /// Mutable access to a tool call by key, bumping the generation counter
    /// so the next `calls_snapshot` will rebuild.
    pub(crate) fn call_mut(
        &mut self,
        key: &ToolCallKey,
    ) -> Option<&mut yalda::acp_channel::ToolCall> {
        let v = self.calls.get_mut(key);
        if v.is_some() {
            self.snap_gen += 1;
        }
        v
    }

    /// O(1) `Rc` clone when `calls` hasn't changed since the last snapshot.
    /// Deep-clones only when the generation counter has advanced.
    pub(crate) fn calls_snapshot(
        &self,
    ) -> std::rc::Rc<std::collections::HashMap<ToolCallKey, yalda::acp_channel::ToolCall>> {
        let mut snap = self.calls_snap.borrow_mut();
        if snap.0 != self.snap_gen {
            *snap = (self.snap_gen, std::rc::Rc::new(self.calls.clone()));
        }
        snap.1.clone()
    }

    /// O(1) `Rc` clone when `expanded` hasn't changed since the last snapshot.
    pub(crate) fn expanded_snapshot(&self) -> std::rc::Rc<std::collections::HashSet<String>> {
        let mut snap = self.expanded_snap.borrow_mut();
        if snap.0 != self.snap_gen {
            *snap = (self.snap_gen, std::rc::Rc::new(self.expanded.clone()));
        }
        snap.1.clone()
    }
}

impl ToolCallKey {
    /// Parse a protocol `ToolCallId` into the domain key. Cheap: clones an
    /// `Arc<str>`, no string allocation.
    pub(crate) fn from_id(id: &yalda::acp_channel::ToolCallId) -> Self {
        ToolCallKey(id.clone())
    }

    /// Borrow the underlying id as a `&str` (render edge only).
    pub(crate) fn as_str(&self) -> &str {
        &self.0.0
    }
}

impl std::fmt::Display for ToolCallKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row in the virtualised claude transcript list. The render
/// closure handed to `gpui::list` indexes into a `Vec<FlatItem>`
/// snapshot that mirrors the old "line N then any tool blocks
/// anchored at line N" emission order.
#[derive(Debug, Clone)]
pub(crate) enum FlatItem {
    /// Doc line at this index in the editor's document.
    Line(usize),
    /// A group of tool calls sharing the same anchor line. Rendered
    /// as a single "Ran N tool calls" header (collapsed) or header +
    /// individual rows (expanded). The anchor_line is the key for
    /// expand/collapse state.
    ToolGroup {
        anchor_line: usize,
        ids: Vec<ToolCallKey>,
    },
    /// A structurally-rendered block (table or fenced code block) that
    /// replaces a range of frozen lines with proper layout. `Rc` so the
    /// per-keystroke S1 rebuild reuses the parsed block by refcount bump
    /// instead of deep-cloning it (see `AgentViewModel::resolved_blocks`).
    Block(std::rc::Rc<RenderedBlock>),
    /// Visual divider at a turn boundary: role label + faint rule.
    TurnHeader { role: TurnRole },
    /// Pulsing indicator shown at transcript tail while awaiting reply.
    ThinkingIndicator,
}

/// Role shown in a `TurnHeader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnRole {
    Claude,
    User,
}

/// Free-function variant of `YaldaGpuiView::build_tool_block` that
/// works without an active `&Context<Self>`. Used inside `gpui::list`'s
/// per-item render closure (which only gets `&mut Window, &mut GpuiApp`).
/// Click handlers are wired through a `WeakEntity<YaldaGpuiView>`
/// captured at render-build time so toggling `expanded_tool_calls`
/// still goes through the same entity update path.
pub(crate) fn build_tool_block_with_weak(
    tc: &yalda::acp_channel::ToolCall,
    expanded: bool,
    code_font: &SharedString,
    weak_view: gpui::WeakEntity<YaldaGpuiView>,
    at: &yalda::theme::AgentTheme,
) -> AnyElement {
    use yalda::acp_channel::ToolCallStatus;
    let (status_glyph, status_color): (&str, Hsla) = match tc.status {
        ToolCallStatus::Pending => ("○", nc(at.tool_pending)),
        ToolCallStatus::InProgress => ("◐", nc(at.tool_in_progress)),
        ToolCallStatus::Completed => ("●", nc(at.tool_completed)),
        ToolCallStatus::Failed => ("✗", nc(at.tool_failed)),
        _ => ("·", nc(at.tool_pending)),
    };
    let dim_color = nc(at.dim);
    let policy = tool_render_policy(tc);
    let title = if tc.title.is_empty() {
        "(tool)".to_string()
    } else {
        tc.title.clone()
    };
    let id_str = tc.tool_call_id.0.to_string();
    let has_body = !matches!(policy, ToolRenderPolicy::HeaderOnly);
    let arrow = if has_body {
        if expanded { "▼" } else { "▶" }
    } else {
        " "
    };

    let mut summary_row = div()
        .id(SharedString::from(format!("tool-summary-{}", id_str)))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .py(px(5.0))
        .px_2()
        .child(div().text_color(dim_color).child(arrow))
        .child(div().text_color(status_color).child(status_glyph))
        .child(
            div()
                .text_color(nc(at.tool_body_fg))
                .text_size(px(12.0))
                .child(format!("[{:?}]", tc.kind).to_lowercase()),
        )
        .child(div().flex_1().text_color(nc(at.frozen_fg)).child(title));

    if has_body {
        let id_for_click = id_str.clone();
        summary_row = summary_row.cursor_pointer().on_click(
            move |_ev: &gpui::ClickEvent, _w: &mut Window, app: &mut GpuiApp| {
                let id = id_for_click.clone();
                let _ = weak_view.update(app, |this, cx| {
                    if let Some(c) = this.agent_mut() {
                        c.tools.toggle_expanded(&id);
                    }
                    cx.notify();
                });
            },
        );
    }

    let mut block = div()
        .flex()
        .flex_col()
        .my_1()
        .pl_2()
        .ml_2()
        .border_l_2()
        .border_color(nc(at.tool_card_border))
        .child(summary_row);

    if expanded && has_body {
        let max_lines = match policy {
            ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
            _ => None,
        };
        let body_bg = nc(at.tool_body_bg);
        let output_bg = nc(at.tool_output_bg);
        let body_fg = nc(at.tool_body_fg);
        let diff_add = nc(at.diff_add);
        let diff_remove = nc(at.diff_remove);
        let diff_header = nc(at.diff_header);
        if let Some(input) = &tc.raw_input {
            let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
            block = block.child(tool_body_free(
                "input",
                &pretty,
                None,
                body_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
        let content_text = render_tool_content_blocks(&tc.content);
        if !content_text.trim().is_empty() {
            block = block.child(tool_body_free(
                "content",
                &content_text,
                max_lines,
                body_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
        if let Some(output) = &tc.raw_output {
            let pretty =
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
            block = block.child(tool_body_free(
                "output",
                &pretty,
                max_lines,
                output_bg,
                body_fg,
                code_font,
                diff_add,
                diff_remove,
                diff_header,
            ));
        }
    }

    block.into_any_element()
}

/// Free-function form of [`YaldaGpuiView::tool_body`] for the
/// virtualised render path. Same content layout, accepts a borrowed
/// `code_font` instead of reaching through `&self`.
// builder/render fn — arg count is inherent, splitting would obscure
#[allow(clippy::too_many_arguments)]
pub(crate) fn tool_body_free(
    label: &str,
    body: &str,
    max_lines: Option<usize>,
    bg: Hsla,
    fg: Hsla,
    code_font: &SharedString,
    diff_add: Hsla,
    diff_remove: Hsla,
    diff_header: Hsla,
) -> gpui::Div {
    let display = match max_lines {
        Some(n) => truncate_lines(body, n),
        None => body.to_string(),
    };

    // Build diff-highlighted lines: color +/- lines and diff headers.
    let mut container = div()
        .mt_1()
        .mx_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(bg)
        .text_size(px(11.0))
        .text_color(fg)
        .font_family(code_font.clone());

    // Label
    container = container.child(
        div()
            .text_size(px(10.0))
            .pb(px(2.0))
            .child(SharedString::from(format!("{}:", label))),
    );

    // Diff-highlighted body lines.
    for line in display.lines() {
        let color = if line.starts_with("+ ") || line.starts_with("+\t") || line == "+" {
            diff_add
        } else if line.starts_with("- ") || line.starts_with("-\t") || line == "-" {
            diff_remove
        } else if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ") {
            diff_header
        } else {
            fg
        };
        container = container.child(
            div()
                .text_color(color)
                .child(SharedString::from(line.to_string())),
        );
    }

    container
}

/// How much of a tool call's body to render when expanded. Mirrors the
/// per-tool policy baked into Claude Code's TUI (see cli.js's
/// `renderToolResultMessage` table) — Read/Search show no body, Bash
/// shows the first 3 lines, edits show their full diff, etc.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ToolRenderPolicy {
    /// No body even when expanded — the user only needs to know the
    /// action happened. Read, Grep/Glob, TodoWrite, mode switches.
    HeaderOnly,
    /// Show at most this many lines per body tile; cap further with a
    /// "+N lines hidden" footer.
    Truncated { max_lines: usize },
    /// Show everything. For diffs, MCP tool returns, Task subagents.
    Full,
}

/// Pick the render policy for a tool call. We classify on `kind`
/// (mapped from claude-code-acp's `tools.js`) plus a couple of
/// raw_input sniffs for tools the kind alone doesn't disambiguate
/// (TodoWrite is `Think`, same as Task — but the user wants its body
/// hidden, so we detect it by an `input.todos` field).
pub(crate) fn tool_render_policy(tc: &yalda::acp_channel::ToolCall) -> ToolRenderPolicy {
    use yalda::acp_channel::ToolKind;
    // TodoWrite shows up as `kind=Think` (same as the Task subagent),
    // and its body is the running todo list — too noisy to render. Sniff
    // for the distinctive `todos` array on the input to tell them apart.
    let is_todowrite = tc.raw_input.as_ref().and_then(|v| v.get("todos")).is_some();
    if is_todowrite {
        return ToolRenderPolicy::HeaderOnly;
    }
    match tc.kind {
        ToolKind::Read | ToolKind::Search | ToolKind::SwitchMode => ToolRenderPolicy::HeaderOnly,
        ToolKind::Execute => ToolRenderPolicy::Truncated { max_lines: 3 },
        ToolKind::Fetch => ToolRenderPolicy::Truncated { max_lines: 10 },
        ToolKind::Edit
        | ToolKind::Move
        | ToolKind::Delete
        | ToolKind::Think
        | ToolKind::Other
        | _ => ToolRenderPolicy::Full,
    }
}

/// Extract a short detail string from a tool call's input for inline
/// display in the group header. Returns the file path for Read/Edit/Write,
/// truncated command for Execute, query for Search, etc.
pub(crate) fn tool_inline_detail(tc: &yalda::acp_channel::ToolCall) -> Option<String> {
    let input = tc.raw_input.as_ref()?;
    // Try file_path first (Read, Edit, Write, Glob).
    if let Some(fp) = input.get("file_path").and_then(|v| v.as_str()) {
        // Show just the filename or last path component to keep it short.
        let short = std::path::Path::new(fp)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(fp);
        return Some(short.to_string());
    }
    // Execute/Bash: show truncated command.
    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        let first_line = cmd.lines().next().unwrap_or(cmd);
        let truncated = if first_line.len() > 60 {
            format!("{}…", &first_line[..60])
        } else {
            first_line.to_string()
        };
        return Some(truncated);
    }
    // Search (Grep/Glob): show pattern.
    if let Some(pat) = input.get("pattern").and_then(|v| v.as_str()) {
        let truncated = if pat.len() > 40 {
            format!("{}…", &pat[..40])
        } else {
            pat.to_string()
        };
        return Some(truncated);
    }
    None
}

/// Short type label for a tool call used in collapsed group headers
/// (e.g. "grep", "edit", "read"). Prefers the leading word of the title — for
/// claude-code-acp this is the tool name (Grep / Read / Bash / Edit / …) — and
/// falls back to the ACP `kind` when the title isn't a clean single token.
pub(crate) fn tool_type_label(tc: &yalda::acp_channel::ToolCall) -> String {
    tc.title
        .split_whitespace()
        .next()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty() && w.len() <= 12 && w.chars().all(|c| c.is_alphanumeric()))
        .unwrap_or_else(|| tool_kind_label(&tc.kind))
}

/// Fallback label derived from the ACP tool kind when the title isn't usable.
pub(crate) fn tool_kind_label(kind: &yalda::acp_channel::ToolKind) -> String {
    use yalda::acp_channel::ToolKind;
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
        ToolKind::Other => "tool",
        _ => "tool",
    }
    .to_string()
}

/// Append a tool call's body tiles directly to a container div.
/// Used for single-tool groups where we skip the nested sub-header.
pub(crate) fn append_tool_body(
    mut block: gpui::Div,
    tc: &yalda::acp_channel::ToolCall,
    policy: ToolRenderPolicy,
    code_font: &SharedString,
    at: &yalda::theme::AgentTheme,
) -> gpui::Div {
    let max_lines = match policy {
        ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
        _ => None,
    };
    let body_bg = nc(at.tool_body_bg);
    let output_bg = nc(at.tool_output_bg);
    let body_fg = nc(at.tool_body_fg);
    let diff_add = nc(at.diff_add);
    let diff_remove = nc(at.diff_remove);
    let diff_header = nc(at.diff_header);
    if let Some(input) = &tc.raw_input {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        block = block.child(tool_body_free(
            "input",
            &pretty,
            None,
            body_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    let content_text = render_tool_content_blocks(&tc.content);
    if !content_text.trim().is_empty() {
        block = block.child(tool_body_free(
            "content",
            &content_text,
            max_lines,
            body_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    if let Some(output) = &tc.raw_output {
        let pretty = serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
        block = block.child(tool_body_free(
            "output",
            &pretty,
            max_lines,
            output_bg,
            body_fg,
            code_font,
            diff_add,
            diff_remove,
            diff_header,
        ));
    }
    block
}

/// Flatten a tool call's `Vec<ToolCallContent>` into a single human-
/// readable string. Splits diffs into a labelled `--- path` header plus
/// old/new bodies; treats terminal embeds as a one-line placeholder.
/// Centralised so policy tweaks (e.g., suppressing the old half of a
/// diff) only need to be made in one spot.
pub(crate) fn render_tool_content_blocks(
    content: &[yalda::acp_channel::ToolCallContent],
) -> String {
    use yalda::acp_channel::ToolCallContent;
    let mut buf = String::new();
    for c in content {
        match c {
            ToolCallContent::Content(content) => {
                if let agent_client_protocol::schema::ContentBlock::Text(t) = &content.content {
                    buf.push_str(&t.text);
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                }
            }
            ToolCallContent::Diff(d) => {
                buf.push_str(&format!("--- {}\n", d.path.display()));
                if let Some(old) = &d.old_text {
                    buf.push_str("- (old)\n");
                    buf.push_str(old);
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                }
                buf.push_str("+ (new)\n");
                buf.push_str(&d.new_text);
                if !buf.ends_with('\n') {
                    buf.push('\n');
                }
            }
            ToolCallContent::Terminal(_) => {
                buf.push_str("[terminal embed — not rendered]\n");
            }
            // ToolCallContent is `#[non_exhaustive]`; future variants
            // render as a placeholder rather than failing to build.
            _ => {
                buf.push_str("[unsupported content variant]\n");
            }
        }
    }
    buf
}

/// Trim `body` to its first `max_lines` lines. If anything was dropped,
/// append a dim "+N lines hidden" footer so the user knows there's more
/// to see — they can re-expand on a wider window or pop the original
/// payload off-screen by collapsing the block.
pub(crate) fn truncate_lines(body: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let mut lines = body.lines();
    let mut head: Vec<&str> = Vec::with_capacity(max_lines);
    for _ in 0..max_lines {
        match lines.next() {
            Some(l) => head.push(l),
            None => break,
        }
    }
    let remaining = lines.count();
    let mut out = head.join("\n");
    if remaining > 0 {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("… +{} lines hidden", remaining));
    }
    out
}

/// Pick the buffer line a freshly-announced tool call should anchor to,
/// and force a section break so subsequent text chunks don't extend the
/// pre-tool line.
///
/// Why the section break matters: text chunks splice verbatim — there's
/// no per-chunk newline in the buffer. If Claude says "Let me check
/// this" → tool fires → "now I see X", all three pieces would land on
/// the same doc line and the tool block (rendered after that line)
/// would visually appear AFTER both halves of text instead of between
/// them. Inserting a `\n` here forces the post-tool text onto a new
/// line below the tool block, restoring chronological order.
///
/// Anchor lands on the last line containing actual text (i.e., the line
/// terminated by the trailing `\n`), so the tool block renders just
/// after the pre-tool content and just before the empty line where the
/// next chunk will splice in.
///
/// Returns a [`LineAnchor`] that survives inserts and deletes elsewhere in
/// the document, per spec-agent-window.md §E1. The renderer resolves it to
/// a line index via `editor.line_for_anchor(a)`; a `None` (line consumed)
/// falls back to EOF rendering.
pub(crate) fn anchor_for_new_tool_call(editor: &mut Editor) -> LineAnchor {
    // Perf (finding 5): O(1) tail probe instead of cloning the whole transcript
    // (`full_text`) just to test emptiness + trailing newline per tool call.
    if !editor.document().is_empty() && editor.document().last_char() != Some('\n') {
        let len = editor.document().rope().len_chars();
        editor.programmatic_insert(len, "\n");
    }
    // Append a dedicated blank line for the tool block to anchor on, rather
    // than reusing the trailing LLM content line. Tagging the anchor line
    // `Tool(k)` (for the gutter) would otherwise steal that line's `Llm(k)`
    // tag, and `find_llm_insertion_point` keys off the last `Llm`-tagged line
    // to place the turn's next chunk — stealing it makes post-tool prose
    // splice into an earlier line (the "ThereLet" / "Found key line" clobber).
    let len = editor.document().rope().len_chars();
    editor.programmatic_insert(len, "\n");
    let line_count = editor.document().line_count();
    // The dedicated blank line we just created is line_count - 2
    // (line_count - 1 is the empty trailing line). saturating_sub guards an
    // empty doc, where the tool block just anchors at the top.
    let line = line_count.saturating_sub(2);
    editor.anchor_for_line(line)
}

/// Detect line ranges in `lines` that should be rendered as structured
/// blocks (tables and fenced code blocks) rather than line-by-line.
/// Only considers frozen (agent-written) lines.
///
/// Returns `Vec<(start, end)>` where `start..end` covers the full block
/// including delimiters. Ranges are non-overlapping and sorted.
/// Pure follow-tail policy (F4, INV-13), factored out of `AgentState` so it
/// can be unit-tested without a GPUI editor/list. In Chatbox mode the user's
/// cursor is outside the transcript, so following is purely the sticky-bottom
/// `follow_output` flag; in Worksheet mode the viewport tracks the cursor and
/// follows only when the cursor is at EOF.
pub(crate) fn should_follow_tail(
    input_mode: InputModeKind,
    follow_output: bool,
    cursor_at_eof: bool,
) -> bool {
    match input_mode {
        InputModeKind::Chatbox => follow_output,
        InputModeKind::Worksheet => cursor_at_eof,
    }
}

/// 64-bit content hash of a detected block range's source lines — the
/// `AgentViewModel::block_cache` key. Content (not position) so a streamed
/// chunk that shifts every range downward still reuses every prior parse;
/// see the field docs for the collision tradeoff.
pub(crate) fn block_content_hash(lines: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut h);
    for l in lines {
        l.hash(&mut h);
    }
    h.finish()
}

pub(crate) fn detect_block_ranges(
    lines: &[String],
    frozen_ranges: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let is_frozen = |i: usize| -> bool { frozen_ranges.iter().any(|&(s, e)| i >= s && i < e) };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_frozen(i) {
            i += 1;
            continue;
        }
        let trimmed = lines[i].trim();

        // Fenced code block: starts with ``` (optionally with language)
        if trimmed.starts_with("```") {
            let start = i;
            i += 1;
            // Find closing fence. Track whether we actually matched one —
            // exhausting the buffer is NOT a close (INV-11). A streaming,
            // still-open fence must render its arrived lines as plain Lines
            // until the closing delimiter freezes, so each new line stays
            // its own FlatItem (keeping the count-keyed scroll path live)
            // and we avoid an O(block) re-parse-to-EOF every chunk (F12).
            let mut closed = false;
            while i < lines.len() {
                if lines[i].trim().starts_with("```") && lines[i].trim().len() <= trimmed.len() + 20
                {
                    i += 1; // include the closing fence
                    closed = true;
                    break;
                }
                i += 1;
            }
            // Only emit a block range once the closing fence is present
            // (symmetric to the >=3-row table rule below). Without a match
            // the loop ran to EOF, so leave these lines unblocked.
            if closed && i > start + 1 {
                ranges.push((start, i));
            }
            continue;
        }

        // Table: consecutive lines starting with `|` (need at least 2 rows
        // for a header + separator).
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let start = i;
            while i < lines.len() && is_frozen(i) {
                let t = lines[i].trim();
                if t.starts_with('|') && t.ends_with('|') {
                    i += 1;
                } else {
                    break;
                }
            }
            if i - start >= 3 {
                // 3+ rows: header, separator, at least one data row
                ranges.push((start, i));
            }
            continue;
        }

        i += 1;
    }
    ranges
}

/// Parse a contiguous line range into a single RenderedBlock (table or code
/// block). Returns `None` if the parser doesn't produce a usable block.
/// Outcome of trying to render a detected range as a single structured block.
/// Total over the partition (Finding 10, INV-10): a detected range either
/// becomes one `Parsed` block or explicitly `FallBackToLines`, so the flat
/// build emits either the Block or the constituent Lines for every range —
/// "detected but not emitted" is unrepresentable rather than an `Option::None`
/// a later reader might forget to expand back into lines.
pub(crate) enum BlockParse {
    Parsed(RenderedBlock),
    FallBackToLines,
}

pub(crate) fn parse_block_range(
    lines: &[String],
    start: usize,
    end: usize,
    theme: &Theme,
) -> BlockParse {
    let slice: String = lines[start..end].join("\n");
    let blocks = render_with_wiki(&slice, theme, None);
    // Take the first Table or CodeBlock produced.
    for b in blocks {
        match &b {
            RenderedBlock::Table { .. } | RenderedBlock::CodeBlock { .. } => {
                return BlockParse::Parsed(b);
            }
            _ => {}
        }
    }
    BlockParse::FallBackToLines
}

/// Storage-side ceiling for any single text payload we keep on a tool
/// call (one diff body, one content text block, one raw_input/raw_output
/// JSON blob). Tool results from the agent — especially Bash captures
/// or large grep outputs — can run into the megabytes; that's fine to
/// hand back to the model, but storing it verbatim makes our render
/// pass tokenize and lay out a wall of text every time the user
/// expands a tool block. 64K chars per payload is generous enough to
/// keep typical traces intact while bounding the worst case.
pub(crate) const TOOL_PAYLOAD_MAX_CHARS: usize = 65_536;

/// Trim oversized strings on a tool call's content/raw_input/raw_output
/// to [`TOOL_PAYLOAD_MAX_CHARS`]. Idempotent: re-running on a tool that
/// got further updated only re-trims new growth.
pub(crate) fn cap_tool_call_payloads(tc: &mut yalda::acp_channel::ToolCall) {
    use yalda::acp_channel::ToolCallContent;
    for c in tc.content.iter_mut() {
        match c {
            ToolCallContent::Content(content) => {
                if let agent_client_protocol::schema::ContentBlock::Text(t) = &mut content.content
                    && t.text.chars().count() > TOOL_PAYLOAD_MAX_CHARS
                {
                    t.text = cap_string_chars(&t.text, TOOL_PAYLOAD_MAX_CHARS);
                }
            }
            ToolCallContent::Diff(d) => {
                if d.new_text.chars().count() > TOOL_PAYLOAD_MAX_CHARS {
                    d.new_text = cap_string_chars(&d.new_text, TOOL_PAYLOAD_MAX_CHARS);
                }
                if let Some(old) = &mut d.old_text
                    && old.chars().count() > TOOL_PAYLOAD_MAX_CHARS
                {
                    *old = cap_string_chars(old, TOOL_PAYLOAD_MAX_CHARS);
                }
            }
            _ => {}
        }
    }
    if let Some(input) = &mut tc.raw_input {
        cap_json_value_strings(input, TOOL_PAYLOAD_MAX_CHARS);
    }
    if let Some(output) = &mut tc.raw_output {
        cap_json_value_strings(output, TOOL_PAYLOAD_MAX_CHARS);
    }
}

/// Walk a `serde_json::Value` and trim any string leaf longer than
/// `max_chars`. Used on tool-call raw_input/raw_output so a single
/// massive `stdout` field can't bloat the cached payload.
pub(crate) fn cap_json_value_strings(v: &mut serde_json::Value, max_chars: usize) {
    match v {
        serde_json::Value::String(s) => {
            if s.chars().count() > max_chars {
                *s = cap_string_chars(s, max_chars);
            }
        }
        serde_json::Value::Array(arr) => {
            for x in arr {
                cap_json_value_strings(x, max_chars);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, x) in map {
                cap_json_value_strings(x, max_chars);
            }
        }
        _ => {}
    }
}

/// Cap a string at `max_chars` UTF-8 chars, replacing the dropped tail
/// with a marker. Used at storage time on tool-call content/output so
/// the renderer never has to chew through multi-MB payloads even when
/// the user expands a tool block. Operates on chars (not bytes) to
/// avoid splitting multi-byte sequences.
pub(crate) fn cap_string_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    let dropped = s.chars().count() - max_chars;
    format!("{head}\n… (+{dropped} chars truncated at storage)")
}

/// Tokenize a line's segments into per-word + per-whitespace-run children
/// inside a `flex_wrap` row, so the GPUI flex layout breaks at word
/// boundaries when the row exceeds container width. StyledText itself
/// doesn't word-wrap, so we have to feed flex many small children for it
/// to have somewhere to break.
///
/// Cursor handling is fused in: for the cursor line, the caret is emitted
/// inline as its own flex child between the before/after halves of the
/// containing token. This keeps wrap behaviour consistent across cursor
/// and non-cursor lines.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_wrapped_line(
    segs: &[Segment],
    line_str: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    base_style: NStyle,
    base_fg: u32,
    code_font: &SharedString,
) -> AnyElement {
    let mut row = div().flex().flex_row().flex_wrap().flex_1().min_w_0();

    // Tokenize each segment into runs of whitespace vs non-whitespace,
    // preserving the segment's style on each token.
    let mut tokens: Vec<Segment> = Vec::new();
    for (text, style) in segs {
        if text.is_empty() {
            continue;
        }
        let mut current = String::new();
        let mut current_ws = false;
        for ch in text.chars() {
            let is_ws = ch == ' ' || ch == '\t';
            if current.is_empty() {
                current_ws = is_ws;
                current.push(ch);
            } else if current_ws == is_ws {
                current.push(ch);
            } else {
                tokens.push((std::mem::take(&mut current), *style));
                current_ws = is_ws;
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push((current, *style));
        }
    }

    // Empty-line placeholder so the row still occupies a visual line.
    if tokens.is_empty() {
        let line = segments_to_styled_line(&[(" ".to_string(), base_style)]);
        row = row.child(styled_line_element(
            &line, base_style, base_fg, code_font, code_font,
        ));
        if is_cursor_line {
            row = row.child(make_caret(mode, ' ', cursor_color));
        }
        return row.into_any_element();
    }

    if !is_cursor_line {
        for (text, style) in &tokens {
            let line = segments_to_styled_line(&[(text.clone(), *style)]);
            row = row.child(styled_line_element(
                &line, base_style, base_fg, code_font, code_font,
            ));
        }
        return row.into_any_element();
    }

    // Cursor line: walk tokens by visual column and inject the caret at the
    // cursor's column boundary, splitting the containing token if needed.
    let line_chars = line_str.chars().count();
    let cursor_col = cursor_col.min(line_chars);
    let mut col_so_far = 0usize;
    let mut caret_emitted = false;

    for (text, style) in &tokens {
        let token_chars = text.chars().count();
        let token_end_col = col_so_far + token_chars;
        let caret_in_token =
            !caret_emitted && cursor_col >= col_so_far && cursor_col <= token_end_col;

        if caret_in_token {
            let split_point = cursor_col - col_so_far;
            let chars: Vec<char> = text.chars().collect();
            let before: String = chars[..split_point].iter().collect();
            if !before.is_empty() {
                let line = segments_to_styled_line(&[(before, *style)]);
                row = row.child(styled_line_element(
                    &line, base_style, base_fg, code_font, code_font,
                ));
            }
            let cursor_char = chars.get(split_point).copied().unwrap_or(' ');
            row = row.child(make_caret(mode, cursor_char, cursor_color));
            caret_emitted = true;
            // After-the-caret: in Normal mode the cursor cell consumed the
            // char at split_point; in Insert mode it's a zero-width beam so
            // the char at split_point still belongs to the after-stream.
            let after_start = match mode {
                EditMode::Normal => split_point + 1,
                EditMode::Insert => split_point,
            };
            if after_start < chars.len() {
                let after: String = chars[after_start..].iter().collect();
                let line = segments_to_styled_line(&[(after, *style)]);
                row = row.child(styled_line_element(
                    &line, base_style, base_fg, code_font, code_font,
                ));
            }
        } else {
            let line = segments_to_styled_line(&[(text.clone(), *style)]);
            row = row.child(styled_line_element(
                &line, base_style, base_fg, code_font, code_font,
            ));
        }
        col_so_far = token_end_col;
    }

    // Cursor sits past the last char (e.g., end-of-line in Insert mode).
    if !caret_emitted {
        row = row.child(make_caret(mode, ' ', cursor_color));
    }

    row.into_any_element()
}

/// Build the cursor caret element. Pulled out so the empty-line, mid-line,
/// and end-of-line code paths all produce identical-looking carets.
///
/// Render a single chatbox logical line as a wrapping row.
///
/// Long lines wrap at whitespace boundaries (flex_wrap), so the cursor stays
/// visible without horizontal scrolling. The caret is emitted inline as its
/// own flex child between the before/after halves of the containing token,
/// so wrap behaviour stays consistent across cursor and non-cursor lines.
// builder/render fn — arg count is inherent, splitting would obscure
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_chatbox_line(
    full_text: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    sel: Option<((usize, usize), (usize, usize))>,
    line_idx: usize,
    total_line_chars: usize,
    code_font: &SharedString,
    text_color: Hsla,
) -> AnyElement {
    let line_h = px(18.0);
    let fg: Hsla = text_color;
    let sel_bg: Hsla = ncolor_to_hsla(SELECTION_BG, BG);

    let chars: Vec<char> = full_text.chars().collect();
    let char_count = chars.len();

    // Selection range projected onto this line.
    let line_sel = sel
        .and_then(|s| line_selection_range(s, line_idx, total_line_chars))
        .and_then(|(s, e)| {
            if e > s {
                Some((s.min(char_count), e.min(char_count)))
            } else {
                None
            }
        });

    // Tokenize into whitespace vs non-whitespace runs so flex_wrap can break
    // at token boundaries. Each token becomes its own flex child.
    let mut tokens: Vec<String> = Vec::new();
    {
        let mut current = String::new();
        let mut current_ws = false;
        for ch in chars.iter().copied() {
            let is_ws = ch == ' ' || ch == '\t';
            if current.is_empty() {
                current_ws = is_ws;
                current.push(ch);
            } else if current_ws == is_ws {
                current.push(ch);
            } else {
                tokens.push(std::mem::take(&mut current));
                current_ws = is_ws;
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }

    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .min_w_0()
        .w_full()
        .min_h(line_h)
        // Pin the text line-box to the caret height (18px) so a line carrying
        // a glyph is exactly as tall as the empty/placeholder line (which only
        // holds the fixed-height caret). Without this, the font's default
        // line-height makes a one-character chatbox a hair taller than an
        // empty one.
        .line_height(line_h)
        .font_family(code_font.clone())
        .text_size(px(13.0))
        .text_color(fg);

    // Emit a chunk of text with the on-line selection highlight painted
    // through any overlapping range. `chunk_start_col` is the column at
    // which `text` begins on the logical line.
    let emit_chunk = |row: gpui::Div, text: String, chunk_start_col: usize| -> gpui::Div {
        if text.is_empty() {
            return row;
        }
        let chunk_chars: Vec<char> = text.chars().collect();
        let chunk_len = chunk_chars.len();
        let chunk_end_col = chunk_start_col + chunk_len;
        if let Some((ss, se)) = line_sel
            && se > chunk_start_col
            && ss < chunk_end_col
        {
            let local_ss = ss.saturating_sub(chunk_start_col).min(chunk_len);
            let local_se = se.saturating_sub(chunk_start_col).min(chunk_len);
            let mut r = row;
            if local_ss > 0 {
                let pre: String = chunk_chars[..local_ss].iter().collect();
                r = r.child(pre);
            }
            if local_se > local_ss {
                let in_sel: String = chunk_chars[local_ss..local_se].iter().collect();
                r = r.child(div().bg(sel_bg).child(in_sel));
            }
            if local_se < chunk_len {
                let post: String = chunk_chars[local_se..].iter().collect();
                r = r.child(post);
            }
            return r;
        }
        row.child(text)
    };

    // Empty line: just emit a placeholder space + (cursor if needed) so the
    // row still occupies a visual line.
    if tokens.is_empty() {
        if is_cursor_line {
            row = row.child(make_caret(mode, ' ', cursor_color));
        } else {
            row = row.child(" ");
        }
        return row.into_any_element();
    }

    if !is_cursor_line {
        let mut col_so_far = 0usize;
        for token in &tokens {
            let token_len = token.chars().count();
            row = emit_chunk(row, token.clone(), col_so_far);
            col_so_far += token_len;
        }
        return row.into_any_element();
    }

    // Cursor line: walk tokens by column and inject the caret at the cursor's
    // column boundary, splitting the containing token if needed.
    let cursor_col = cursor_col.min(char_count);
    let mut col_so_far = 0usize;
    let mut caret_emitted = false;
    for token in &tokens {
        let token_chars: Vec<char> = token.chars().collect();
        let token_len = token_chars.len();
        let token_end_col = col_so_far + token_len;
        let caret_in_token =
            !caret_emitted && cursor_col >= col_so_far && cursor_col <= token_end_col;

        if caret_in_token {
            let split_point = cursor_col - col_so_far;
            let before: String = token_chars[..split_point].iter().collect();
            if !before.is_empty() {
                row = emit_chunk(row, before, col_so_far);
            }
            let cursor_char = token_chars.get(split_point).copied().unwrap_or(' ');
            row = row.child(make_caret(mode, cursor_char, cursor_color));
            caret_emitted = true;
            // In Normal mode the cursor cell consumed the char at split_point;
            // in Insert mode the caret is a zero-width beam so the char at
            // split_point still belongs to the after-stream.
            let after_start = match mode {
                EditMode::Normal => split_point + 1,
                EditMode::Insert => split_point,
            };
            if after_start < token_len {
                let after: String = token_chars[after_start..].iter().collect();
                row = emit_chunk(row, after, col_so_far + after_start);
            }
        } else {
            row = emit_chunk(row, token.clone(), col_so_far);
        }
        col_so_far = token_end_col;
    }

    // Cursor sits past the last char (e.g., end-of-line in Insert mode).
    if !caret_emitted {
        row = row.child(make_caret(mode, ' ', cursor_color));
    }

    row.into_any_element()
}

/// Wire a `ListState`'s scroll handler to update the shared `follow_output`
/// flag. When the user scrolls up (`is_scrolled == true`), follow is disabled.
/// When they scroll back to the bottom (`is_scrolled == false`), it re-enables.
pub(crate) fn setup_list_follow_handler(
    list_state: &gpui::ListState,
    follow: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    let flag = follow.clone();
    list_state.set_scroll_handler(move |ev: &gpui::ListScrollEvent, _w, _cx| {
        // `is_scrolled` is false when the list is pinned to the bottom
        // (logical_scroll_top == None in GPUI's ListState internals).
        flag.set(!ev.is_scrolled);
    });
}

/// Called when the ACP turn ends (the agent's `session/prompt` response
/// resolves). Ensures the transcript has a trailing newline so the next
/// chunk has a clean starting point. The cursor stays where the user put
/// it (the worksheet is cursor-anchored, not auto-following the agent —
/// spec-agent-window.md §19).
/// The pump-side decision an [`AgentEventKind`] fold surfaces (spec §7). The
/// reducer ([`YaldaGpuiView::apply_agent_event`]) is a pure state-fold and
/// does NOT finalize or flip `turn_phase` inside itself; instead it returns one
/// of these so the CALLER routes a boundary through the idempotent
/// `finalize_agent_turn_idem` ledger and owns the `turn_phase = Idle` flip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentEventEffect {
    /// Streamed content / non-boundary event — no finalize.
    None,
    /// A live/terminal turn boundary (spec §5). The caller finalizes
    /// `(generation, turn)` idempotently and flips the phase to `Idle`.
    TurnEnded { generation: u64, turn: usize },
    /// End of the replayed history prefix (old `ReplayComplete`). This is a
    /// replay-prefix MARKER, not a turn completion: it must NOT occupy a
    /// `(generation, turn)` slot in the per-turn finalize ledger, because the
    /// server stamps the `ReplayEnd` envelope `turn` with the CURRENT settled
    /// count `self.turns` — the SAME index the next live turn's `completed_turn`
    /// carries — so keying the turn ledger here would pre-occupy the upcoming
    /// live turn's entry and wedge its finalize (the stuck-thinking-after-resume
    /// bug). The caller instead settles the replay buffer through a DEDICATED
    /// idempotency (`finalize_replay_prefix`) and flips the phase to `Idle` only
    /// if no live turn is in flight.
    ReplayEnded,
}

pub(crate) fn finalize_agent_turn(editor: &mut Editor) {
    let total_len = editor.document().rope().len_chars();
    let needs_newline = total_len == 0
        || editor
            .document()
            .full_text()
            .chars()
            .last()
            .map(|c| c != '\n')
            .unwrap_or(true);
    if needs_newline {
        editor.programmatic_insert(total_len, "\n");
    }
    // Perf cache (finding 2): the turn is over; invalidate the LLM-tail hint so
    // the next turn re-anchors from scratch instead of trusting a stale line.
    editor.clear_cached_llm_line();
}

/// Which input surface the agent window is currently presenting. Per
/// spec-agent-window.md §4, every `AgentState` carries one of these two
/// values; new sessions start at `Chatbox` to match today's compose-box-
/// first feel. Toggled by `Ctrl-Alt-Enter` (§5).
///
/// `InputModeKind` is the **Copy discriminant** — the two-variant tag with no
/// payload, kept for the persisted `PersistedSlot.mode` string and the
/// `should_follow_tail` policy fn. The live state is [`InputSurface`], which
/// owns the `Chatbox` inside its variant so "a chatbox exists iff we're in
/// Chatbox mode" is enforced by the type rather than two hand-synced fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputModeKind {
    /// User input is interleaved with LLM output in the transcript editor.
    /// Frozen lines are immutable; editable lines accumulate until a Submit
    /// sweeps and freezes them all (§9–§15).
    Worksheet,
    /// User input goes into a separate `Chatbox` editor pinned to the
    /// bottom of the window. The transcript is read-only while in this
    /// mode (§16–§20).
    Chatbox,
}

/// The single live input surface of an agent window (§4–§5). Replaces the old
/// `input_mode: InputMode` + `chatbox: Option<Chatbox>` pair: the Chatbox data
/// lives INSIDE the `Chatbox` variant, so the illegal states (Chatbox-mode with
/// no box, or Worksheet with a stranded box) are unrepresentable. New sessions
/// start at `Chatbox` (compose-box-first); `Ctrl-Alt-Enter` toggles. NOT `Copy`
/// (it owns a `Chatbox`).
// wire/event enum — boxing the large variant would ripple through serialization + every match site
#[allow(clippy::large_enum_variant)]
pub(crate) enum InputSurface {
    Worksheet,
    Chatbox(Chatbox),
}

impl InputSurface {
    pub(crate) fn is_chatbox(&self) -> bool {
        matches!(self, InputSurface::Chatbox(_))
    }
    pub(crate) fn chatbox(&self) -> Option<&Chatbox> {
        match self {
            InputSurface::Chatbox(cb) => Some(cb),
            InputSurface::Worksheet => None,
        }
    }
    pub(crate) fn chatbox_mut(&mut self) -> Option<&mut Chatbox> {
        match self {
            InputSurface::Chatbox(cb) => Some(cb),
            InputSurface::Worksheet => None,
        }
    }
    /// The Copy discriminant, for the persisted mode string and
    /// `should_follow_tail` (which must not borrow the owned `Chatbox`).
    pub(crate) fn mode(&self) -> InputModeKind {
        match self {
            InputSurface::Worksheet => InputModeKind::Worksheet,
            InputSurface::Chatbox(_) => InputModeKind::Chatbox,
        }
    }
}

/// Tool names that the v1 sub-agent classifier treats as sub-agents.
/// Centralised here so swapping in a structured ACP sub-agent type — or
/// supporting a renamed vendor tool — is a one-slice change (§25).
pub(crate) const SUBAGENT_TOOL_NAMES: &[&str] = &["Task", "Subagent", "Spawn"];

/// Yalda-side classification of a `ToolCall` that represents a sub-agent
/// transcript (§26). Produced by the heuristic in `classify_subagent`; the
/// `Subagents` sidebar lists these, and `focused_subagent` keys into the
/// derived list (by `tool_call_id`) to swap the main transcript view.
///
/// Not stored: `AgentState::subagents()` derives this list on demand by
/// folding over `tool_call_order` + `tool_calls`, so it can never drift
/// from the underlying tool-call state (ADR-0006 quick win #1).
#[derive(Clone)]
pub(crate) struct SubAgent {
    /// Originating tool-call id. The tool call itself stays in
    /// `tool_calls`; the sub-agent entry is an extra view over the same
    /// content.
    pub(crate) tool_call_id: ToolCallKey,
    /// Best-effort display label: the tool call's `title` if set,
    /// otherwise its `name`, with `subagent-N` as the ultimate fallback.
    pub(crate) label: String,
    /// Mirrors the underlying tool call's status.
    pub(crate) status: yalda::acp_channel::ToolCallStatus,
}

/// Heuristic classifier (§25). v1: anything with `kind == ToolKind::Other`
/// AND a `title` prefix in [`SUBAGENT_TOOL_NAMES`] is treated as a sub-
/// agent. (The spec calls the matching field "name"; ACP names it
/// `title` — same meaning, the user-facing label for the tool call.)
/// Returns the freshly-constructed `SubAgent`, or `None` if the tool call
/// doesn't match.
pub(crate) fn classify_subagent(tc: &yalda::acp_channel::ToolCall) -> Option<SubAgent> {
    use yalda::acp_channel::ToolKind;
    if tc.kind != ToolKind::Other {
        return None;
    }
    let title = tc.title.as_str();
    if !SUBAGENT_TOOL_NAMES
        .iter()
        .any(|prefix| title.starts_with(prefix))
    {
        return None;
    }
    let label = if title.is_empty() {
        "subagent".to_string()
    } else {
        title.to_string()
    };
    Some(SubAgent {
        tool_call_id: ToolCallKey::from_id(&tc.tool_call_id),
        label,
        status: tc.status,
    })
}

/// Per-line metadata that the Worksheet gutter reads to label each line.
/// Stored in `editor.metadata::<TurnId>()` keyed by `LineAnchor`, so the
/// tag follows the line through inserts, deletes, and inter-block
/// annotations (spec-agent-window.md §11, §E2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnId {
    /// LLM output, turn N. Gutter prints `N` in a dim accent color.
    Llm(usize),
    /// User input frozen as part of turn n's prompt. Gutter prints `Un`.
    User(usize),
    /// Tool-call block originating from turn N. Gutter prints `Tn`.
    /// Lives on the anchor line of a `ToolGroup` flat-item.
    Tool(usize),
    /// Yalda-local lifecycle notice (attach/detach/disconnect/permission/
    /// force-restart, retry `Notice`s). NOT agent-authored: it carries no
    /// turn number, never emits a Claude `TurnHeader`, renders with a blank
    /// gutter, and is excluded from agent-turn numbering and the live≡replay
    /// parity contract (which is defined over `{User, Llm, Tool}` only —
    /// Finding 5, INV-3 / Constraint 5). Kept out of `append_llm_chunk`'s
    /// `Llm(k)` lane so a notice can never seed or mis-attribute a turn.
    System,
}

/// The role a header-owning turn maps to. A header-owning turn is exactly
/// `{Llm, User}`; `Tool` and `System` turns anchor ToolGroups / lifecycle
/// notices and never emit a `TurnHeader`. Encoding this as a returned
/// `Option<HeaderRole>` (rather than an `unreachable!()` arm) makes "Tool
/// has no header" a compiler-checked total mapping (Finding 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderRole {
    Claude,
    User,
}

impl HeaderRole {
    /// Total mapping from a turn id to the header it owns (if any).
    /// `Tool`/`System` -> `None`; `Llm` -> `Claude`; `User` -> `User`.
    pub(crate) fn from_turn(tid: TurnId) -> Option<HeaderRole> {
        match tid {
            TurnId::Llm(_) => Some(HeaderRole::Claude),
            TurnId::User(_) => Some(HeaderRole::User),
            TurnId::Tool(_) | TurnId::System => None,
        }
    }

    pub(crate) fn into_turn_role(self) -> TurnRole {
        match self {
            HeaderRole::Claude => TurnRole::Claude,
            HeaderRole::User => TurnRole::User,
        }
    }
}

/// Standalone input editor used when `InputMode == Chatbox`. Has its own
/// document, cursor, undo stack, and modal state (§16). The chatbox is
/// dropped on a `Chatbox → Worksheet` toggle (§6) and re-constructed empty
/// on a `Worksheet → Chatbox` toggle (§7) — undo history doesn't survive
/// the round trip; the previous draft is recoverable as transcript
/// content if the user already submitted.
pub(crate) struct Chatbox {
    pub(crate) editor: Editor,
    pub(crate) mode: EditMode,
    pub(crate) scroll_handle: ScrollHandle,
}

impl Chatbox {
    pub(crate) fn new() -> Self {
        Self {
            editor: Editor::new(String::new(), std::path::PathBuf::from("*chatbox*")),
            mode: EditMode::Insert,
            scroll_handle: ScrollHandle::new(),
        }
    }

    pub(crate) fn text(&self) -> String {
        self.editor.document().full_text()
    }
}

/// The agent turn lifecycle as one explicit state (Finding 9). Replaces the
/// loose `(awaiting_reply, turn_started, last_event_at, stop_requested_at)`
/// quadruple whose valid combinations were unwritten convention — e.g.
/// `awaiting_reply=false` with `stop_requested_at=Some` was structurally
/// reachable but meaningless. Each transition site (submit, on-event,
/// finalize, reset_for_replay, the Stop handler) is now a total function over
/// this enum, and the thinking indicator / Stop-escalation read the variant
/// rather than probing flag combinations.
///
/// Invariants made unrepresentable: a `since`/`escalated` stop marker can only
/// exist while the turn is in flight (it lives *inside* `StopRequested`), and
/// the elapsed/quiet timers (`started`/`last_event`) only exist while awaiting.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TurnPhase {
    /// No turn in flight. The footer shows no spinner; Stop is a no-op.
    Idle,
    /// A prompt was sent and we're streaming the reply. `started` drives the
    /// elapsed timer; `last_event` drives the "quiet for M:SS" stall reading.
    Awaiting {
        started: std::time::Instant,
        last_event: std::time::Instant,
    },
    /// The user pressed Stop once; a graceful `session/cancel` is pending but
    /// the turn is still in flight (timers keep running). A second Stop while
    /// in this state escalates to a hard kill + resume — captured by setting
    /// `escalated` on the way into `force_restart_agent`. Carries the same
    /// `started`/`last_event` so the indicator keeps reading correctly.
    StopRequested {
        started: std::time::Instant,
        last_event: std::time::Instant,
        // kept for API symmetry / future use
        #[allow(dead_code)]
        since: std::time::Instant,
        escalated: bool,
    },
}

impl TurnPhase {
    /// True while a reply is in flight (Awaiting or StopRequested). Drives the
    /// thinking indicator, the Stop button's visibility, and `any_agent_awaiting`.
    pub(crate) fn is_awaiting(&self) -> bool {
        !matches!(self, TurnPhase::Idle)
    }

    /// When the in-flight turn started (elapsed-timer source), or `None` when idle.
    pub(crate) fn turn_started(&self) -> Option<std::time::Instant> {
        match self {
            TurnPhase::Idle => None,
            TurnPhase::Awaiting { started, .. } | TurnPhase::StopRequested { started, .. } => {
                Some(*started)
            }
        }
    }

    /// Last inbound reply activity (quiet-clock source), or `None` when idle.
    pub(crate) fn last_event_at(&self) -> Option<std::time::Instant> {
        match self {
            TurnPhase::Idle => None,
            TurnPhase::Awaiting { last_event, .. }
            | TurnPhase::StopRequested { last_event, .. } => Some(*last_event),
        }
    }

    /// True once the user pressed Stop for the in-flight turn (a graceful
    /// cancel is pending). A second Stop in this state escalates.
    pub(crate) fn stop_requested(&self) -> bool {
        matches!(self, TurnPhase::StopRequested { .. })
    }

    /// Refresh the quiet-clock for the in-flight turn (any inbound event). A
    /// no-op when idle. Preserves a pending Stop request.
    pub(crate) fn note_event(&mut self, now: std::time::Instant) {
        match self {
            TurnPhase::Idle => {}
            TurnPhase::Awaiting { last_event, .. }
            | TurnPhase::StopRequested { last_event, .. } => *last_event = now,
        }
    }

    /// Enter the awaiting state on a successful submit. Clears any prior Stop.
    pub(crate) fn begin(now: std::time::Instant) -> Self {
        TurnPhase::Awaiting {
            started: now,
            last_event: now,
        }
    }

    /// Record the user's first Stop (graceful cancel pending). No-op if not
    /// awaiting, or already stop-requested (idempotent on repeat from a stale
    /// call path — escalation is decided by the handler before this runs).
    pub(crate) fn request_stop(&mut self, now: std::time::Instant) {
        if let TurnPhase::Awaiting {
            started,
            last_event,
        } = *self
        {
            *self = TurnPhase::StopRequested {
                started,
                last_event,
                since: now,
                escalated: false,
            };
        }
    }

    /// Mark the pending Stop as escalated (a second Stop → hard kill + resume).
    /// Only meaningful while `StopRequested`; a no-op otherwise. The caller
    /// then drives `force_restart_agent`, which returns the phase to `Idle`.
    pub(crate) fn escalate(&mut self) {
        if let TurnPhase::StopRequested { escalated, .. } = self {
            *escalated = true;
        }
    }

    /// Whether a pending Stop has been escalated to a hard kill (second Stop).
    // kept for API symmetry / future use
    #[allow(dead_code)]
    pub(crate) fn is_escalated(&self) -> bool {
        matches!(
            self,
            TurnPhase::StopRequested {
                escalated: true,
                ..
            }
        )
    }
}

/// The memoized `render_agent` view-model caches, grouped into one owner
/// instead of six sibling fields on `AgentState` (A.7 god-struct shrink).
/// Every field is derived from the structural inputs captured in
/// `view_model_fp`; none are serialized (runtime-only). This owner owns the
/// S1 cache decision via [`cached`](Self::cached) + [`store`](Self::store):
/// the rebuild between them runs at the call site on `&mut AgentState` (it
/// needs `tools`/`editor` and writes `block_cache`), which can't be borrowed
/// while a method holds `&mut self` here — hence the split rather than one
/// closure-taking `memoize`.
/// A detected frozen block range `(start, end)` and its parse result —
/// `None` = rejected by `parse_block_range`, renders as plain source lines
/// (INV-10). The `Rc` is shared with `AgentViewModel::block_cache` and bumped
/// (never deep-cloned) into `FlatItem::Block` on each rebuild.
pub(crate) type ResolvedBlock = ((usize, usize), Option<std::rc::Rc<RenderedBlock>>);

#[derive(Default)]
pub(crate) struct AgentViewModel {
    /// Parsed-block cache keyed by a 64-bit hash of the range's SOURCE LINES,
    /// not its `(start, end)` position: a streamed chunk inserts lines and
    /// shifts every later range, so position keys missed on every chunk and
    /// the entire frozen transcript re-parsed (pulldown-cmark + syntect) per
    /// chunk. Content keys survive shifts; identical ranges share one `Rc`.
    /// A 64-bit collision would render the wrong block — per-session and
    /// transient, the same accepted risk as `view_model_fp`.
    pub(crate) block_cache: std::collections::HashMap<u64, std::rc::Rc<RenderedBlock>>,
    /// Frozen line count when `block_cache` was last (re)validated.
    pub(crate) block_cache_frozen_count: usize,
    /// The detected ranges resolved to their parsed blocks (`None` = rejected
    /// by `parse_block_range`, renders as plain source lines per INV-10).
    /// Sorted by start, disjoint. Rebuilt only when the frozen line count
    /// changes; every other S1 rebuild (each worksheet keystroke) just walks
    /// it — the rebuild previously deep-cloned EVERY parsed block into fresh
    /// per-rebuild maps, the dominant per-keystroke cost on large transcripts.
    pub(crate) resolved_blocks: Vec<ResolvedBlock>,
    /// Memoized flat-items list. On a `view_model_fp` hit `render_agent`
    /// reuses this `Rc` verbatim and skips the whole rebuild (gutter scan,
    /// tool-anchor resolution, flat build, blank-collapse).
    pub(crate) flat_items_cache: std::rc::Rc<Vec<FlatItem>>,
    /// Memoized per-line gutter tags, paired with `flat_items_cache`.
    pub(crate) gutter_cache: std::rc::Rc<Vec<Option<TurnId>>>,
    /// Reverse index: doc line → its position in `flat_items_cache`. Derived
    /// from the canonical `flat_items` at build time (single source of truth),
    /// so cursor-reveal scroll math is an O(1) array lookup that can never
    /// drift from what's actually rendered. This is what makes Worksheet
    /// typing O(changed) instead of O(transcript) — see ADR-0020 / INV-RV.
    pub(crate) line_to_item_cache: std::rc::Rc<Vec<u32>>,
    /// Fingerprint of the structural inputs the cached view-model was built
    /// from. `None` = never built (forces a rebuild on first render).
    pub(crate) view_model_fp: Option<u64>,
    /// Bumped on every view-model rebuild. Lets tests assert a fingerprint
    /// hit reused the cache (seq unchanged) vs. forced a rebuild.
    pub(crate) view_model_seq: u64,
}

impl AgentViewModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Cache hit: return the memoized `Rc`s iff `fp` matches the fingerprint
    /// the cache was built at; `None` = miss → the caller rebuilds and calls
    /// [`store`](Self::store). The S1 cache decision is split into
    /// `cached` + `store` (rather than one closure-taking `memoize`) so this
    /// owner owns the decision: the rebuild runs at the call site on
    /// `&mut AgentState` (it reads `tools`/`editor` and writes
    /// `self.view_model.block_cache`), which cannot be borrowed while a method
    /// holds `&mut self` on the nested `view_model`.
    // type alias would hurt readability here more than help
    #[allow(clippy::type_complexity)]
    pub(crate) fn cached(
        &self,
        fp: u64,
    ) -> Option<(std::rc::Rc<Vec<FlatItem>>, std::rc::Rc<Vec<Option<TurnId>>>)> {
        (self.view_model_fp == Some(fp))
            .then(|| (self.flat_items_cache.clone(), self.gutter_cache.clone()))
    }

    /// Store a freshly-rebuilt view-model, stamp the fingerprint, and bump
    /// `view_model_seq`. Call exactly once per [`cached`](Self::cached) miss.
    pub(crate) fn store(
        &mut self,
        fp: u64,
        flat_items: Vec<FlatItem>,
        gutter: Vec<Option<TurnId>>,
        line_to_item: Vec<u32>,
    ) -> (std::rc::Rc<Vec<FlatItem>>, std::rc::Rc<Vec<Option<TurnId>>>) {
        #[cfg(test)]
        {
            VIEW_MODEL_REBUILDS.with(|n| n.set(n.get() + 1));
        }
        let flat_rc = std::rc::Rc::new(flat_items);
        let gutter_rc = std::rc::Rc::new(gutter);
        self.flat_items_cache = flat_rc.clone();
        self.gutter_cache = gutter_rc.clone();
        self.line_to_item_cache = std::rc::Rc::new(line_to_item);
        self.view_model_fp = Some(fp);
        self.view_model_seq = self.view_model_seq.wrapping_add(1);
        (flat_rc, gutter_rc)
    }

    /// O(1) flat-item index for `doc_line`, clamped into range. This is the
    /// ONLY supported way to compute a cursor-reveal scroll target — it reads
    /// the build-time reverse index so callers never re-derive (and never
    /// re-scan the transcript) the line→item mapping. Empty cache → 0.
    pub(crate) fn item_for_line(&self, doc_line: usize) -> usize {
        let m = &self.line_to_item_cache;
        m.get(doc_line)
            .or_else(|| m.last())
            .copied()
            .unwrap_or(0) as usize
    }
}

/// Build the doc-line → flat-item reverse index from the CANONICAL final
/// `flat_items`. Single source of truth: it reads the very list the renderer
/// virtualises, so a cursor-reveal target can never disagree with what's on
/// screen. O(items + collapsed block lines); runs only on a view-model
/// rebuild (cache miss), never per keystroke.
fn build_line_to_item(flat_items: &[FlatItem], resolved: &[ResolvedBlock], line_count: usize) -> Vec<u32> {
    let mut map = vec![u32::MAX; line_count];
    // PARSED ranges became `FlatItem::Block`, emitted in ascending-start order
    // — the same order they appear in `flat_items` — so a single forward
    // cursor pairs each Block item with its source range.
    let parsed: Vec<(usize, usize)> = resolved
        .iter()
        .filter_map(|((s, e), b)| b.as_ref().map(|_| (*s, *e)))
        .collect();
    let mut bi = 0usize;
    for (p, item) in flat_items.iter().enumerate() {
        match item {
            FlatItem::Line(idx) => {
                if let Some(slot) = map.get_mut(*idx) {
                    *slot = p as u32;
                }
            }
            FlatItem::Block(_) => {
                if let Some(&(s, e)) = parsed.get(bi) {
                    for l in s..e.min(line_count) {
                        map[l] = p as u32;
                    }
                    bi += 1;
                }
            }
            // ToolGroup/TurnHeader/ThinkingIndicator own no doc line.
            _ => {}
        }
    }
    debug_assert!(
        bi == parsed.len(),
        "block items ({bi}) did not consume all parsed ranges ({})",
        parsed.len()
    );
    // Lines with no item of their own (blank lines collapsed away by the
    // blank-collapse pass) inherit the previous mapped item, so a cursor that
    // lands on one reveals the nearest rendered row, not the top.
    let mut last = 0u32;
    for v in map.iter_mut() {
        if *v == u32::MAX {
            *v = last;
        } else {
            last = *v;
        }
    }
    map
}

/// S1 view-model rebuild — the `cached()` miss path of `render_agent`: the
/// gutter-tag scan, tool-anchor resolution, frozen-block partition, flat
/// build and blank-collapse. A top-level fn (not inlined in `render_agent`)
/// so the per-keystroke cost is testable headlessly: `lines` /
/// `frozen_ranges` are plain data and `c` comes from
/// `AgentState::new_for_test` — no Window or GpuiApp required.
pub(crate) fn rebuild_agent_view_model(
    c: &mut AgentState,
    lines: &[String],
    frozen_ranges: &[(usize, usize)],
    frozen_line_count: usize,
    theme: &Theme,
    view_model_fp: u64,
) -> (std::rc::Rc<Vec<FlatItem>>, std::rc::Rc<Vec<Option<TurnId>>>) {
    // Per-line gutter tag, sourced from the editor's `TurnId` metadata
    // keyed by `LineAnchor` (spec §11, §E2). Lines without a tag yet
    // (currently-editable, not yet swept by Submit) render as a blank
    // gutter. Lines whose anchor hasn't been allocated count as
    // untagged — happens for editable lines the user just typed.
    // Hoist the metadata view out of the per-line loop: `metadata::<TurnId>()`
    // does a HashMap-by-TypeId lookup and builds a fresh view each call, so
    // calling it once per line was O(n) view constructions per frame. Build
    // it once and reuse it across all lines.
    let gutter_tag_per_line: Vec<Option<TurnId>> = {
        let turn_meta = c.editor.metadata::<TurnId>();
        (0..lines.len())
            .map(|i| {
                c.editor
                    .anchor_for_line_opt(i)
                    .and_then(|a| turn_meta.get(a).copied())
            })
            .collect()
    };

    // ============ Virtualised list build ============
    //
    // Frozen (agent) content is parsed into RenderedBlocks so that
    // tables, code blocks, headings, and lists display properly.
    // Editable (user) content stays as per-line rendering with
    // cursor/selection support.

    // Build "tool calls anchored at line N" lookup, grouped by
    // anchor line. All calls at the same anchor form one ToolGroup.
    // Anchors are opaque `LineAnchor` ids (spec §E1); resolve via the
    // editor each paint. Anchors whose line got consumed by a delete
    // fall back to EOF so the tool block still renders, just at the
    // tail of the transcript.
    let eof_line = c.editor.document().line_count().saturating_sub(1);
    let mut tools_at_line: std::collections::HashMap<usize, Vec<ToolCallKey>> =
        std::collections::HashMap::new();
    for id in &c.tools.order {
        if let Some(&anchor) = c.tools.anchor.get(id) {
            let line = c.editor.line_for_anchor(anchor).unwrap_or(eof_line);
            tools_at_line.entry(line).or_default().push(id.clone());
        }
    }

    // Detect tables and fenced code blocks in frozen content for
    // block-level rendering; everything else stays line-by-line.
    // Re-validated only when the frozen line count changes (a
    // streamed chunk / submit) — a worksheet keystroke in the
    // editable tail leaves all of this untouched. Parses are
    // reused by CONTENT (see `AgentViewModel::block_cache`), so a
    // chunk that shifts every range only re-parses ranges whose
    // text actually changed. Rejected ranges resolve to `None`
    // and render as their source lines (Finding 10, INV-10).
    if frozen_line_count != c.view_model.block_cache_frozen_count {
        let block_ranges = detect_block_ranges(lines, frozen_ranges);
        let mut new_cache: std::collections::HashMap<u64, std::rc::Rc<RenderedBlock>> =
            std::collections::HashMap::new();
        let mut resolved: Vec<ResolvedBlock> = Vec::with_capacity(block_ranges.len());
        for &(start, end) in &block_ranges {
            let key = block_content_hash(&lines[start..end]);
            let parsed = if let Some(block) = c.view_model.block_cache.get(&key) {
                Some(block.clone())
            } else if let BlockParse::Parsed(block) = parse_block_range(lines, start, end, theme) {
                Some(std::rc::Rc::new(block))
            } else {
                None
            };
            if let Some(block) = &parsed {
                new_cache.insert(key, block.clone());
            }
            resolved.push(((start, end), parsed));
        }
        c.block_ranges = block_ranges;
        c.view_model.block_cache = new_cache;
        c.view_model.resolved_blocks = resolved;
        c.view_model.block_cache_frozen_count = frozen_line_count;
    }

    // The block/line partition reads `resolved_blocks` directly —
    // sorted, disjoint, parsed-or-None per range — via a cursor
    // in the flat loop below. (This used to materialize per-
    // rebuild lookup maps, deep-cloning every parsed block on
    // every rebuild — the dominant per-keystroke cost on large
    // transcripts.)
    let resolved = &c.view_model.resolved_blocks;
    let mut next_block = 0usize;

    // Flat ordering: TurnHeader?, line_0, tool_group_at[0], line_1, …
    // Lines inside a detected block range are replaced by one
    // FlatItem::Block at the range start; interior lines are skipped.
    // TurnHeader items are inserted at turn boundaries (role changes).
    let mut flat_items: Vec<FlatItem> = Vec::with_capacity(lines.len() * 2);
    let mut prev_turn: Option<TurnId> = None;
    for line_idx in 0..lines.len() {
        // Insert a TurnHeader whenever the dominant turn changes.
        let cur_turn = gutter_tag_per_line.get(line_idx).copied().flatten();
        // Tools and yalda-local System notices don't get their own header
        // and don't break the current turn run — a notice landing mid-turn
        // must not re-emit a Claude header (Finding 5, INV-3). The
        // total `HeaderRole::from_turn` returns `None` for those, so the
        // header-owning turn set `{Llm, User}` is enforced by the type
        // rather than an `unreachable!()` arm (Finding 6).
        if let Some(tid) = cur_turn {
            if let Some(role) = HeaderRole::from_turn(tid) {
                let changed = match prev_turn {
                    Some(prev) => prev != tid,
                    None => true,
                };
                if changed {
                    flat_items.push(FlatItem::TurnHeader {
                        role: role.into_turn_role(),
                    });
                    prev_turn = Some(tid);
                }
            }
        } else if prev_turn.is_some() {
            // Editable (untagged) lines after frozen content = user input.
            // Suppress the "You" header when the editable tail is all
            // blank — in Chatbox mode the compose area is separate, so
            // an empty transcript tail is just whitespace, not a turn.
            let remaining_non_empty = (line_idx..lines.len()).any(|j| !lines[j].trim().is_empty());
            if remaining_non_empty {
                flat_items.push(FlatItem::TurnHeader {
                    role: TurnRole::User,
                });
            }
            prev_turn = None;
        }

        // Advance the resolved-range cursor past ranges that
        // ended, then place this line: a PARSED range emits one
        // Block (an Rc bump) at its start and subsumes its
        // interior; an unparsed range or plain region emits Lines.
        while next_block < resolved.len() && resolved[next_block].0.1 <= line_idx {
            next_block += 1;
        }
        match resolved.get(next_block) {
            Some(((start, end), Some(block))) if line_idx >= *start && line_idx < *end => {
                if line_idx == *start {
                    flat_items.push(FlatItem::Block(block.clone()));
                }
            }
            _ => flat_items.push(FlatItem::Line(line_idx)),
        }
        // Tool groups anchored inside a block range still render.
        if let Some(ids) = tools_at_line.get(&line_idx) {
            flat_items.push(FlatItem::ToolGroup {
                anchor_line: line_idx,
                ids: ids.clone(),
            });
        }
    }

    // Collapse blank lines: (a) strip blank frozen (Claude) Lines
    // entirely — they're protocol padding with no visual purpose,
    // (b) strip blank Lines adjacent to ToolGroup / TurnHeader /
    // Block items, and (c) collapse runs of consecutive blank
    // user Lines to at most one.
    {
        let is_blank_line = |item: &FlatItem| -> bool {
            matches!(item, FlatItem::Line(idx) if lines.get(*idx).is_none_or(|s| s.trim().is_empty()))
        };
        let is_frozen_line = |item: &FlatItem| -> bool {
            matches!(item, FlatItem::Line(idx) if frozen_ranges.iter().any(|&(s, e)| *idx >= s && *idx < e))
        };
        let is_structural = |item: &FlatItem| -> bool {
            matches!(
                item,
                FlatItem::ToolGroup { .. } | FlatItem::TurnHeader { .. } | FlatItem::Block(_)
            )
        };
        let mut keep = vec![true; flat_items.len()];
        for i in 0..flat_items.len() {
            if !is_blank_line(&flat_items[i]) {
                continue;
            }
            // Blank frozen lines are always stripped — they're just
            // anchor padding inserted by the ACP splice logic.
            if is_frozen_line(&flat_items[i]) {
                keep[i] = false;
                continue;
            }
            // Drop blank line if adjacent to a structural item.
            let adj_structural = (i > 0 && is_structural(&flat_items[i - 1]))
                || (i + 1 < flat_items.len() && is_structural(&flat_items[i + 1]));
            if adj_structural {
                keep[i] = false;
                continue;
            }
            // Collapse consecutive blanks to one.
            if i > 0 && is_blank_line(&flat_items[i - 1]) && keep[i - 1] {
                keep[i] = false;
            }
        }
        let mut j = 0;
        // index drives an in-place compaction (keep[i] gates flat_items.swap(i, j))
        #[allow(clippy::needless_range_loop)]
        for i in 0..flat_items.len() {
            if keep[i] {
                flat_items.swap(i, j);
                j += 1;
            }
        }
        flat_items.truncate(j);
    }

    // Coalesce a contiguous run of tool calls into ONE collapsible group
    // so a long sequence (grep → grep → edit → read → …) doesn't flood the
    // transcript. The blank anchor lines between adjacent tool calls were
    // already stripped by the blank-collapse pass above, so a run shows up
    // as directly-adjacent `ToolGroup`s; merge their ids into the first.
    // Any prose Line, Block, or TurnHeader between two runs breaks the run
    // (those are real content), so tool calls separated by agent text stay
    // in separate groups. The merged group renders as a typed-count header
    // (e.g. "4 grep, 3 edit, 7 read"), collapsed by default.
    {
        let mut merged: Vec<FlatItem> = Vec::with_capacity(flat_items.len());
        for item in flat_items.drain(..) {
            if let FlatItem::ToolGroup { ids, .. } = &item
                && let Some(FlatItem::ToolGroup { ids: prev_ids, .. }) = merged.last_mut()
            {
                prev_ids.extend(ids.iter().cloned());
                continue;
            }
            merged.push(item);
        }
        flat_items = merged;
    }

    // Thinking indicator at the tail while waiting for Claude.
    if c.turn_phase.is_awaiting() {
        flat_items.push(FlatItem::ThinkingIndicator);
    }

    // Derive the cursor-reveal reverse index from the FINAL flat_items (after
    // blank-collapse, tool-group merge, and the tail indicator) so it matches
    // the rendered list exactly.
    let line_to_item = build_line_to_item(&flat_items, resolved, lines.len());

    c.view_model
        .store(view_model_fp, flat_items, gutter_tag_per_line, line_to_item)
}

/// State held while the user is conversing with an ACP-attached Claude
/// agent. The transcript lives in an in-memory `Editor` (no on-disk file);
/// Claude's replies are spliced in as frozen lines via the same lock-and-
/// advance pattern the TUI uses (`app::claude::append_to_claude_buffer`),
/// so the user can keep typing inline edits between turns.
pub(crate) struct AgentState {
    /// Editor over the chat transcript. `frozen_lines` mark Claude's turns;
    /// the editable region below `lockable_through_line` is the user's
    /// pending draft.
    pub(crate) editor: Editor,
    /// Live ACP connection. `None` while attaching, after a worker crash,
    /// or when the user pre-emptively detached.
    pub(crate) channel: Option<AcpChannelClient>,
    /// Receiver for an in-flight ACP attach. The attach runs on a
    /// background `std::thread` because `AcpChannelClient::spawn` blocks
    /// on the worker's initialize handshake — running it on the GPUI
    /// foreground executor would freeze the UI. The pump task polls this
    /// each tick; when it resolves, the result moves to `channel` and
    /// `attach_pending` clears.
    pub(crate) attach_pending: Option<std::sync::mpsc::Receiver<std::io::Result<AcpChannelClient>>>,
    pub(crate) mode: EditMode,
    pub(crate) keybinds: KeybindManager,
    /// Virtualized list state for the claude transcript. We render
    /// every doc-line + tool-block as an item in a `gpui::list` —
    /// non-uniform-height list that only paints visible rows. Without
    /// this, render scaled with total transcript length and made input
    /// laggy on long sessions because every cx.notify re-tokenized
    /// every line for word-wrap. `ListAlignment::Bottom` gives the
    /// chat-style initial pin. The `follow_output` flag (maintained by
    /// the scroll handler) gates pump-driven auto-scroll so the user
    /// can scroll up to read history without being yanked to the bottom.
    pub(crate) list_state: gpui::ListState,
    /// Total number of items currently registered in `list_state`. We
    /// track it separately so we can splice in new items as the
    /// buffer grows without paying for a full reset.
    pub(crate) list_item_count: usize,
    /// `edit_seq` at the last `reconcile_list` call that actually touched
    /// the list. Used to detect mid-line appends (count unchanged but
    /// content grew) so we can invalidate the tail item's cached height.
    pub(crate) last_reconciled_edit_seq: u64,
    /// Footer status line — attach result, send result, error. Cleared on
    /// the next non-Ctrl keystroke so it persists for at least one frame.
    pub(crate) status: Option<SharedString>,
    /// The turn lifecycle as one explicit state (Finding 9). `Idle` between
    /// turns; `Awaiting` while streaming a reply (carrying the elapsed-timer
    /// `started` and the quiet-clock `last_event`); `StopRequested` once the
    /// user pressed Stop (a graceful cancel pending, a second Stop escalates).
    /// Replaces the prior `(awaiting_reply, turn_started, last_event_at,
    /// stop_requested_at)` quadruple — see `TurnPhase`.
    pub(crate) turn_phase: TurnPhase,
    /// The turn-number state machine (Findings 3 & 13, INV-3/INV-4) — the
    /// **single owner** of `k`. Holds `last_seen` (settled live turns; the pump
    /// compares the live counter against it each tick, and when it ticks up the
    /// in-flight turn just ended → finalize the buffer + return the phase to
    /// `Idle`) and `replay_turn` (the replay cursor). On `session/load` the
    /// agent re-emits the whole prior conversation as one burst of
    /// `UserMessage`/`Chunk` events with no per-turn prompt-response to advance
    /// `last_seen`; without the cursor the replayed history collapses into one
    /// `TurnId::Llm(1)`. Instead each replayed `UserMessage` boundary steps the
    /// cursor so chunks attach to the *next* `Llm(k)` —
    /// `User(1),Llm(1),User(2),Llm(2)` — and `current_turn()` prefers it when
    /// non-zero so live submit and replay share one source of `k`.
    /// `ReplayComplete` folds the cursor back into `last_seen` and zeroes it.
    /// (Was two loose `usize` fields reconstructed into a temporary `ReplayTurns`
    /// on every read and copied back out on every mutation — now owned directly.)
    pub(crate) replay_turns: yalda::acp_channel::ReplayTurns,
    /// `edit_seq` at which the tail was last revealed by the follow-scroll
    /// (F4, INV-13). The render-time re-reveal historically fired only when
    /// the flat-item COUNT changed, so a chunk that grows the last line/block
    /// without adding a row (agent prose before a `\n`, or a streaming code
    /// fence) was skipped and the freshly grown tail fell below the fold.
    /// Tracking the last-scrolled `edit_seq` lets the reveal fire on content
    /// growth regardless of count delta, while still de-duping idle frames
    /// (same `edit_seq` ⇒ no re-scroll). `u64::MAX` = never scrolled.
    pub(crate) last_scrolled_edit_seq: u64,
    /// The live tool-call state cluster (calls + order + anchors + expanded
    /// set) — one owner instead of four sibling fields. See [`ToolCalls`].
    pub(crate) tools: ToolCalls,
    /// Line ranges `(start, end)` that are rendered as structural blocks
    /// (tables, fenced code blocks) instead of line-by-line. Updated each
    /// render pass.
    pub(crate) block_ranges: Vec<(usize, usize)>,
    /// Set by a Worksheet keystroke to request "reveal the cursor's line in
    /// the virtualised list on the next render". The render path consumes it
    /// via the O(1) `AgentViewModel::item_for_line` lookup (INV-RV) — the key
    /// handler itself does NO transcript-sized work, which is what keeps
    /// Worksheet typing flat as the session grows (ADR-0020).
    pub(crate) pending_reveal_cursor: bool,
    /// The memoized `render_agent` view-model caches (block cache, flat-items,
    /// gutter tags, fingerprint, seq) — one owner instead of six sibling
    /// fields (A.7). See [`AgentViewModel`].
    pub(crate) view_model: AgentViewModel,
    /// Cached per-line transcript text (trimmed + tab-expanded) used by
    /// `render_agent`. Perf: building this `Vec<String>` allocates a String
    /// per transcript line on every `cx.notify()` (cursor blink, cross-tile
    /// wakeups, every streamed chunk), an O(L) cost regardless of how few
    /// lines changed. Cache it keyed on `edit_seq` so unchanged frames reuse
    /// the prior vec instead of re-extracting + re-allocating the whole doc.
    pub(crate) lines_cache: std::rc::Rc<Vec<String>>,
    /// `edit_seq` the `lines_cache` was built at; `u64::MAX` = never built.
    pub(crate) lines_cache_seq: u64,
    /// Incremental highlight cache for the transcript. Re-highlights only the
    /// lines that changed between renders instead of the whole buffer every
    /// `cx.notify()`. Bypassed when `YALDA_HL_CACHE=0`.
    pub(crate) highlight_cache: HighlightCache,
    /// The active input surface (§4). The `Chatbox` draft editor lives INSIDE
    /// the `Chatbox` variant — make-illegal-states-unrepresentable, so the old
    /// "`chatbox` is `Some` iff `input_mode == Chatbox`" invariant (two
    /// hand-synced fields) is now enforced by the type. New sessions start at
    /// `Chatbox`; `Ctrl-Alt-Enter` toggles (§5).
    pub(crate) input_surface: InputSurface,
    /// Last-seen full snapshot of the agent's plan. Updated on every ACP
    /// `Plan` notification (which carries a complete plan, not a delta —
    /// see spec-agent-window.md §21). Consumed by the Tasklist sidebar.
    pub(crate) current_plan: Option<yalda::acp_channel::Plan>,
    /// Last-seen session mode id from the agent (Claude Code's `default` /
    /// `plan` / `learn`, etc.). Distinct from the permission mode on
    /// `AcpChannelClient`. Surfaced by the Status Strip.
    pub(crate) agent_mode: Option<yalda::acp_channel::SessionModeId>,
    /// Last-seen active model id from the agent (e.g. `claude-opus-4-8`),
    /// sourced from the `session/new` response's `config_options`. Surfaced
    /// by the Status Strip in place of the old best-effort model label.
    pub(crate) agent_model: Option<String>,
    /// The session's permission mode, as session state sourced from the
    /// server. In session-server mode the agent/channel live in the server
    /// (not the GUI), so `channel` is `None` and the live `AcpChannelClient`
    /// permission flag is unreachable from here — the authoritative value
    /// arrives on `SessionInfo.permission_mode` and is mirrored into this
    /// field whenever a slot learns its `SessionInfo`. Rendered by the Status
    /// Strip badge and cycled by `cycle_claude_permission_mode`. Initialized
    /// to `DEFAULT_PERMISSION_MODE` and overwritten the moment the server
    /// reports the real value. For the legacy direct-spawn path the channel is
    /// still the live authority; this field is kept in sync alongside it.
    pub(crate) permission_mode: yalda::acp_channel::PermissionMode,
    /// Last-seen usage snapshot (tokens used/total, cost). Populated only
    /// when the upstream `unstable_session_usage` feature is on; otherwise
    /// stays `None` and the Status Strip omits these fields per §30.
    pub(crate) usage: Option<yalda::acp_channel::UsageSnapshot>,
    /// `tool_call_id` of the currently focused sub-agent. When `Some`, the
    /// main transcript area swaps to show that sub-agent's content instead
    /// of the root agent's (§27). Keyed by a stable `ToolCallKey` rather
    /// than a positional index so it survives any reordering of the derived
    /// `subagents()` list (ADR-0006 quick win #1).
    ///
    /// The sub-agent list itself is NOT stored — see `subagents()`, which
    /// derives it from `tool_call_order` + `tool_calls`.
    pub(crate) focused_subagent: Option<ToolCallKey>,
    /// Whether auto-scroll should follow new output. Defaults to `true`
    /// (pinned to bottom). Set to `false` when the user scrolls up in the
    /// transcript, re-enabled when they scroll back to the bottom or send
    /// a new message. Shared with the ListState scroll handler via Rc.
    pub(crate) follow_output: std::rc::Rc<std::cell::Cell<bool>>,
    /// Whether the Tasklist sidebar is open (§24).
    pub(crate) tasklist_open: bool,
    /// Whether the Subagents sidebar is open (§28).
    pub(crate) subagents_open: bool,
    /// True when this session is managed by the session server (client/server
    /// mode). False when the GUI owns the ACP subprocess directly (legacy).
    /// Checked by the status strip and anywhere that needs to distinguish
    /// the two paths from within `AgentState` alone.
    pub(crate) server_managed: bool,
    /// Order-independent reconciler for user-turn insertions — the single
    /// authority that de-dupes the three sites a user turn can be announced
    /// from (optimistic submit, server `UserPrompt`, agent `UserMessage`).
    /// Replaces the position-dependent `document_trimmed_end_ends_with`
    /// heuristic that double-rendered input whenever content streamed in
    /// between the optimistic echo and its stream copy. See `agent_transcript`.
    pub(crate) reconciler: yalda::agent_transcript::UserTurnReconciler,
    /// User-turn `k`s inserted since the last `reset_for_replay` generation.
    /// The M3 runtime tripwire asserts a `k` is never inserted twice — a
    /// double-render reuses a `k`, so this turns a silent visual regression
    /// into a loud, located failure. Scoped per generation (cleared on
    /// transcript wipe) so a reconnect's `k`-restart is not a false positive.
    pub(crate) user_turn_ks: std::collections::HashSet<usize>,
    /// The channel generation this transcript is rebaselined to (spec §4).
    /// `0` until the first `AgentEvent` carrying a generation is folded. The
    /// uniform rebaseline rule fires when a folded event's `generation` is
    /// strictly greater than this value: `reset_for_replay` runs FIRST, then
    /// this field advances. Cleared to `0` by `reset_for_replay` so a
    /// re-attach replaying gen-N events rebaselines exactly once.
    pub(crate) generation: u64,
    /// Idempotent-finalize ledger keyed on `(generation, turn)` (spec §7/H5).
    /// `finalize_agent_turn_idem` no-ops when the pair is already present, so
    /// a duplicate `TurnEnded` (the forwarded `AgentEvent` boundary AND a
    /// lingering inference during the §9 additive rollout) finalizes the turn
    /// exactly once — no double trailing newline, no double phase flip.
    /// Cleared by `reset_for_replay` (a fresh generation re-numbers turns).
    pub(crate) finalized: std::collections::HashSet<(u64, usize)>,
    /// Dedicated idempotency for the `ReplayEnd` replay-prefix marker, decoupled
    /// from the per-turn `finalized` ledger above. The `ReplayEnd` envelope
    /// `turn` collides with the next live turn's `completed_turn` (both = the
    /// server's settled count at emit time), so keying `ReplayEnd` into the turn
    /// ledger would steal the live turn's finalize slot and wedge it
    /// ("thinking" forever after resume). This flag settles the replayed prefix
    /// exactly once without consuming a turn key. Reset by `reset_for_replay`.
    pub(crate) replay_prefix_finalized: bool,
    /// Set once this session has observed a real forwarded `TurnEnded` in the
    /// canonical `AgentEvent` stream (the per-session `has_forwarded_turn_ended
    /// _in_stream` gate, spec §9). While `false` the legacy inference is the
    /// SOLE driver of transcript mutation + finalize and the `Agent` arm is
    /// diagnostic-only; once `true` the `AgentEvent` reducer drives mutation
    /// and the idempotent ledger neutralises the still-live inference. Reset
    /// to `false` on `reset_for_replay` so a logless reconnect falls back to
    /// inference until the new channel forwards its first boundary.
    pub(crate) agent_stream_authoritative: bool,
    /// Background polling task that drains the ACP channel into the editor
    /// every ~50ms. Held only so that dropping `AgentState` (e.g. on
    /// `back_to_doc`) cancels the task. The leading `_` mutes unused-field
    /// warnings — the field IS used (its Drop runs on screen exit), but
    /// no method reads it.
    pub(crate) _pump: Option<Task<()>>,
}

impl AgentState {
    /// Derived list of classified sub-agents (§25–§26), folded over
    /// `tool_call_order` + `tool_calls` in first-seen order. This is a pure
    /// projection of the tool-call state — there is no stored mirror to keep
    /// in sync, so it can never drift (ADR-0006 quick win #1). Each entry
    /// carries the originating tool-call id, label, and status, all read
    /// live from the underlying `ToolCall`.
    pub(crate) fn subagents(&self) -> Vec<SubAgent> {
        self.tools
            .order
            .iter()
            .filter_map(|id| self.tools.calls.get(id))
            .filter_map(classify_subagent)
            .collect()
    }

    /// Fingerprint of the structural inputs to the `render_agent`
    /// view-model (flat_items + gutter). Two renders with an equal
    /// fingerprint produce byte-identical flat_items/gutter, so the
    /// cached `Rc`s can be reused. Deliberately EXCLUDES cursor,
    /// selection, theme, and tool-call *content* — none of those affect
    /// the flat build (they're read later, inside the render closure).
    /// See the call site in `render_agent` (S1) for the trap analysis.
    ///
    /// NOTE: `edit_seq` is intentionally EXCLUDED. Most streaming chunks
    /// append text to an existing line without changing line count, frozen
    /// ranges, or tool structure — the only inputs the flat build reads.
    /// Including `edit_seq` forced a full O(transcript) rebuild on every
    /// mid-line chunk, making streaming visually jarring. `line_count`
    /// covers the structural case (new lines added); frozen/tool/expanded
    /// changes cover the rest.
    pub(crate) fn view_model_fingerprint(
        &self,
        line_count: usize,
        frozen_line_count: usize,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        line_count.hash(&mut h);
        frozen_line_count.hash(&mut h);
        self.tools.order.len().hash(&mut h);
        self.tools.order.last().hash(&mut h);
        // Resolved tool anchor lines (Finding 11, INV-8). The flat build
        // resolves each tool's `LineAnchor` to a current line via
        // `line_for_anchor` and groups tools by that line, so the resolved
        // line is a genuine input to the build. Folding it in (in
        // `tool_call_order` order, the same order the build reads) makes the
        // memo key name this dependency directly instead of relying on the
        // unstated invariant "anything that moves an anchor also bumps
        // `edit_seq`". Cheap: one `line_for_anchor` per live tool call.
        for id in &self.tools.order {
            let resolved = self
                .tools
                .anchor
                .get(id)
                .and_then(|&anchor| self.editor.line_for_anchor(anchor));
            resolved.hash(&mut h);
        }
        // Expanded set: hash len + sorted contents (order-independent).
        self.tools.expanded.len().hash(&mut h);
        {
            let mut ids: Vec<&String> = self.tools.expanded.iter().collect();
            ids.sort_unstable();
            for id in ids {
                id.hash(&mut h);
            }
        }
        self.turn_phase.is_awaiting().hash(&mut h);
        h.finish()
    }

    /// Minimal `AgentState` for unit tests. Only the fields the S1
    /// memoization touches need realistic values; the rest are empty/default.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        AgentState {
            editor: Editor::new(String::new(), PathBuf::from("*claude*")),
            channel: None,
            attach_pending: None,
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            last_reconciled_edit_seq: 0,
            status: None,
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            pending_reveal_cursor: false,
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            agent_model: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: true,
            reconciler: yalda::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            generation: 0,
            finalized: std::collections::HashSet::new(),
            replay_prefix_finalized: false,
            agent_stream_authoritative: false,
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: None,
        }
    }

    /// Build a fresh server-managed `AgentState` in the empty baseline, with
    /// `status` shown in the footer. Used for both the "connecting…"
    /// placeholder a tile renders the instant it opens (before the
    /// `list_sessions` / `create_session` round-trip lands) and for the
    /// reconnected/created slots once a `server_session_id` is known. Replaces
    /// the several copies of this giant struct literal that previously lived
    /// inline in `open_agent_inner` / `create_agent_session_via_server`. The
    /// follow handler is wired up before returning.
    pub(crate) fn new_server_managed(status: Option<SharedString>) -> Self {
        let state = AgentState {
            editor: Editor::new(String::new(), PathBuf::from("*claude*")),
            channel: None,
            attach_pending: None,
            mode: EditMode::Insert,
            keybinds: KeybindManager::default(),
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            last_reconciled_edit_seq: 0,
            status,
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            last_scrolled_edit_seq: u64::MAX,
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            pending_reveal_cursor: false,
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::Chatbox(Chatbox::new()),
            current_plan: None,
            agent_mode: None,
            agent_model: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            subagents_open: false,
            server_managed: true,
            reconciler: yalda::agent_transcript::UserTurnReconciler::new(),
            user_turn_ks: std::collections::HashSet::new(),
            generation: 0,
            finalized: std::collections::HashSet::new(),
            replay_prefix_finalized: false,
            agent_stream_authoritative: false,
            follow_output: std::rc::Rc::new(std::cell::Cell::new(true)),
            _pump: None,
        };
        setup_list_follow_handler(&state.list_state, &state.follow_output);
        state
    }

    /// Single source of the in-flight turn number `k` (Finding 3, INV-3),
    /// used by both live submit and replay so gutter tags and TurnHeaders
    /// are identical in both regimes. Delegates to the owned `ReplayTurns`.
    pub(crate) fn current_turn(&self) -> usize {
        self.replay_turns.current_turn()
    }

    /// Advance the replay cursor at a replayed user-message boundary and
    /// return the new turn `k` (Finding 3, INV-3).
    pub(crate) fn advance_replay_user_boundary(&mut self) -> usize {
        self.replay_turns.advance_user_boundary()
    }

    /// The lowest turn number not yet issued to a user turn this generation.
    /// `current_turn()` (`= last_seen + 1`) only advances when a turn's boundary
    /// (`TurnEnded`) settles `last_seen`, so a NEW local submit made while the
    /// previous turn is still in flight would otherwise reuse the in-flight
    /// turn's `k`. Taking `max(current_turn(), next_unused_user_turn())` for a
    /// local submit gives pipelined submits distinct, monotonic turn numbers
    /// instead of colliding (and tripping the M3 double-insert tripwire). It is
    /// a no-op for the common non-pipelined case (`last_seen + 1` is already the
    /// next unused number). `user_turn_ks` holds every user `k` issued this
    /// generation and is wiped per replay generation, so this never drifts.
    pub(crate) fn next_unused_user_turn(&self) -> usize {
        self.user_turn_ks.iter().max().map_or(1, |m| m + 1)
    }

    /// THE single chokepoint for user-turn **dedup + turn-number attribution**.
    /// All four announcement sites — the chatbox optimistic submit, the
    /// worksheet submit, the server `UserPrompt` notification, and the agent's
    /// `UserMessage` echo — route their reconcile through here so suppression
    /// and `k`-derivation have exactly one home instead of drifting copies (the
    /// structural cause of the double-render regressions). Returns `Some(k)` —
    /// the canonical turn number the caller must COMMIT (freeze) however its
    /// surface lays the turn out — or `None` to skip (the reconciler suppressed
    /// an echo, or the M3 tripwire fired). The two commit shapes are
    /// [`insert_user_turn`] (append at EOF) and [`commit_worksheet_turn`]
    /// (freeze authored lines in place); both share this core so a worksheet
    /// turn can never drift from a chatbox turn in numbering or dedup.
    ///
    /// `advance_boundary` must be `true` only for the direct-channel replay
    /// path (`!server_managed`), where there is no replayed `TurnEnded` to bump
    /// the live counter and the [`ReplayTurns`] cursor must be stepped per user
    /// boundary. It is `false` for every live insertion and for the
    /// server-managed path (whose boundaries arrive as replayed `TurnEnded`),
    /// so a live or server turn can never wrongly drive the machine into replay
    /// mode. A *skipped* echo never advances the boundary — suppression and
    /// attribution stay decoupled.
    pub(crate) fn register_user_turn(
        &mut self,
        text: &str,
        origin: yalda::agent_transcript::UserTurnOrigin,
        advance_boundary: bool,
    ) -> Option<usize> {
        use yalda::agent_transcript::UserTurnAction;
        match self.reconciler.reconcile(origin, text, advance_boundary) {
            UserTurnAction::Skip => None,
            UserTurnAction::Insert { advance_boundary } => {
                let k = if advance_boundary {
                    self.advance_replay_user_boundary()
                } else {
                    // Every NON-replay insert mints a fresh turn (a local submit,
                    // or a live/server echo that wasn't suppressed — dual-source
                    // echoes for an existing turn already returned `Skip`). It
                    // should attribute to `current_turn()` (`= last_seen + 1`),
                    // BUT if that `k` is already taken because the previous turn's
                    // boundary hasn't advanced `last_seen` yet (a pipelined submit,
                    // or a content-mismatched echo), take the next unused number
                    // instead — otherwise two distinct turns collide on one `k`
                    // and trip the M3 tripwire (the live crash this guards). A
                    // no-op in the common, non-pipelined case.
                    self.current_turn().max(self.next_unused_user_turn())
                };
                // M3 runtime tripwire: a `k` inserted twice within one
                // generation means the dedup failed and we are about to
                // double-render. Panic in dev (located at the exact mutation);
                // log + drop the duplicate in release rather than ship a double.
                if !self.user_turn_ks.insert(k) {
                    debug_assert!(
                        false,
                        "double user turn: TurnId::User({k}) inserted twice \
                         (text={text:?}) — reconciler dedup regression"
                    );
                    eprintln!(
                        "[yalda-gpui] INVARIANT: TurnId::User({k}) already present; \
                         dropping duplicate user turn (text={text:?})"
                    );
                    return None;
                }
                Some(k)
            }
        }
    }

    /// Insert a user turn into the transcript by APPENDING it at EOF — the
    /// chatbox optimistic submit, the server `UserPrompt` notification, and the
    /// agent's `UserMessage` echo all route here. Delegates dedup + attribution
    /// to [`register_user_turn`] and commits an accepted turn via
    /// `freeze_as_user_turn`. (Worksheet submits share the same core but freeze
    /// in place — see [`commit_worksheet_turn`].)
    pub(crate) fn insert_user_turn(
        &mut self,
        text: &str,
        origin: yalda::agent_transcript::UserTurnOrigin,
        advance_boundary: bool,
    ) {
        if let Some(k) = self.register_user_turn(text, origin, advance_boundary) {
            self.editor.freeze_as_user_turn(text, TurnId::User(k));
        }
    }

    /// Commit a Worksheet-mode submit: derive the canonical turn `k` through the
    /// shared reconciler core ([`register_user_turn`]) — so the server/agent
    /// echo of this prompt is content-matched and **suppressed** instead of
    /// double-rendered — then freeze every collected line IN PLACE under
    /// `TurnId::User(k)`. Worksheet freezes pre-existing, possibly
    /// non-contiguous authored lines (blank spacers included) in document order,
    /// so it does its own per-line freeze rather than the EOF-append
    /// `freeze_as_user_turn`: the chokepoint supplies the *number*, the
    /// worksheet supplies the *placement*.
    ///
    /// `prompt_body` MUST be the joined body actually sent (not the raw
    /// per-line text): registering that is what lets `normalize_user_text` match
    /// the single multi-line echo. Worksheet is a LOCAL submit exactly like the
    /// chatbox, so `advance_boundary` is `false` and `k = current_turn()` (the
    /// single source for the in-flight turn number, INV-3) — replacing the old
    /// hand-rolled `last_seen_turns + 1`, which silently diverged from the
    /// chokepoint and never armed dedup. Returns the committed `k`, or `None` if
    /// the M3 tripwire fired (no lines frozen; the caller still clears/notifies).
    pub(crate) fn commit_worksheet_turn(
        &mut self,
        collected: &[(usize, String)],
        prompt_body: &str,
    ) -> Option<usize> {
        let k = self.register_user_turn(
            prompt_body,
            yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
            false,
        )?;
        for (l, _) in collected {
            self.editor.add_frozen_lines(*l, *l + 1);
            let anchor = self.editor.anchor_for_line(*l);
            self.editor
                .metadata_mut::<TurnId>()
                .insert(anchor, TurnId::User(k));
        }
        Some(k)
    }

    /// Auto-scroll follow decision (F4, INV-13). In Chatbox mode the user's
    /// cursor isn't in the transcript so output streams with sticky-bottom
    /// behavior gated by `follow_output`; in Worksheet mode the viewport
    /// stays anchored to the cursor, following only when the cursor sits at
    /// EOF (the user is typing at the tail and wants to keep seeing fresh
    /// output). This is the single authority the pump (×2) and render-time
    /// re-reveal all consult, replacing the byte-identical copy that used to
    /// live at each site (and drift independently).
    pub(crate) fn follow_tail(&self) -> bool {
        let line_count = self.editor.document().line_count();
        let cursor_at_eof = self.editor.cursor().line + 1 >= line_count;
        should_follow_tail(
            self.input_surface.mode(),
            self.follow_output.get(),
            cursor_at_eof,
        )
    }

    /// Reveal the tail item if we're following AND content has actually grown
    /// since the last reveal (F4, INV-13). The trigger keys on `edit_seq`
    /// (true content growth), NOT on flat-item COUNT, so a chunk that extends
    /// the last line/block without adding a row still re-pins the viewport.
    /// Idempotent within a frame: a repeat call at the same `edit_seq` is a
    /// no-op, so idle ticks don't fight a user who scrolled up. Returns whether
    /// a reveal was actually requested (exercised by the unit test).
    pub(crate) fn reveal_tail_if_following(&mut self, count: usize) -> bool {
        let edit_seq = self.editor.document().edit_seq();
        if count == 0 || edit_seq == self.last_scrolled_edit_seq || !self.follow_tail() {
            return false;
        }
        self.last_scrolled_edit_seq = edit_seq;
        self.list_state.scroll_to_reveal_item(count - 1);
        true
    }

    /// Reconcile the `(list_state, list_item_count)` pair to a new flat-item
    /// count, updating BOTH atomically so the GPUI `ListState` GPUI paints and
    /// the scalar we splice against can never drift (Finding 8, INV-12). When
    /// block ranges are active the item count can shrink unpredictably, so we
    /// reset rather than splice; an incremental splice preserves the height
    /// cache on pure growth. Returns whether the list grew (so callers / the
    /// follow path can key on growth without re-deriving it). This is the only
    /// mutator that touches `list_item_count`, so parity is a property of the
    /// method rather than discipline at each render surface.
    pub(crate) fn reconcile_list(&mut self, new_count: usize, edit_seq: u64) -> bool {
        let old_count = self.list_item_count;
        if new_count != old_count {
            if !self.block_ranges.is_empty() || new_count < old_count {
                self.list_state.reset(new_count);
            } else {
                self.list_state
                    .splice(old_count..old_count, new_count - old_count);
            }
            self.list_item_count = new_count;
            self.last_reconciled_edit_seq = edit_seq;
        } else if new_count > 0 && edit_seq != self.last_reconciled_edit_seq {
            // Mid-line append: item count unchanged but text grew inside the
            // last item (streaming agent prose before a `\n`). Invalidate
            // just the tail item's cached height so GPUI re-measures it
            // instead of painting new content at the old height.
            let last = new_count - 1;
            self.list_state.splice(last..last + 1, 1);
            self.last_reconciled_edit_seq = edit_seq;
        }
        new_count > old_count
    }

    /// Test-only wrapper that passes a dummy `edit_seq` (the tests don't
    /// exercise the mid-line-append height invalidation).
    #[cfg(test)]
    pub(crate) fn reconcile_list_test(&mut self, new_count: usize) -> bool {
        self.reconcile_list(new_count, 0)
    }

    /// Fold the replay cursor back into the live counter at end-of-replay
    /// (Finding 13, INV-4).
    pub(crate) fn finish_replay(&mut self) {
        self.replay_turns.finish_replay();
    }

    /// Idempotent turn finalize keyed on `(generation, turn)` (spec §7/H5).
    ///
    /// The newline-guard `finalize_agent_turn(editor)` is itself idempotent on
    /// the buffer (it only appends a `\n` when one is missing), but the PHASE
    /// transition (`turn_phase = Idle`) and any future per-turn side effect are
    /// NOT — and during the §9 additive rollout a turn boundary can arrive
    /// TWICE (the forwarded `AgentEvent::TurnEnded` AND the still-live
    /// inference). This ledger collapses both into exactly one finalize: the
    /// second call for the same `(generation, turn)` is a no-op. Returns
    /// whether this call actually finalized (the first time for the pair), so
    /// callers can gate the phase flip on it.
    ///
    /// The caller — NOT this method, and NOT the reducer fold — owns the
    /// `turn_phase = Idle` decision (it is a pump-side concern, spec §7): this
    /// keeps finalize a pure ledger op and lets the caller flip the phase only
    /// when a finalize genuinely happened.
    pub(crate) fn finalize_agent_turn_idem(&mut self, generation: u64, turn: usize) -> bool {
        if !self.finalized.insert((generation, turn)) {
            return false; // already finalized this (generation, turn)
        }
        finalize_agent_turn(&mut self.editor);
        true
    }

    /// Settle the replayed history prefix exactly once on `ReplayEnd`, WITHOUT
    /// consuming a per-turn `(generation, turn)` ledger slot. The `ReplayEnd`
    /// marker is not a turn boundary and its envelope `turn` aliases the next
    /// live turn's index, so it must NOT route through `finalize_agent_turn_idem`
    /// (that would pre-occupy the live turn's key and wedge its finalize). Like
    /// the turn ledger this applies the buffer-idempotent newline guard; the
    /// boolean exists only to keep the phase flip a one-shot. Returns whether
    /// this call actually settled (the first `ReplayEnd` of this generation), so
    /// the caller can gate the phase flip on it. Reset by `reset_for_replay`.
    pub(crate) fn finalize_replay_prefix(&mut self) -> bool {
        if self.replay_prefix_finalized {
            return false;
        }
        self.replay_prefix_finalized = true;
        finalize_agent_turn(&mut self.editor);
        true
    }

    /// Reset all transcript-derived state to the empty baseline so that a
    /// server re-attach — which replays the session's full `event_log` — can
    /// rebuild the transcript from scratch without duplicating what's already
    /// on screen. Used by the reconnect path. Preserves the live channel /
    /// attach handle, input mode, follow-output preference, and pump handle;
    /// only the rendered transcript and its derived caches are cleared.
    pub(crate) fn reset_for_replay(&mut self) {
        self.editor = Editor::new(String::new(), PathBuf::from("*claude*"));
        self.list_state = gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0));
        setup_list_follow_handler(&self.list_state, &self.follow_output);
        self.list_item_count = 0;
        // Fresh editor restarts `edit_seq` at 0; clear the follow-scroll
        // watermark so the first replayed chunk re-reveals the tail (F4).
        self.last_scrolled_edit_seq = u64::MAX;
        self.turn_phase = TurnPhase::Idle;
        self.replay_turns = yalda::acp_channel::ReplayTurns::default();
        // The transcript is being rebuilt from the authoritative event_log:
        // nothing is "pending local" any more, and this starts a fresh
        // tripwire generation (the replay re-numbers `k` from 1). This clear
        // MUST happen-before any replayed echo is processed — guaranteed since
        // reset runs inside the reconnect update before re-attach.
        self.reconciler.reset();
        self.user_turn_ks.clear();
        // A fresh generation re-numbers turns from 1 and discards the finalize
        // ledger (spec §4/§7): a (gen, turn) that finalized in the OLD channel
        // must not suppress the SAME (gen, turn) replayed in the new one. The
        // `generation` field is NOT reset here — the uniform rebaseline rule
        // (`apply_agent_event`) advances it to the bumped value AFTER calling
        // this, so zeroing it would make the very event that triggered the
        // reset re-trigger it. `agent_stream_authoritative` IS reset: a logless
        // reconnect must fall back to inference until the new channel forwards
        // its first `TurnEnded`.
        self.finalized.clear();
        self.replay_prefix_finalized = false;
        self.agent_stream_authoritative = false;
        self.tools.clear();
        self.block_ranges.clear();
        self.view_model = AgentViewModel::new();
        self.lines_cache = std::rc::Rc::new(Vec::new());
        self.lines_cache_seq = u64::MAX;
        self.highlight_cache.reset();
        self.current_plan = None;
        self.focused_subagent = None;
        self.usage = None;
    }
}

/// A re-attachable session resolved by the background half of `open_agent`
/// (S4). Carries everything the main thread needs to fill or push a slot —
/// the attach round-trip has already been issued off-thread.
pub(crate) struct AttachedSlot {
    pub(crate) label: String,
    pub(crate) sid: String,
    /// The ACP session id, used as the slot's `resume_id`.
    pub(crate) acp_id: Option<String>,
    /// Footer status string ("reconnected …").
    pub(crate) status: String,
    /// Server-reported permission mode for this session, mirrored into
    /// `AgentState.permission_mode` when the slot binds.
    pub(crate) permission_mode: yalda::acp_channel::PermissionMode,
}

/// Outcome of the background session-server round-trips kicked off by
/// `spawn_open_agent_server`. Applied on the paint thread by
/// `apply_open_agent_resolution`.
pub(crate) enum OpenResolution {
    /// Existing cwd sessions were found and re-attached.
    Attached(Vec<AttachedSlot>),
    /// No existing session — a fresh one was created.
    Created {
        sid: String,
        acp_id: Option<String>,
        /// Server-reported permission mode for the freshly created session.
        permission_mode: yalda::acp_channel::PermissionMode,
    },
    /// The list or create round-trip failed; surface it on the placeholder.
    Failed(String),
}

/// Process-wide monotonic allocator for `AgentTile::pending_open_token`.
/// Tokens are never reused, so an in-flight async server open always binds
/// back to exactly the placeholder that started it.
pub(crate) static NEXT_OPEN_TOKEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

pub(crate) fn alloc_open_token() -> u64 {
    NEXT_OPEN_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One agent conversation, owned centrally by [`AgentSessions`] (spec-agent-
/// session-ownership.md). State that used to live in `AgentSlot.state` + the
/// slot's binding fields now lives here, keyed by a stable [`SessionId`]; the
/// `server_session_id` is owned by the store (not this struct), so there is one
/// source of truth for the binding. Derefs to its inner [`AgentState`] so the
/// many `slot.state.foo` bodies become `session.foo` with minimal churn.
pub(crate) struct AgentSession {
    /// The session state. Contains editor, channel, tool calls, etc.
    pub(crate) state: AgentState,
    /// User-facing label shown in tab strips / pickers.
    pub(crate) label: String,
    /// Absolute working directory the agent subprocess runs in (spec-agent-
    /// cwd.md §1).
    pub(crate) cwd: PathBuf,
    /// The id this session was created from on persistence restore. Stays this
    /// value even if `session/load` fell back to `session/new`, so the next
    /// reboot retries the original load. `None` for fresh `claude-new` sessions.
    pub(crate) resume_id: Option<String>,
}

impl std::ops::Deref for AgentSession {
    type Target = AgentState;
    fn deref(&self) -> &AgentState {
        &self.state
    }
}

impl std::ops::DerefMut for AgentSession {
    fn deref_mut(&mut self) -> &mut AgentState {
        &mut self.state
    }
}

/// THE owner of agent-session state (spec-agent-session-ownership.md). Strict
/// 1:1 session↔sid enforced by [`SessionStore`].
pub(crate) type AgentSessions = SessionStore<AgentSession>;

/// One existing server session offered in the in-tile [`SessionPicker`].
/// Built from a [`SessionInfo`] returned by `list_sessions`; carries
/// everything the attach path needs so selecting a row never has to make a
/// second round-trip to learn the sid / acp id / permission mode.
pub(crate) struct PickerSession {
    pub(crate) sid: String,
    pub(crate) acp_id: Option<String>,
    pub(crate) label: String,
    pub(crate) turns: usize,
    pub(crate) connected: bool,
    pub(crate) permission_mode: yalda::acp_channel::PermissionMode,
}

/// In-tile session chooser shown when an Agent tile has no `bound` session:
/// it lists the FREE sessions (those no tile binds) plus a "start a new
/// session" row, so the user picks/rebinds instead of silently resuming.
/// `bound == None` ⇒ the tile renders this picker; selecting a row binds the
/// tile, after which `render_agent` renders the normal transcript.
pub(crate) struct SessionPicker {
    /// `None` while the background `list_sessions` round-trip is in flight;
    /// `Some` once it lands (possibly empty — only the "new session" row).
    /// These are the FREE sessions — selectable rows `1..=N`.
    pub(crate) sessions: Option<Vec<PickerSession>>,
    /// Sessions already BOUND to some tile (cwd-matched). Rendered in a
    /// separate, NON-selectable column — informational only, since a session
    /// is bound by at most one tile and can't be attached from here.
    pub(crate) bound: Vec<PickerSession>,
    /// Set if the list round-trip failed; rendered in place of the list.
    pub(crate) error: Option<SharedString>,
    /// Highlighted row. Row 0 is always "start a new session"; rows `1..=N`
    /// map to `sessions[row - 1]`.
    pub(crate) selected: usize,
    /// The cwd this picker was opened for, threaded into create/attach.
    pub(crate) cwd: PathBuf,
}

impl SessionPicker {
    pub(crate) fn loading(cwd: PathBuf) -> Self {
        Self {
            sessions: None,
            bound: Vec::new(),
            error: None,
            selected: 0,
            cwd,
        }
    }

    /// Selectable rows: the "new session" row plus one per listed session.
    pub(crate) fn row_count(&self) -> usize {
        1 + self.sessions.as_ref().map(|s| s.len()).unwrap_or(0)
    }

    /// Move the highlight by `delta`, wrapping at both ends.
    pub(crate) fn move_selection(&mut self, delta: isize) {
        let n = self.row_count() as isize;
        if n <= 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
    }
}

/// One Agent tile — a VIEW onto a single session, not a store
/// (spec-agent-session-ownership.md). The session STATE lives in
/// `YaldaGpuiView::sessions`; the tile holds only a lightweight key. Agent and
/// Buffer are ORTHOGONAL `App` variants — a tile is one or the other; an Agent
/// tile has zero knowledge of buffers (no stash). Leaving an agent (Ctrl-V)
/// converts the tile to a fresh `BufferApp::Picking`; the pooled file buffers
/// stay reachable via Cmd+O regardless.
///
/// A tile shows EXACTLY ONE session (`bound`). `bound == None` ⇒ the tile
/// renders the `picker` (free-session chooser / rebind / "new"). Strict 1:1: a
/// given `SessionId` is bound by at most one tile (INV-2); rebinding points the
/// tile at a free session and frees (does not kill) the previous one. Session
/// close / unbind / rebind all keep the tile `App::Agent` with `bound = None`.
pub(crate) struct AgentTile {
    /// The session shown here; `None` ⇒ render the picker (the selector).
    pub(crate) bound: Option<SessionId>,
    /// Set while an async server open/create round-trip for this tile is in
    /// flight; the resolution binds back to this tile by matching the token
    /// across the whole workspace. Globally unique (see `alloc_open_token`).
    /// Cleared once the round-trip resolves.
    pub(crate) pending_open_token: Option<u64>,
    /// When `Some` (and `bound == None`), this tile shows the in-tile session
    /// picker instead of a transcript. Cleared the moment a session is bound.
    pub(crate) picker: Option<SessionPicker>,
}

impl AgentTile {
    pub(crate) fn new() -> Self {
        Self {
            bound: None,
            pending_open_token: None,
            picker: None,
        }
    }
}
