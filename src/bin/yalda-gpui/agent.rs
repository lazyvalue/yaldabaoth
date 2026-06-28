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

/// Next user-turn jump ordinal from `cur`, clamped to `[0, count-1]`. `to_last`
/// parks on the most recent turn (toggle-on / "what I wrote last"); otherwise
/// `delta` steps (`+1` = newer/`j`, `-1` = older/`k`), saturating at both ends.
/// Caller guards `count == 0`. Pure — unit-tested.
pub(crate) fn next_jump_ord(cur: usize, count: usize, delta: i32, to_last: bool) -> usize {
    let last = count.saturating_sub(1);
    let cur = cur.min(last);
    if to_last {
        last
    } else if delta >= 0 {
        (cur + delta as usize).min(last)
    } else {
        cur.saturating_sub((-delta) as usize)
    }
}

/// True when a jump step should drop the viewport at the buffer's page end
/// instead of on a user-turn header: a forward (`j`) step — not the toggle-on
/// "jump to last" — that can't advance because the cursor is already parked on
/// the newest turn. `k` (older) and toggle-on keep their per-turn behavior.
/// Pure — unit-tested.
pub(crate) fn jump_lands_at_page_end(
    prev: usize,
    next: usize,
    count: usize,
    delta: i32,
    to_last: bool,
) -> bool {
    delta > 0 && !to_last && next == prev && next == count.saturating_sub(1)
}

/// Flat-item indices of the user input turns (`TurnHeader { role: User }`) in
/// render order. The single source for user-turn jump navigation (agent `.`
/// menu): the handler reads the count to clamp the ordinal, and `build_body`
/// resolves the Nth entry to the scroll target. Pure — unit-tested.
pub(crate) fn user_turn_item_indices(flat_items: &[FlatItem]) -> Vec<usize> {
    flat_items
        .iter()
        .enumerate()
        .filter(|(_, it)| matches!(it, FlatItem::TurnHeader { role: TurnRole::User }))
        .map(|(i, _)| i)
        .collect()
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
                    if let Some(mut c) = this.agent_mut(cx) {
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
pub(crate) fn anchor_for_new_tool_call(editor: &mut Editor, floor_char: usize) -> LineAnchor {
    let eof = editor.document().rope().len_chars();
    if floor_char < eof {
        // A user worksheet draft sits below `floor_char` (its untagged top).
        // Splice the tool's dedicated blank anchor line at the top of that
        // draft so the tool block renders ABOVE the user's in-progress text
        // instead of under it (the interspersed-tool-group bug). `floor_char`
        // is at a line start, so the char before it is the prior agent line's
        // trailing '\n' — inserting "\n" here opens a clean blank line and
        // shifts the draft down by one.
        let line = editor.document().rope().char_to_line(floor_char);
        editor.programmatic_insert(floor_char, "\n");
        return editor.anchor_for_line(line);
    }
    // No pending draft — append the anchor line at EOF (original behavior).
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

/// Char index at the TOP of the user's in-progress worksheet draft — the
/// contiguous run of *untagged*, unfrozen lines at EOF that holds at least one
/// non-blank line. Agent content (LLM chunks via
/// [`Editor::append_llm_chunk_floored`], tool anchors via
/// [`anchor_for_new_tool_call`]) splices here so a turn streaming in while the
/// user composes lands ABOVE their text rather than below it.
///
/// Frozen and turn-tagged lines (`Llm`/`User`/`Tool`/`System`) are agent-owned
/// and stop the upward walk. When the trailing region is all blank — Chatbox,
/// or an untouched worksheet tail — this returns EOF, so the splice is
/// byte-for-byte unchanged from the pre-floor behavior.
pub(crate) fn agent_tail_floor_char(editor: &Editor) -> usize {
    let doc = editor.document();
    let eof = doc.rope().len_chars();
    let line_count = doc.line_count();
    let turn_meta = editor.metadata::<TurnId>();
    let mut floor = line_count;
    let mut has_user_text = false;
    while floor > 0 {
        let l = floor - 1;
        if editor.is_frozen_line(l) {
            break;
        }
        let tagged = editor
            .anchor_for_line_opt(l)
            .is_some_and(|a| turn_meta.get(a).is_some());
        if tagged {
            break;
        }
        if !doc.line_text(l).trim().is_empty() {
            has_user_text = true;
        }
        floor -= 1;
    }
    if !has_user_text || floor >= line_count {
        eof
    } else {
        doc.line_col_to_char(floor, 0)
    }
}

/// Detect line ranges in `lines` that should be rendered as structured
/// blocks (tables and fenced code blocks) rather than line-by-line.
/// Only considers frozen (agent-written) lines.
///
/// Returns `Vec<(start, end)>` where `start..end` covers the full block
/// including delimiters. Ranges are non-overlapping and sorted.
/// Pure follow-tail policy (F4, INV-13), factored out of `AgentState` so it
/// can be unit-tested without a GPUI editor/list. Model C: the user's cursor is
/// in the compose buffer (outside the transcript) in BOTH placements — the
/// transcript is read-only — so following is purely the sticky-bottom
/// `follow_output` flag regardless of placement.
pub(crate) fn should_follow_tail(follow_output: bool) -> bool {
    follow_output
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

/// 64-bit fingerprint of the frozen-line *layout* (the ranges themselves, in
/// order). Gates structural-block re-detection: it changes when a frozen block
/// is added, removed, OR shifted — unlike a bare line-count, which misses an
/// insert-between-blocks that moves a block without changing the total.
pub(crate) fn frozen_ranges_fp(frozen_ranges: &[(usize, usize)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    frozen_ranges.len().hash(&mut h);
    for &(s, e) in frozen_ranges {
        s.hash(&mut h);
        e.hash(&mut h);
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
///
/// `line_font` is the typography font for ordinary spans (monospace in the
/// Code/worksheet views, proportional in the WP view); `code_font` is the
/// fallback `styled_line_element` uses for spans carrying a code background.
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
    line_font: &SharedString,
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
            &line, base_style, base_fg, line_font, code_font,
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
                &line, base_style, base_fg, line_font, code_font,
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
                    &line, base_style, base_fg, line_font, code_font,
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
                    &line, base_style, base_fg, line_font, code_font,
                ));
            }
        } else {
            let line = segments_to_styled_line(&[(text.clone(), *style)]);
            row = row.child(styled_line_element(
                &line, base_style, base_fg, line_font, code_font,
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

/// Render a single chatbox logical line as a NON-WRAPPING, horizontally-scrolled
/// row (spec-chatbox-caret-containment.md Behavior 5).
///
/// The line is sliced to the visible column window `[left_col, left_col +
/// visible_cols)` and rendered from the row's left edge — NOT pixel-offset.
/// Slicing relies on the string boundary, so there is no per-column pixel drift
/// (the drift that, with an inexact `char_w`, scrolls the caret off the edge —
/// the recurring bug). A long unbreakable token therefore scrolls instead of
/// wrapping or overflowing. The caret is injected at `cursor_col - left_col`
/// within the slice, preserving Normal (block over the char) vs Insert
/// (zero-width beam before it) semantics.
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
    selection_bg: Hsla,
    left_col: usize,
    visible_cols: usize,
) -> AnyElement {
    let line_h = px(18.0);
    let fg: Hsla = text_color;
    // Theme-driven selection background — the same color the edit view
    // (`build_edit_body_*`) paints, so the chatbox highlight contrast tracks
    // the active theme instead of a hardcoded Dracula swath that clashes on
    // light/non-Dracula themes.
    let sel_bg: Hsla = selection_bg;

    let chars: Vec<char> = full_text.chars().collect();
    let char_count = chars.len();

    // ── Horizontal window: slice the line to the visible columns. ──
    // `vs..ve` is the char range shown; rendering it from the row's LEFT edge
    // (not a pixel offset) is what makes the approximate `char_w` safe — no
    // per-column drift can accumulate (spec Behavior 5).
    let vs = left_col.min(char_count);
    let ve = left_col.saturating_add(visible_cols.max(1)).min(char_count);
    let slice: Vec<char> = chars[vs..ve].to_vec();
    let slice_len = slice.len();

    // Selection projected onto this line, then intersected with the slice and
    // shifted into slice-local columns.
    let line_sel = sel
        .and_then(|s| line_selection_range(s, line_idx, total_line_chars))
        .and_then(|(s, e)| if e > s { Some((s, e)) } else { None })
        .and_then(|(s, e)| {
            let ss = s.max(vs).saturating_sub(vs);
            let se = e.min(ve).saturating_sub(vs);
            if se > ss {
                Some((ss.min(slice_len), se.min(slice_len)))
            } else {
                None
            }
        });

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .min_w_0()
        .w_full()
        .min_h(line_h)
        // Pin the text line-box to the caret height (18px) so a line carrying
        // a glyph is exactly as tall as the empty/placeholder line (which only
        // holds the fixed-height caret).
        .line_height(line_h)
        // Belt-and-suspenders clip: the slice is sized to fit by construction,
        // but a fractional last column never spills past the box edge.
        .overflow_hidden()
        .font_family(code_font.clone())
        .text_size(px(13.0))
        .text_color(fg);

    // Emit a chunk of the SLICE with the selection highlight painted through any
    // overlapping range. `start` is the chunk's slice-local start column.
    let emit_chunk = |row: gpui::Div, text: String, start: usize| -> gpui::Div {
        if text.is_empty() {
            return row;
        }
        let chunk_chars: Vec<char> = text.chars().collect();
        let chunk_len = chunk_chars.len();
        let end = start + chunk_len;
        if let Some((ss, se)) = line_sel
            && se > start
            && ss < end
        {
            let local_ss = ss.saturating_sub(start).min(chunk_len);
            let local_se = se.saturating_sub(start).min(chunk_len);
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

    // Caret column within the slice (cursor_col >= left_col by the containment
    // invariant; saturating for safety). `None` when the caret isn't here.
    let rel_caret = is_cursor_line.then(|| cursor_col.saturating_sub(vs));

    match rel_caret {
        // Non-cursor line: one chunk for the whole visible slice (a placeholder
        // space when empty so the row keeps its height).
        None => {
            if slice_len == 0 {
                row = row.child(" ");
            } else {
                let text: String = slice.iter().collect();
                row = emit_chunk(row, text, 0);
            }
        }
        // Cursor line: split the slice at the caret column and inject the caret.
        Some(rel) => {
            let rel = rel.min(slice_len);
            let before: String = slice[..rel].iter().collect();
            if !before.is_empty() {
                row = emit_chunk(row, before, 0);
            }
            let cursor_char = slice.get(rel).copied().unwrap_or(' ');
            row = row.child(make_caret(mode, cursor_char, cursor_color));
            // Normal mode consumes the char under the caret; Insert is a
            // zero-width beam so that char stays in the after-stream.
            let after_start = match mode {
                EditMode::Normal => rel + 1,
                EditMode::Insert => rel,
            };
            if after_start < slice_len {
                let after: String = slice[after_start..].iter().collect();
                row = emit_chunk(row, after, after_start);
            }
        }
    }

    let el = row.into_any_element();
    // Headless harness (#3.2): tag the caret's visual row so a `#[gpui::test]`
    // can read its PAINTED bounds and prove it's inside the compose box — the
    // virtualized list never paints an off-screen row, so a missing probe means
    // the caret fell below the fold. No-op in production. Pins INV-UX-1 at paint.
    if is_cursor_line {
        probe_bounds("compose-cursor-row", el)
    } else {
        el
    }
}

/// Word-wrap a (tab-expanded) monospace line into visual-row char ranges of at
/// most `width` columns each (INV-UX-2). Breaks at the last space strictly inside
/// `[start, start+width)`; a word longer than `width` is hard-broken at the limit.
/// Returns half-open `[start, end)` ranges over the line covering EVERY char
/// (nothing dropped — the caret must be addressable at every column), always ≥1
/// row (an empty line → one `(0, 0)` row). `width == 0` is treated as 1.
///
/// Computed here (not in GPUI layout) because the compose is monospace and the
/// box width in columns is known — so the caret's visual row/col stay exactly
/// known (view-owns-its-coordinates), and INV-UX-1 holds without measuring the
/// painted text. This supersedes the horizontal-scroll window the compose used
/// (`spec-chatbox-caret-containment.md`): wrapped text never needs to scroll
/// sideways.
pub(crate) fn wrap_line_cols(line: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let n = line.len();
    if n == 0 {
        return vec![(0, 0)];
    }
    let mut rows = Vec::new();
    let mut start = 0;
    while start < n {
        if n - start <= width {
            rows.push((start, n));
            break;
        }
        let hard = start + width;
        // Last space strictly inside (start, hard): break AFTER it so the space
        // trails this row and the next row begins at real content. None ⇒ a word
        // longer than the row, hard-break at the column limit.
        let mut end = hard;
        for j in (start + 1..hard).rev() {
            if line[j] == ' ' {
                end = j + 1;
                break;
            }
        }
        rows.push((start, end));
        start = end;
    }
    rows
}

/// The index of the visual row (`wrap_line_cols` output) the caret sits on for
/// `cursor_col`. A column on a row boundary belongs to the NEXT row's start; a
/// caret at end-of-line sits on the last row.
pub(crate) fn caret_visual_row(rows: &[(usize, usize)], cursor_col: usize) -> usize {
    for (i, &(rs, re)) in rows.iter().enumerate() {
        if cursor_col >= rs && cursor_col < re {
            return i;
        }
    }
    rows.len().saturating_sub(1)
}

/// Render one LOGICAL compose line as a column of wrapped visual rows (INV-UX-2).
/// Each visual row is drawn by [`build_chatbox_line`] over exactly its own char
/// range, so nothing is clipped and no horizontal scroll is needed; the caret is
/// placed on the single visual row that holds `cursor_col`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_chatbox_wrapped_line(
    full_text: &str,
    is_cursor_line: bool,
    cursor_col: usize,
    mode: EditMode,
    cursor_color: Hsla,
    sel: Option<((usize, usize), (usize, usize))>,
    line_idx: usize,
    code_font: &SharedString,
    text_color: Hsla,
    selection_bg: Hsla,
    wrap_cols: usize,
) -> AnyElement {
    let chars: Vec<char> = full_text.chars().collect();
    let total_chars = chars.len();
    let rows = wrap_line_cols(&chars, wrap_cols);
    let caret_row = is_cursor_line.then(|| caret_visual_row(&rows, cursor_col));

    let mut col = div().flex().flex_col().w_full().min_w_0();
    for (r, &(rs, re)) in rows.iter().enumerate() {
        col = col.child(build_chatbox_line(
            full_text,
            caret_row == Some(r),
            cursor_col,
            mode,
            cursor_color,
            sel,
            line_idx,
            total_chars,
            code_font,
            text_color,
            selection_bg,
            rs,        // left_col = this visual row's start
            re - rs,   // visible_cols = this row's exact width (no clip, no scroll)
        ));
    }
    col.into_any_element()
}

/// Vertical caret-containment metrics for the WRAPPED compose, in VISUAL-row
/// space (INV-UX-1 under INV-UX-2). Once lines wrap, the box scrolls in visual
/// rows — not logical lines — so the vertical window MUST be computed over visual
/// rows or the caret falls below the fold (the recurring chatbox-cursor bug).
/// Returns `(caret_visual_row, total_visual_rows, per_line_row_counts)`:
/// the caret's absolute visual-row index, the total visual-row count, and each
/// logical line's visual-row count (so the list scroll can map a target visual
/// row back to a `(list item, offset)` pair via [`compose_item_for_visual_row`]).
pub(crate) fn compose_visual_metrics(
    lines: &[String],
    caret_line: usize,
    caret_col: usize,
    wrap_cols: usize,
) -> (usize, usize, Vec<usize>) {
    let mut per_line: Vec<usize> = Vec::with_capacity(lines.len().max(1));
    let mut caret_vrow = 0usize;
    let mut total = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let rows = wrap_line_cols(&chars, wrap_cols);
        if i < caret_line {
            caret_vrow += rows.len();
        } else if i == caret_line {
            caret_vrow += caret_visual_row(&rows, caret_col);
        }
        per_line.push(rows.len());
        total += rows.len();
    }
    (caret_vrow, total.max(1), per_line)
}

/// Map an absolute VISUAL-row index to `(logical-line list-item index, visual-row
/// offset within that item)` for the compose list scroll. The compose list's
/// items are logical lines (each a wrapped column of visual rows), so the
/// authoritative visual-row top from the window math must be translated back into
/// the list's item coordinate space. `per_line` is each line's visual-row count
/// (from [`compose_visual_metrics`]).
pub(crate) fn compose_item_for_visual_row(per_line: &[usize], visual_row: usize) -> (usize, usize) {
    let mut acc = 0usize;
    for (i, &n) in per_line.iter().enumerate() {
        if visual_row < acc + n {
            return (i, visual_row - acc);
        }
        acc += n;
    }
    let last = per_line.len().saturating_sub(1);
    let off = per_line.last().copied().unwrap_or(1).saturating_sub(1);
    (last, off)
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

/// The scroll/list UI state for one transcript, moved OUT of `AgentState` into
/// the [`TranscriptView`] widget (ticket 021 — widgets own UI state; models own
/// domain state). The `ListState` is a virtualised, non-uniform-height list
/// that only paints visible rows; `list_item_count` mirrors what's registered
/// so growth can splice (preserving the height cache) instead of resetting; the
/// two `*_seq` watermarks de-dupe the per-frame reconcile + follow-scroll.
///
/// Kept a plain struct (no GPUI `Context`) so its reconcile/reveal logic stays
/// unit-testable against an `AgentState` with no window — the original
/// `reconcile_list` / `reveal_tail_if_following` tests carry over verbatim.
pub(crate) struct TranscriptScroll {
    /// Virtualized list state for the claude transcript. We render every
    /// doc-line + tool-block as an item in a `gpui::list` — non-uniform-height
    /// list that only paints visible rows. `ListAlignment::Bottom` gives the
    /// chat-style initial pin. The `follow_output` flag (on `AgentState`,
    /// maintained by the scroll handler) gates pump-driven auto-scroll so the
    /// user can scroll up to read history without being yanked to the bottom.
    pub(crate) list_state: gpui::ListState,
    /// Total number of items currently registered in `list_state`. Tracked
    /// separately so new items splice in as the buffer grows without a reset.
    pub(crate) list_item_count: usize,
    /// `edit_seq` at the last `reconcile_list` call that actually touched the
    /// list. Detects mid-line appends (count unchanged but content grew) so the
    /// tail item's cached height is invalidated.
    pub(crate) last_reconciled_edit_seq: u64,
    /// `edit_seq` at which the tail was last revealed by the follow-scroll
    /// (F4, INV-13). Keying the re-reveal on content growth (not count delta)
    /// re-pins the viewport even when a chunk grows the last line without
    /// adding a row. `u64::MAX` = never scrolled.
    pub(crate) last_scrolled_edit_seq: u64,
    /// Cheap per-item identity keys from the last reconcile, for the
    /// block-ranges (worksheet) splice diff. Diffing keys lets the unchanged
    /// PREFIX above an edit keep its measured heights + scroll anchor, instead
    /// of `reset()`-ing the whole list (which nulled the scroll → the worksheet
    /// "newline jumps to the top of the viewport" bug).
    pub(crate) last_keys: Vec<FlatKey>,
}

/// A cheap, `Copy` identity for a [`FlatItem`] — enough to diff two item lists
/// for the [`splice_list_to_items`] prefix/suffix match WITHOUT deep-comparing
/// rendered blocks. `Block` keys on the `Rc` pointer (the S1 rebuild reuses the
/// same `Rc` by refcount bump, so a stable block stays pointer-equal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlatKey {
    Line(usize),
    Tool(usize),
    Block(usize),
    Turn(TurnRole),
    Thinking,
}

impl FlatKey {
    pub(crate) fn of(item: &FlatItem) -> FlatKey {
        match item {
            FlatItem::Line(i) => FlatKey::Line(*i),
            FlatItem::ToolGroup { anchor_line, .. } => FlatKey::Tool(*anchor_line),
            FlatItem::Block(rc) => FlatKey::Block(std::rc::Rc::as_ptr(rc) as usize),
            FlatItem::TurnHeader { role } => FlatKey::Turn(*role),
            FlatItem::ThinkingIndicator => FlatKey::Thinking,
        }
    }
}

impl TranscriptScroll {
    pub(crate) fn new() -> Self {
        Self {
            list_state: gpui::ListState::new(0, gpui::ListAlignment::Bottom, gpui::px(256.0)),
            list_item_count: 0,
            last_reconciled_edit_seq: 0,
            last_scrolled_edit_seq: u64::MAX,
            last_keys: Vec::new(),
        }
    }

    /// Reconcile the `(list_state, list_item_count)` pair to a new flat-item
    /// list, updating BOTH atomically so the GPUI `ListState` it paints and the
    /// scalar we splice against can never drift (Finding 8, INV-12). Returns
    /// whether the list grew. This is the only mutator that touches
    /// `list_item_count`, so parity is a property of the method.
    ///
    /// When block ranges are active (worksheet), an edit can restructure items
    /// anywhere, but we still SPLICE the minimal changed range (by diffing cheap
    /// per-item [`FlatKey`]s) rather than `reset()`. The unchanged prefix above
    /// the edit keeps its measured heights AND the scroll anchor, so a worksheet
    /// newline no longer snaps the viewport to the top (`reset()` nulled the
    /// scroll — the recurring "cursor jumps to the top" bug). The streaming /
    /// non-worksheet path keeps the count-based tail splice.
    pub(crate) fn reconcile_list(
        &mut self,
        block_ranges_active: bool,
        items: &[FlatItem],
        edit_seq: u64,
    ) -> bool {
        let new_count = items.len();
        let old_count = self.list_item_count;

        if block_ranges_active {
            let new_keys: Vec<FlatKey> = items.iter().map(FlatKey::of).collect();
            // Splice the minimal changed range, preserving the prefix's heights
            // + scroll anchor (no jump-to-top).
            splice_list_to_items(&self.list_state, &self.last_keys, &new_keys);
            // A count-stable text edit (typing on the editable tail line) leaves
            // the keys identical, so the splice above is a no-op; re-measure the
            // tail item like the streaming path so a wrap-induced height change
            // isn't painted at the stale height.
            if new_count == old_count
                && new_count > 0
                && edit_seq != self.last_reconciled_edit_seq
                && new_keys == self.last_keys
            {
                let last = new_count - 1;
                self.list_state.splice(last..last + 1, 1);
            }
            self.last_keys = new_keys;
            self.list_item_count = new_count;
            self.last_reconciled_edit_seq = edit_seq;
            return new_count > old_count;
        }

        if new_count != old_count {
            if new_count < old_count {
                self.list_state.reset(new_count);
            } else {
                self.list_state
                    .splice(old_count..old_count, new_count - old_count);
            }
            self.list_item_count = new_count;
            self.last_reconciled_edit_seq = edit_seq;
            // Keep keys roughly in sync so a later switch INTO worksheet mode
            // diffs against a recent snapshot instead of over-splicing once.
            self.last_keys = items.iter().map(FlatKey::of).collect();
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

    /// Reveal the tail item if `following` AND content has actually grown since
    /// the last reveal (F4, INV-13). Keys on `edit_seq` (true content growth),
    /// NOT on flat-item COUNT, so a chunk that extends the last line/block
    /// without adding a row still re-pins the viewport. Idempotent within a
    /// frame: a repeat call at the same `edit_seq` is a no-op. Returns whether
    /// a reveal was actually requested. `following` is the caller's
    /// `AgentState::follow_tail()`, `edit_seq` the document's current seq.
    pub(crate) fn reveal_tail_if_following(
        &mut self,
        following: bool,
        edit_seq: u64,
        count: usize,
    ) -> bool {
        if count == 0 || edit_seq == self.last_scrolled_edit_seq || !following {
            return false;
        }
        self.last_scrolled_edit_seq = edit_seq;
        self.list_state.scroll_to_reveal_item(count - 1);
        true
    }
}

impl Default for TranscriptScroll {
    fn default() -> Self {
        Self::new()
    }
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

/// Which surface holds keyboard focus in an agent tile (Model C — `design-c.md`
/// §4.5). Default `Compose`. `Transcript` is the read-only navigation/selection
/// mode (the base "workspace" capability), entered via the local menu and exited
/// with `Esc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentFocus {
    #[default]
    Compose,
    Transcript,
}

/// The live input surface of an agent window (Model C — `design-c.md`). There is
/// ONE model — a focusable read-only transcript plus a [`Compose`] buffer — and a
/// single mode axis, **placement**: `mode == Worksheet` renders the compose inline
/// at the transcript tail (the base case); `mode == Chatbox` renders it in a pinned
/// box (the diminutive case). The compose buffer therefore exists in BOTH modes —
/// the old enum (`Worksheet` unit | `Chatbox(Chatbox)`) is replaced by a struct so
/// "the compose exists iff in chatbox mode" is no longer the model. Toggling flips
/// `mode`; the `compose` value never moves (lossless). `Ctrl-Alt-Enter` toggles.
pub(crate) struct InputSurface {
    pub(crate) compose: Compose,
    /// The placement discriminant (`design-c.md` `Placement`): which side the
    /// editable buffer sits on. Persisted as the `PersistedSlot.mode` string;
    /// read by `should_follow_tail` and the render path.
    pub(crate) mode: InputModeKind,
}

impl InputSurface {
    /// A fresh surface in `mode` with an empty compose buffer.
    pub(crate) fn new(mode: InputModeKind) -> Self {
        Self {
            compose: Compose::new(),
            mode,
        }
    }

    /// A surface in `mode` whose compose is seeded with a persisted `draft`
    /// (Model C restore — `design-c.md` §4.4). Empty `draft` ⇒ empty compose.
    pub(crate) fn with_draft(mode: InputModeKind, draft: &str) -> Self {
        Self {
            compose: Compose::seeded(draft),
            mode,
        }
    }
    pub(crate) fn is_chatbox(&self) -> bool {
        self.mode == InputModeKind::Chatbox
    }
    /// The compose buffer — present in every mode (Model C). New code reads this.
    pub(crate) fn compose(&self) -> &Compose {
        &self.compose
    }
    pub(crate) fn compose_mut(&mut self) -> &mut Compose {
        &mut self.compose
    }
    /// Back-compat shim: `Some` only in Chatbox mode. Retained for the
    /// not-delivered resubmit path, which only refills the box in chatbox mode.
    /// New code uses the total `compose()`/`compose_mut()`.
    pub(crate) fn chatbox_mut(&mut self) -> Option<&mut Compose> {
        if self.is_chatbox() {
            Some(&mut self.compose)
        } else {
            None
        }
    }
    /// The Copy placement discriminant, for the persisted mode string and
    /// `should_follow_tail`.
    pub(crate) fn mode(&self) -> InputModeKind {
        self.mode
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
    /// The prompt the subagent was spawned with (the Task tool's `prompt` /
    /// `description` raw-input), shown in the subagent pane. `None` when the
    /// adapter didn't carry it.
    pub(crate) prompt: Option<String>,
}

/// Classify a tool call as a sub-agent (Task) spawn, as the HARNESS actually
/// emits it over ACP — not the old name heuristic.
///
/// claude-code-acp maps the `Task` tool to `ToolKind::Think` (the SAME kind as
/// `TodoWrite`), carrying the spawn in `raw_input` (`prompt` / `subagent_type` /
/// `description`). The previous classifier keyed on `kind == ToolKind::Other`,
/// which a real Task NEVER has — so subagents were invisible. This detects the
/// structured signal the harness sends:
///   - exclude `TodoWrite` (also `Think`) by its distinctive `todos` input;
///   - a `Think` call carrying a `prompt`/`subagent_type` IS a subagent;
///   - plus a name-prefix fallback (`Task`/`Subagent`/`Spawn`) for adapters
///     that only title it, kept so detection can't regress for them.
/// The captured `prompt` feeds the subagent pane (the user wants the prompt +
/// output visible, not just a label).
pub(crate) fn classify_subagent(tc: &yalda::acp_channel::ToolCall) -> Option<SubAgent> {
    use yalda::acp_channel::ToolKind;
    let raw = tc.raw_input.as_ref();
    // TodoWrite is also `Think`; its body is the running todo list, not a
    // subagent. Exclude it by the distinctive `todos` array (mirrors
    // `tool_render_policy`).
    if raw.and_then(|v| v.get("todos")).is_some() {
        return None;
    }
    let prompt = raw
        .and_then(|v| v.get("prompt").or_else(|| v.get("description")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let subagent_type = raw
        .and_then(|v| v.get("subagent_type"))
        .and_then(|v| v.as_str());
    // Structural: a `Think` tool call carrying a Task-shaped input.
    let structural = tc.kind == ToolKind::Think && (prompt.is_some() || subagent_type.is_some());
    // Fallback: titled like a subagent spawn (kept narrow so non-subagent
    // tools don't get swept in).
    let title = tc.title.as_str();
    let name_match = SUBAGENT_TOOL_NAMES
        .iter()
        .any(|prefix| title.starts_with(prefix));
    if !structural && !name_match {
        return None;
    }
    let label = if !title.is_empty() {
        title.to_string()
    } else if let Some(t) = subagent_type {
        format!("subagent: {t}")
    } else {
        "subagent".to_string()
    };
    Some(SubAgent {
        tool_call_id: ToolCallKey::from_id(&tc.tool_call_id),
        label,
        status: tc.status,
        prompt,
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

/// The editable draft buffer (Model C — `design-c.md`). Has its own document,
/// cursor, undo stack, and modal state. Shared by both placements: rendered
/// inline at the transcript tail in Worksheet mode, in a pinned box in Chatbox
/// mode. Its value survives a placement toggle untouched (the draft is never lost).
/// Renamed from `Chatbox` — it is the *compose surface*, not a mode.
pub(crate) struct Compose {
    pub(crate) editor: Editor,
    pub(crate) mode: EditMode,
    /// Virtualised scroll state for the compose panel, used ONLY once the draft
    /// exceeds the visible cap (`COMPOSE_MAX_VISIBLE_LINES`). Below that the
    /// panel renders every line directly (cheap); above it, building the whole
    /// draft every keystroke was O(draft) element assembly — the Message Box
    /// typing lag. `gpui::list` then builds only the visible rows (INV-2,
    /// matching the transcript + Edit view).
    /// Reconciled by splicing the changed range (never `reset()`) so scroll
    /// stays anchored — `reset()` snapped the box to its top on any newline in a
    /// >8-line draft. See `ScrollAnchoredList`.
    pub(crate) list: ScrollAnchoredList<String>,
    /// The authoritative visible top-left grid cell (spec-chatbox-caret-
    /// containment.md). Recomputed every render by `compose_window` from the
    /// current caret + measured box width; the list scrolls *to* `top_line`
    /// (never reads it back) and every row is sliced to the columns starting at
    /// `left_col`. `Cell` so the `&Chatbox` (or `&mut`) render path can store
    /// the new window without a wider borrow.
    pub(crate) window: std::cell::Cell<ComposeWindow>,
    /// Measured INNER content size `(x, y, w, h)` of the compose box, written
    /// during paint via `CaptureBounds`, read next frame to derive
    /// `visible_cols = floor(w / CHATBOX_CHAR_W)`. The real painted width — not
    /// the whole-window `viewport_width_px`, which over-counts by the split tree
    /// + sidebars + padding and would itself strand the caret (review B1).
    pub(crate) bounds: std::rc::Rc<std::cell::Cell<(f32, f32, f32, f32)>>,
}

/// Conservative monospace column advance for the compose grid, in px. Set equal
/// to the caret block width (`make_caret`, 8px) — slightly WIDER than the real
/// ~7.8px advance of 13px SF Mono/Menlo — so `visible_cols = floor(box_w / this)`
/// under-counts columns (never over-counts → the caret can't be scrolled to a
/// column that's actually clipped) and the one reserved right column exactly
/// covers the caret block (spec Behavior 2). Slicing (not pixel-offset) is what
/// makes an approximate advance safe: rows render from the slice's left edge, so
/// there is no per-column pixel drift to accumulate.
pub(crate) const CHATBOX_CHAR_W: f32 = 8.0;

impl Compose {
    pub(crate) fn new() -> Self {
        Self {
            editor: Editor::new(String::new(), std::path::PathBuf::from("*compose*")),
            mode: EditMode::Insert,
            // Compose rows are uniform 18px and non-wrapping; the default item
            // height MUST match so an unmeasured (freshly-spliced) row estimates
            // its height correctly. A wrong default (was 64px, ~3.5× too tall)
            // throws off the list's height model and strands the caret off-screen
            // on reveal — the recurring "cursor offscreen in the chatbox" bug.
            list: ScrollAnchoredList::new(gpui::ListAlignment::Top, gpui::px(18.0)),
            window: std::cell::Cell::new(ComposeWindow::default()),
            bounds: std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0, 0.0, 0.0))),
        }
    }

    pub(crate) fn text(&self) -> String {
        self.editor.document().full_text()
    }

    /// Recompute the caret-containment window from the CURRENT editor state and
    /// the given visible extent, store it (authoritative), and return it
    /// (spec-chatbox-caret-containment.md). The single integration point that
    /// reads the cursor + the caret line's tab-expanded length and feeds
    /// `compose_window` — kept here (not inline in the GPUI render path) so it is
    /// headlessly testable over every edit path (Constraint 4).
    pub(crate) fn compute_window(&self, visible_rows: usize, visible_cols: usize) -> ComposeWindow {
        let line_count = self.editor.document().line_count().max(1);
        let cursor = self.editor.cursor();
        // Length of the caret's line in the SAME representation the rows render
        // in (tabs expanded to 4 spaces), so the horizontal clamp matches what's
        // painted.
        let cursor_line_len = {
            let cl = cursor.line.min(line_count.saturating_sub(1));
            self.editor
                .document()
                .line_text(cl)
                .trim_end_matches('\n')
                .replace('\t', "    ")
                .chars()
                .count()
        };
        let win = compose_window(
            cursor.line,
            cursor.col,
            cursor_line_len,
            self.window.get(),
            line_count,
            visible_rows,
            visible_cols,
        );
        self.window.set(win);
        win
    }

    /// A fresh compose seeded with `text` (cursor at the end). Used on restore to
    /// re-apply a persisted draft, and on the not-delivered resubmit path.
    pub(crate) fn seeded(text: &str) -> Self {
        let mut c = Self::new();
        for ch in text.chars() {
            c.editor.insert_char(ch);
        }
        c
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
    /// Fingerprint of the frozen-line *layout* (ranges, not just count) when
    /// `block_cache` / `resolved_blocks` / the editor's atomic-block set were
    /// last (re)validated. A hash of the ranges, so re-detection fires when a
    /// frozen block *moves* (e.g. the user inserts an editable line between two
    /// frozen blocks) and not only when the frozen line count changes — a count
    /// gate alone leaves `resolved_blocks` pointing at stale line indices after
    /// such an insert. `None` forces a rebuild (cold / theme-invalidated).
    pub(crate) block_cache_frozen_fp: Option<u64>,
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
    /// Sorted doc lines the WORKSHEET caret is allowed to rest on (block-paged
    /// navigation): every editable line, plus the first line of each frozen
    /// PROSE run. Tool groups, code/table blocks and interior/blank frozen lines
    /// are deliberately absent — they're crossed in a single keystroke and never
    /// host the caret (they render as one element with no per-line caret). Built
    /// from the same final `flat_items` as `line_to_item_cache`.
    pub(crate) nav_stops_cache: std::rc::Rc<Vec<usize>>,
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

    /// Drop the theme-dependent caches so the next render re-resolves under the
    /// CURRENT theme. Fenced code blocks + tables bake their span colors
    /// (background + syntect foregrounds) into the parsed `RenderedBlock` at
    /// parse time, and `block_cache` keys on content — not theme — and only
    /// re-parses when the frozen line count changes. So after a theme toggle a
    /// block parsed under the old theme keeps its stale colors (e.g. a Folio
    /// light-on-light code box surviving into Nightfox). Clearing the block
    /// cache + forcing the frozen-count guard to miss re-parses every block;
    /// dropping `view_model_fp` forces the S1 flat-items rebuild that consumes
    /// them. Call on `set_theme` for every live session.
    pub(crate) fn invalidate_theme(&mut self) {
        self.block_cache.clear();
        self.block_cache_frozen_fp = None;
        self.view_model_fp = None;
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
        nav_stops: Vec<usize>,
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
        self.nav_stops_cache = std::rc::Rc::new(nav_stops);
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

    /// Snap a worksheet vertical-move landing line to the nearest navigable stop
    /// in the move direction (block-paged navigation). `down` picks the first
    /// stop at-or-after `line`; otherwise the last stop at-or-before it. `None`
    /// when there is no stop in that direction (caller leaves the cursor put).
    /// Empty cache (no render yet) ⇒ `None`, so motion falls back to raw.
    pub(crate) fn snap_nav_stop(&self, line: usize, down: bool) -> Option<usize> {
        // `nav_stops_cache` is sorted ascending — binary-search so a vertical
        // keystroke on a huge transcript stays O(log n), not O(stops).
        let s = &self.nav_stops_cache;
        if down {
            // first stop >= line
            s.get(s.partition_point(|&l| l < line)).copied()
        } else {
            // last stop <= line
            let n = s.partition_point(|&l| l <= line);
            (n > 0).then(|| s[n - 1])
        }
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

/// Doc lines the worksheet caret may rest on, in ascending order (the
/// frozen-BLOCK navigation model). A line is a stop iff it renders as a
/// caret-bearing `FlatItem::Line` that is EITHER editable (every editable line
/// is its own stop, so text editing keeps per-line motion) OR a non-blank frozen
/// PROSE line — and per the model "a single frozen line terminated by a newline
/// is a block," EVERY such prose line is its own stop. That is what lets the
/// caret land between any two adjacent frozen prose lines to insert there.
///
/// What is NOT a stop, and so is crossed in a single keystroke: `FlatItem::Block`
/// (a fenced code block / table — one atomic block, no `FlatItem::Line`s for its
/// interior), `FlatItem::ToolGroup`, turn headers, the thinking indicator, and
/// blank frozen padding (stripped from the render). Because a structural block
/// emits no `Line`, it naturally contributes no stop; the caret jumps over it to
/// the next prose/editable stop.
pub(crate) fn build_nav_stops(
    flat_items: &[FlatItem],
    lines: &[String],
    frozen: &[(usize, usize)],
) -> Vec<usize> {
    let is_frozen = |idx: usize| frozen.iter().any(|&(s, e)| idx >= s && idx < e);
    let mut stops = Vec::new();
    // (1) Rendered text lines: every non-blank frozen prose line is a stop;
    // blank frozen padding is not. Tool groups / blocks own no `Line` item, so
    // they're skipped (crossed in one keystroke).
    for item in flat_items {
        if let FlatItem::Line(idx) = item {
            let blank_frozen =
                is_frozen(*idx) && lines.get(*idx).is_none_or(|s| s.trim().is_empty());
            if !blank_frozen {
                stops.push(*idx);
            }
        }
    }
    // (2) EVERY editable line is a stop, independent of `flat_items` — so the
    // editable tail is always reachable even when it's a lone blank line the
    // blank-collapse pass stripped from the rendered list. Without this you
    // could navigate up into the transcript and never get the caret back down
    // to where you type (and then `i` lands on a frozen line → can't insert).
    for l in 0..lines.len() {
        if !is_frozen(l) {
            stops.push(l);
        }
    }
    stops.sort_unstable();
    stops.dedup();
    stops
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
    // Re-validated whenever the frozen *layout* changes — a streamed
    // chunk, a submit, OR an editable line inserted between two frozen
    // blocks (which shifts a block's line indices without changing the
    // frozen line count). A worksheet keystroke in the editable tail
    // leaves the layout (and this) untouched. Parses are reused by
    // CONTENT (see `AgentViewModel::block_cache`), so a chunk that
    // shifts every range only re-parses ranges whose text actually
    // changed. Rejected ranges resolve to `None` and render as their
    // source lines (Finding 10, INV-10).
    let frozen_layout_fp = frozen_ranges_fp(frozen_ranges);
    if Some(frozen_layout_fp) != c.view_model.block_cache_frozen_fp {
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
        // Seed the editor's atomic-block set so `can_insert_char_at` forbids
        // splitting these multi-line structural blocks. Detected ranges are the
        // atomic blocks whether or not they parsed (an unparsed-but-detected
        // fence/table is still structural and must stay whole); single frozen
        // prose lines are never detected, so they remain individually
        // insertable-between.
        c.editor.set_atomic_blocks(block_ranges.clone());
        c.block_ranges = block_ranges;
        c.view_model.block_cache = new_cache;
        c.view_model.resolved_blocks = resolved;
        c.view_model.block_cache_frozen_fp = Some(frozen_layout_fp);
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
    // "You" divider: presence-driven. It appears the moment you enter Insert mode
    // on an editable run (you're composing a turn there), even before you type,
    // and vanishes when you leave Insert without committing text (so no phantom
    // "You"). `composing` + the caret line are folded into
    // `view_model_fingerprint` (caret only WHILE composing) so this updates live
    // without busting the memo on Normal-mode navigation.
    // Model C: the transcript is READ-ONLY and the user draft lives in the
    // separate Compose buffer — so the transcript never hosts a presence-driven
    // "You" divider (that boundary is the inline compose's own gutter/border,
    // rendered in screens.rs). `composing` is therefore always false here: the
    // editable-run divider is a Model-A artifact and must not inject spurious
    // "You" headers into the frozen transcript.
    let composing = false;
    let caret_line = c.editor.cursor().line;
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
            // Emit the "You" header only when THIS editable run actually holds
            // text. The run is the contiguous untagged region starting here, up
            // to the next header-bearing (Llm/User) line — NOT the whole rest of
            // the document. Scanning to EOF made any blank editable gap wedged
            // above downstream Claude content sprout a phantom "You" header
            // (the reported bug): the downstream Claude lines are non-blank, so
            // the gap looked like a user turn. Tool/System lines carry a tag but
            // no header role, so they don't bound the run.
            let run_end = (line_idx..lines.len())
                .find(|&j| {
                    gutter_tag_per_line
                        .get(j)
                        .copied()
                        .flatten()
                        .and_then(HeaderRole::from_turn)
                        .is_some()
                })
                .unwrap_or(lines.len());
            // The "You" divider is PRESENCE-driven (user contract): it appears
            // when this editable run holds non-whitespace text OR when the user is
            // composing in it (Insert mode with the caret inside the run) — so it
            // shows the instant you enter Insert, before you've typed. An empty
            // run with no caret shows none; leaving Insert without committing text
            // makes it vanish (no phantom "You"). Caret/Insert are folded into
            // `view_model_fingerprint` so this updates live.
            let run_non_empty = (line_idx..run_end).any(|j| !lines[j].trim().is_empty());
            let caret_in_run = composing && (line_idx..run_end).contains(&caret_line);
            if run_non_empty || caret_in_run {
                flat_items.push(FlatItem::TurnHeader {
                    role: TurnRole::User,
                });
                // A real user turn (or one being composed) ends the prior turn;
                // the next Llm line re-opens with its own header.
                prev_turn = None;
            }
            // All-blank/whitespace gap with no caret: leave `prev_turn` intact so
            // an abandoned editable gap mid-Claude-turn doesn't split it.
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
    //
    // EXCEPTION: in Worksheet mode the editable tail lives in the
    // transcript, so the caret can sit on one of these blank Lines
    // (e.g. you press Enter on the empty tail). Stripping that Line
    // makes the caret vanish — `line_idx == cursor_line` never matches
    // a rendered row — and routes the cursor-reveal to the wrong item
    // (`item_for_line` falls back to the last item), so the viewport
    // scrolls past where the cursor actually is. Never collapse the
    // line the cursor is on.
    {
        let protect_line: Option<usize> = (!c.input_surface.is_chatbox())
            .then(|| c.editor.cursor().line);
        let is_blank_line = |item: &FlatItem| -> bool {
            matches!(item, FlatItem::Line(idx)
                if Some(*idx) != protect_line && lines.get(*idx).is_none_or(|s| s.trim().is_empty()))
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
            // Drop a blank editable line that is the LAST rendered item — a
            // trailing editable blank below the user's text renders as a stray
            // empty row at the bottom of the transcript ("extra blank newline").
            // The caret's own line is never blank here (it's `protect_line`), so
            // this only strips a tail the caret has moved off of.
            if i + 1 == flat_items.len() {
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

    // INVARIANT — no empty turn header (INV-UX-4): a `You`/`Claude` divider is
    // rendered only for a turn that actually has visible content (a Line / Block /
    // ToolGroup / ThinkingIndicator) before the next header. The blank-collapse
    // pass can strip a turn's only (blank) lines, and a run of content-less turns
    // (blank-tagged separators between exchanges, or tool-numbered blank lines on
    // resume) would otherwise render as a stack of empty alternating `You`/
    // `Claude` dividers — the reported "blank turns" bug. Scan right→left: a
    // header is dropped unless some non-header item was seen since the previous
    // kept header. Runs AFTER the thinking indicator so a trailing in-flight
    // `Claude` header keeps its spinner as content.
    {
        let mut keep = vec![true; flat_items.len()];
        let mut content_since_header = false;
        for i in (0..flat_items.len()).rev() {
            if matches!(flat_items[i], FlatItem::TurnHeader { .. }) {
                if !content_since_header {
                    keep[i] = false;
                }
                content_since_header = false;
            } else {
                content_since_header = true;
            }
        }
        let mut j = 0;
        #[allow(clippy::needless_range_loop)]
        for i in 0..flat_items.len() {
            if keep[i] {
                flat_items.swap(i, j);
                j += 1;
            }
        }
        flat_items.truncate(j);
    }

    // Derive the cursor-reveal reverse index from the FINAL flat_items (after
    // blank-collapse, tool-group merge, empty-header removal, and the tail
    // indicator) so it matches the rendered list exactly.
    let line_to_item = build_line_to_item(&flat_items, resolved, lines.len());
    let nav_stops = build_nav_stops(&flat_items, lines, frozen_ranges);

    c.view_model.store(
        view_model_fp,
        flat_items,
        gutter_tag_per_line,
        line_to_item,
        nav_stops,
    )
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
    /// User-turn jump mode (agent (space) menu → "jump between user turns"): when
    /// on, bare `j`/`k` in Normal mode move the viewport between the user's
    /// input turns (`TurnHeader { role: User }`) instead of moving the editor
    /// cursor — for finding "what I wrote last" amid a wall of agent output.
    pub(crate) user_turn_jump_mode: bool,
    /// Which user turn (0-based ordinal among the transcript's user
    /// `TurnHeader`s) the jump cursor is currently parked on. Clamped to the
    /// live count each step; `k` decrements (older), `j` increments (newer).
    pub(crate) user_turn_jump_ord: usize,
    /// Set by a jump keystroke to request "reveal the Nth user turn on the next
    /// render". The render path resolves the ordinal to a flat-item index
    /// against the FRESH list and issues the scroll (same INV-RV discipline as
    /// `pending_reveal_cursor`). `None` = no pending jump.
    pub(crate) pending_jump_ord: Option<usize>,
    /// Set alongside `pending_jump_ord` when the jump lands at the buffer's
    /// page end (a `j` pressed while already on the newest user turn) rather
    /// than on a user-turn header. The render path reveals the LAST flat item
    /// instead of resolving the ordinal. Consumed (`take`) with the ordinal.
    pub(crate) pending_jump_end: bool,
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
    /// The active input surface (Model C — `design-c.md`): the `Compose` draft
    /// buffer + the placement (`Worksheet` inline / `Chatbox` pinned). New
    /// sessions start in `Chatbox`; `Ctrl-Alt-Enter` toggles placement.
    pub(crate) input_surface: InputSurface,
    /// Which surface holds keyboard focus (Model C — `design-c.md` §4.5).
    /// `Compose` (default): keystrokes edit the draft. `Transcript`: keystrokes
    /// drive read-only navigation/selection over the committed transcript, and
    /// the transcript renders a caret. This is the base capability that makes
    /// Worksheet a *workspace* (select history, `S` = send selection), shared by
    /// both placements. `Esc` from `Transcript` returns to `Compose`.
    pub(crate) focus: AgentFocus,
    /// Worksheet inline-edit (spec-worksheet.md / INV-UX-9): whether a **You-block**
    /// is currently open — i.e. the `Compose` is acting as an inline editable reply
    /// attached to the transcript while the agent is idle. `false` = the worksheet
    /// is in pure navigation (no compose chrome shown). Entering Insert from
    /// transcript navigation opens it; leaving Insert with only whitespace discards
    /// it (rule 3); Submit freezes + clears it (rule 4). Only meaningful in
    /// Worksheet mode; the mid-turn chatbox (rule 7) is derived from `turn_phase`,
    /// not from this flag.
    pub(crate) you_block_open: bool,
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
    /// The transcript-structure generation counter — bumped on every mutation
    /// to the tool-call cluster (`calls`/`order`/`expanded`). Ticket 021's
    /// observe filter compares it across renders so a tool start / update /
    /// expand re-renders the transcript while a chatbox keystroke (which never
    /// touches tools) does not. A genuine monotonic counter (not a derived
    /// fingerprint): `ToolCalls` already maintains it for its snapshot caches.
    pub(crate) fn tools_gen(&self) -> u64 {
        self.tools.snap_gen
    }

    /// A monotonic-equivalent fingerprint of the frozen-line set (ticket 021's
    /// observe filter). Frozen ranges are append/merge-only in this transcript
    /// model, so `(range_count, total_frozen_lines, last_range_end)` changes
    /// iff the frozen set moved — including the pure `add_frozen_lines` path
    /// (a worksheet commit that freezes already-present lines) which does NOT
    /// bump `edit_seq`. Deriving it from the live state (rather than threading
    /// a hand-maintained counter through every freeze site across `editor.rs`)
    /// is deliberate: it cannot drift from a missed mutation site — the exact
    /// "missed dependency" failure mode the ticket's risk section warns about.
    /// O(1): reads only the range vec's len + last element.
    pub(crate) fn frozen_gen(&self) -> u64 {
        let ranges = self.editor.frozen_lines();
        let count: usize = ranges.iter().map(|(s, e)| e - s).sum();
        let last_end = ranges.last().map(|&(_, e)| e).unwrap_or(0);
        // Pack the three monotonic-ish signals into one u64. Each component is
        // small (line counts on a transcript), so collisions across a real
        // mutation are not reachable in practice.
        ((ranges.len() as u64) << 40) ^ ((count as u64) << 20) ^ (last_end as u64)
    }

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
        // The blank-collapse pass is mode- and cursor-sensitive: in Worksheet
        // mode `protect_line` keeps the caret's (possibly blank) line so the
        // caret has a row to render on, and toggling surfaces flips whether the
        // trailing editable blank is the compose tail (kept) or stray noise
        // (stripped). Both are genuine inputs to the build, so the memo key must
        // name them — otherwise entering Worksheet mode (or moving the worksheet
        // caret onto the collapsible tail) reuses a flat list built for the
        // other state and the caret lands on a stripped line "below the visible
        // buffer". Folded in WORKSHEET ONLY: a chatbox transcript caret never
        // drives collapse, and insert-mode typing already busts the memo via
        // `edit_seq`, so this adds rebuilds only on Normal-mode worksheet
        // navigation (cheap — O(changed) S1 rebuild, INV-RV).
        (!self.input_surface.is_chatbox()).hash(&mut h);
        if !self.input_surface.is_chatbox() {
            self.editor.cursor().line.hash(&mut h);
            // Insert vs Normal flips the PRESENCE-driven "You" divider: it shows
            // while composing (Insert mode with the caret in an editable run),
            // even before any text. Entering/leaving Insert without moving the
            // caret or editing changes nothing else in this key, so the memo must
            // name the mode or the divider wouldn't appear/disappear live.
            (self.mode == EditMode::Insert).hash(&mut h);
        }
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

            status: None,
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            pending_reveal_cursor: false,
            user_turn_jump_mode: false,
            user_turn_jump_ord: 0,
            pending_jump_ord: None,
            pending_jump_end: false,
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::new(InputModeKind::Chatbox),
            focus: AgentFocus::default(),
            you_block_open: false,
            current_plan: None,
            agent_mode: None,
            agent_model: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            // Subagent panes auto-appear at the tile bottom when subagents exist;
            // Cmd-2 (ToggleSubagents) collapses them.
            subagents_open: true,
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

            status,
            turn_phase: TurnPhase::Idle,
            replay_turns: yalda::acp_channel::ReplayTurns::default(),
            tools: ToolCalls::default(),
            block_ranges: Vec::new(),
            pending_reveal_cursor: false,
            user_turn_jump_mode: false,
            user_turn_jump_ord: 0,
            pending_jump_ord: None,
            pending_jump_end: false,
            view_model: AgentViewModel::new(),
            lines_cache: std::rc::Rc::new(Vec::new()),
            lines_cache_seq: u64::MAX,
            highlight_cache: HighlightCache::new(),
            input_surface: InputSurface::new(InputModeKind::Chatbox),
            focus: AgentFocus::default(),
            you_block_open: false,
            current_plan: None,
            agent_mode: None,
            agent_model: None,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            usage: None,
            focused_subagent: None,
            tasklist_open: false,
            // Subagent panes auto-appear at the tile bottom when subagents exist;
            // Cmd-2 (ToggleSubagents) collapses them.
            subagents_open: true,
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
        // The follow-output scroll handler is wired by the owning
        // `TranscriptView` (ticket 021): the `ListState` now lives in
        // `TranscriptScroll`, not on `AgentState`.
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
    ///
    /// Model C: the live worksheet submit no longer freezes in place — it routes
    /// through `submit_compose` → `insert_user_turn` (append at EOF), since the
    /// draft now lives in a separate `Compose` buffer, not in the transcript.
    /// This in-place freeze is retained only as the reconciler-dedup SEAM that
    /// the `agent_seam_*` tests drive directly; hence `#[cfg(test)]`.
    #[cfg(test)]
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
        // Freeze ONLY the lines that actually contributed to `prompt_body` (the
        // non-blank ones). A blank spacer line collected here but filtered out of
        // the sent body must NOT be frozen as part of the turn — doing so paints
        // an empty "You" region into the transcript (the reported bug). Blank
        // lines stay editable and collapse in render.
        for (l, t) in collected {
            if t.trim().is_empty() {
                continue;
            }
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
        should_follow_tail(self.follow_output.get())
    }

    /// Fold the replay cursor back into the live counter at end-of-replay
    /// (Finding 13, INV-4).
    pub(crate) fn finish_replay(&mut self) {
        self.replay_turns.finish_replay();
        // Reopening a session that was in Worksheet mode rebuilds the transcript
        // from the replayed event_log; land the caret on the editable tail (the
        // last line) so the user finds it where they compose, not stranded at
        // its line-0 birth position or on replayed agent content. Worksheet only
        // — Chatbox composes in a separate surface and leaves the transcript
        // caret untouched (mirrors `finalize_agent_turn_idem`).
        if !self.input_surface.is_chatbox() {
            self.move_cursor_to_tail();
        }
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
        // In Worksheet mode the caret IS the compose point, so once a turn
        // settles drop it to the editable tail — the user composes their next
        // message right below the agent's reply, and the viewport follows there.
        // Chatbox composes in a separate surface, so its transcript caret is
        // left where it is.
        if !self.input_surface.is_chatbox() {
            self.move_cursor_to_tail();
        }
        true
    }

    /// Drop the worksheet caret to the end of the editable tail (the last line)
    /// and queue a reveal so the viewport scrolls to it. `finalize_agent_turn`
    /// guarantees a trailing newline, so the last line is the empty editable
    /// compose row (or the end of an in-progress draft).
    pub(crate) fn move_cursor_to_tail(&mut self) {
        let last = self.editor.document().line_count().saturating_sub(1);
        let col = self.editor.document().line_len_chars(last);
        self.editor.cursor_mut().line = last;
        self.editor.cursor_mut().col = col;
        self.pending_reveal_cursor = true;
        self.follow_output.set(true);
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
        // The list/scroll UI state (`list_state`, counts, watermarks) now lives
        // in the owning `TranscriptView`'s `TranscriptScroll` (ticket 021). A
        // replay wipes the transcript ⇒ the flat-item count drops, and the
        // view's next render reconciles the `ListState` down (a shrink resets
        // it) and re-reveals the tail from the fresh `edit_seq` (which restarts
        // at 0 on the new editor) — no explicit list reset needed here.
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
///
/// The payload is an `Entity<AgentSession>` (not a bare `AgentSession`): per-
/// session state lives in a GPUI entity so the framework's invalidation
/// (notify-at-mutation-site + observation) has per-session granularity. The
/// store's 1:1 sid invariant is untouched — `SessionStore<P>` is payload-
/// generic, so this is purely a payload-type swap. Mutate via
/// `entity.update(cx, |s, cx| { …; cx.notify() })`, read via `entity.read(cx)`;
/// the `with_session` / `read_session` shims on `YaldaGpuiView` keep the call
/// sites terse.
pub(crate) type AgentSessions = SessionStore<Entity<AgentSession>>;

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
    /// Highlighted row. Row 0 is always "start a new session"; rows `1..=N` map
    /// to the FREE sessions projected from the universal roster for the active
    /// workspace's cwd (`picker_projection`, universal-agent-list). UI state
    /// ONLY — neither the rows NOR the cwd are cached here: a rendered picker is
    /// always on the active tab, so its cwd is read live from `agent_base_cwd`
    /// at render/select time. (Caching the cwd here is what made "Set CWD, then
    /// Start a new session" create the agent in the *old* dir.)
    pub(crate) selected: usize,
}

impl SessionPicker {
    pub(crate) fn new() -> Self {
        Self { selected: 0 }
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
