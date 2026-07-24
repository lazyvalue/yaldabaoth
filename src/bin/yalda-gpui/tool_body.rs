//! Beautiful tool-call body rendering (UXI-AgentTile-25).
//!
//! Splits into a **pure planning layer** (`plan_tool_sections` — JSON → data,
//! headless-testable, no gpui) and a **render layer** (`render_tool_section` /
//! `append_tool_body_rich` — data → gpui). This replaces the old pipeline where
//! every tool input/output was `serde_json::to_string_pretty`'d into a monospace
//! blob: now a Task's prompt + report render as MARKDOWN, a Bash command renders
//! as a code block, an Edit renders as a diff, paths/patterns render as labeled
//! chips, and only genuinely unknown shapes fall back to JSON.
//!
//! Both call sites — the focused-subagent view (`screens.rs`) and the inline
//! transcript tool groups (`transcript_view.rs`) — render through here.

use super::*;

/// Hard cap on the bytes of any single text payload before it is parsed +
/// rendered, so a pathological megabyte output can't stall a frame. The head is
/// kept and a truncation note appended.
const TOOL_TEXT_MAX_BYTES: usize = 96 * 1024;
/// Cap on markdown blocks rendered for a tool section in the INLINE transcript
/// (the focused-subagent view renders all). Keeps a giant report from ballooning
/// a transcript row; a "+N more blocks" footer signals the rest.
const TOOL_MARKDOWN_MAX_BLOCKS_INLINE: usize = 40;

/// Which side of a tool call a section shows. Drives the tile's role color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionRole {
    Input,
    Output,
}

/// How a section's body renders.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SectionBody {
    /// `label → value` chips (path, pattern, agent type, url, …), rendered
    /// inline and wrapped. Values use the code font (identifiers/paths).
    Chips(Vec<(String, String)>),
    /// One block of proportional prose (a description / one-liner).
    Prose(String),
    /// Monospace code, optional syntect language hint, optional line cap.
    Code {
        text: String,
        max_lines: Option<usize>,
    },
    /// Full CommonMark (a subagent report, MCP text, a markdown prompt). Rendered
    /// via the doc block renderer.
    Markdown { text: String },
    /// `+`/`-`/`@@` diff coloring with an optional path header.
    Diff {
        header: Option<String>,
        text: String,
    },
    /// Pretty-printed JSON — last resort for shapes we don't recognise.
    Json(String),
}

/// One rendered section of a tool call's body.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolSection {
    pub(crate) role: SectionRole,
    /// Semantic label ("prompt", "command", "report", "diff", "output", …) —
    /// NOT generic "input"/"output". Shown small + uppercase on the tile.
    pub(crate) label: &'static str,
    /// Emphasized tile (warm-accent border) — the subagent's report, the star.
    pub(crate) emphasis: bool,
    pub(crate) body: SectionBody,
}

impl ToolSection {
    fn input(label: &'static str, body: SectionBody) -> Self {
        Self { role: SectionRole::Input, label, emphasis: false, body }
    }
    fn output(label: &'static str, body: SectionBody) -> Self {
        Self { role: SectionRole::Output, label, emphasis: false, body }
    }
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())
}

/// A short, one-line value for a chip — clip very long values so a chip row
/// stays one line (StyledText doesn't wrap here).
fn chip_value(s: &str) -> String {
    let one = s.lines().next().unwrap_or(s);
    if one.chars().count() > 80 {
        let clipped: String = one.chars().take(80).collect();
        format!("{clipped}…")
    } else {
        one.to_string()
    }
}

/// Does this content string look like a diff (so it should keep +/- coloring
/// rather than render as prose/markdown)? Matches the shape
/// `render_tool_content_blocks` emits for `ToolCallContent::Diff`.
fn looks_like_diff(s: &str) -> bool {
    s.lines().any(|l| {
        l.starts_with("--- ")
            || l.starts_with("+++ ")
            || l.starts_with("@@ ")
            || l == "+ (new)"
            || l == "- (old)"
    })
}

/// Is this tool's textual output terminal-style (NOT markdown)? Execute/Bash and
/// Fetch-of-raw stdout should render monospace — a leading `#` is a shell
/// comment, not an H1.
fn output_is_terminal(kind: &yalda::acp_channel::ToolKind) -> bool {
    matches!(kind, yalda::acp_channel::ToolKind::Execute)
}

/// PURE: plan the sections for a tool call's expanded body (UXI-AgentTile-25).
/// All per-tool-kind branching, output-text extraction, and markdown-vs-code
/// decisions live here so they're headless-testable with zero gpui. The render
/// layer just walks the result.
pub(crate) fn plan_tool_sections(
    tc: &yalda::acp_channel::ToolCall,
    policy: ToolRenderPolicy,
) -> Vec<ToolSection> {
    use yalda::acp_channel::ToolKind;
    let mut out: Vec<ToolSection> = Vec::new();
    let input = tc.raw_input.as_ref();
    let code_max = match policy {
        ToolRenderPolicy::Truncated { max_lines } => Some(max_lines),
        _ => None,
    };

    // ── INPUT ────────────────────────────────────────────────────────────
    let is_subagent = classify_subagent(tc).is_some();
    if is_subagent {
        if let Some(v) = input {
            if let Some(t) = str_field(v, "subagent_type") {
                out.push(ToolSection::input("agent", SectionBody::Chips(vec![("agent".into(), t.into())])));
            }
            if let Some(d) = str_field(v, "description") {
                out.push(ToolSection::input("task", SectionBody::Prose(d.into())));
            }
            if let Some(p) = str_field(v, "prompt") {
                out.push(ToolSection::input("prompt", SectionBody::Markdown { text: p.into() }));
            }
        }
    } else if let Some(v) = input {
        match tc.kind {
            ToolKind::Execute => {
                if let Some(d) = str_field(v, "description") {
                    out.push(ToolSection::input("task", SectionBody::Prose(d.into())));
                }
                if let Some(cmd) = str_field(v, "command") {
                    out.push(ToolSection::input(
                        "command",
                        SectionBody::Code { text: cmd.into(), max_lines: code_max },
                    ));
                }
            }
            ToolKind::Read => {
                let mut chips = Vec::new();
                if let Some(fp) = str_field(v, "file_path") {
                    chips.push(("path".into(), chip_value(fp)));
                }
                if let Some(o) = v.get("offset").and_then(|x| x.as_i64()) {
                    let l = v.get("limit").and_then(|x| x.as_i64());
                    chips.push(("lines".into(), match l {
                        Some(l) => format!("{o}..{}", o + l),
                        None => format!("{o}.."),
                    }));
                }
                if !chips.is_empty() {
                    out.push(ToolSection::input("read", SectionBody::Chips(chips)));
                }
            }
            ToolKind::Edit => {
                if let Some(fp) = str_field(v, "file_path") {
                    out.push(ToolSection::input("path", SectionBody::Chips(vec![("path".into(), chip_value(fp))])));
                }
                // If the tool didn't attach a Diff in `content`, synthesize one
                // from old/new so an edit still shows as a diff, not JSON.
                let content_has_diff = looks_like_diff(&render_tool_content_blocks(&tc.content));
                if !content_has_diff {
                    if let (Some(old), Some(new)) = (str_field(v, "old_string"), str_field(v, "new_string")) {
                        let mut text = String::new();
                        for l in old.lines() {
                            text.push_str(&format!("- {l}\n"));
                        }
                        for l in new.lines() {
                            text.push_str(&format!("+ {l}\n"));
                        }
                        out.push(ToolSection::input("diff", SectionBody::Diff { header: None, text }));
                    }
                }
            }
            ToolKind::Move | ToolKind::Delete => {
                let mut chips = Vec::new();
                for key in ["path", "file_path", "source", "destination", "old_path", "new_path"] {
                    if let Some(s) = str_field(v, key) {
                        chips.push((key.to_string(), chip_value(s)));
                    }
                }
                if !chips.is_empty() {
                    out.push(ToolSection::input("files", SectionBody::Chips(chips)));
                }
            }
            ToolKind::Search => {
                let mut chips = Vec::new();
                for key in ["pattern", "path", "glob", "include", "type", "output_mode"] {
                    if let Some(s) = str_field(v, key) {
                        chips.push((key.to_string(), chip_value(s)));
                    }
                }
                if !chips.is_empty() {
                    out.push(ToolSection::input("search", SectionBody::Chips(chips)));
                }
            }
            ToolKind::Fetch => {
                if let Some(u) = str_field(v, "url") {
                    out.push(ToolSection::input("url", SectionBody::Chips(vec![("url".into(), chip_value(u))])));
                }
                if let Some(p) = str_field(v, "prompt") {
                    out.push(ToolSection::input("prompt", SectionBody::Prose(p.into())));
                }
            }
            // Write (Edit kind covers edits; a create/write may be Other): handled
            // by the structured fallback, which renders `content` as code.
            _ => {
                out.extend(fallback_input_sections(v));
            }
        }
        // Write specifically: an Edit-kind create with `content` renders as code.
        if tc.kind == ToolKind::Edit
            && let Some(content) = str_field(v, "content")
        {
            out.push(ToolSection::input("content", SectionBody::Code {
                text: content.into(),
                max_lines: Some(40),
            }));
        }
    }

    // ── CONTENT (tc.content: text / diff) ────────────────────────────────
    let content_text = render_tool_content_blocks(&tc.content);
    let content_trimmed = content_text.trim().to_string();
    if !content_trimmed.is_empty() {
        if looks_like_diff(&content_text) {
            out.push(ToolSection::output("diff", SectionBody::Diff { header: None, text: content_text.clone() }));
        } else if output_is_terminal(&tc.kind) {
            out.push(ToolSection::output("output", SectionBody::Code { text: content_text.clone(), max_lines: code_max }));
        } else {
            let emphasis = is_subagent;
            out.push(ToolSection {
                role: SectionRole::Output,
                label: if is_subagent { "report" } else { "output" },
                emphasis,
                body: SectionBody::Markdown { text: content_text.clone() },
            });
        }
    }

    // ── OUTPUT (tc.raw_output) ───────────────────────────────────────────
    if let Some(output) = &tc.raw_output {
        match extract_output_text(output) {
            Some(text) if text.trim() != content_trimmed => {
                if output_is_terminal(&tc.kind) {
                    out.push(ToolSection::output("output", SectionBody::Code { text, max_lines: code_max }));
                } else {
                    let emphasis = is_subagent;
                    out.push(ToolSection {
                        role: SectionRole::Output,
                        label: if is_subagent { "report" } else { "output" },
                        emphasis,
                        body: SectionBody::Markdown { text },
                    });
                }
            }
            // Same text as `content` (Claude Code mirrors output into content) —
            // suppress the duplicate.
            Some(_) => {}
            // No clean text payload → pretty JSON fallback, unless it's empty.
            None => {
                let pretty = serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string());
                if !pretty.trim().is_empty() && pretty.trim() != "null" && pretty.trim() != "{}" {
                    out.push(ToolSection::output("output", SectionBody::Json(pretty)));
                }
            }
        }
    }

    out
}

/// Structured fallback for an unknown tool INPUT object (§4): scalars become one
/// chips row; long / multiline strings each become their own labeled code
/// section (real newlines, not `\n`-riddled JSON); anything left nests into a
/// single JSON section.
fn fallback_input_sections(v: &serde_json::Value) -> Vec<ToolSection> {
    use serde_json::Value;
    let mut out = Vec::new();
    let Value::Object(map) = v else {
        let pretty = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
        if !pretty.trim().is_empty() {
            out.push(ToolSection::input("input", SectionBody::Json(pretty)));
        }
        return out;
    };
    let mut chips: Vec<(String, String)> = Vec::new();
    let mut leftover = serde_json::Map::new();
    for (k, val) in map {
        match val {
            Value::String(s) => {
                if s.len() >= 80 || s.contains('\n') {
                    // Long / multiline string → its own code tile (readable).
                    out.push(ToolSection::input(
                        "field",
                        SectionBody::Code { text: format!("{k}:\n{s}"), max_lines: None },
                    ));
                } else if !s.trim().is_empty() {
                    chips.push((k.clone(), chip_value(s)));
                }
            }
            Value::Number(n) => chips.push((k.clone(), n.to_string())),
            Value::Bool(b) => chips.push((k.clone(), b.to_string())),
            other => {
                leftover.insert(k.clone(), other.clone());
            }
        }
    }
    if !chips.is_empty() {
        out.insert(0, ToolSection::input("input", SectionBody::Chips(chips)));
    }
    if !leftover.is_empty() {
        let pretty = serde_json::to_string_pretty(&Value::Object(leftover))
            .unwrap_or_default();
        out.push(ToolSection::input("input", SectionBody::Json(pretty)));
    }
    out
}

// ═══ RENDER LAYER ═══════════════════════════════════════════════════════════

/// The style context a tool body renders in — theme, fonts, zoom. Built at the
/// call site (subagent view / transcript closure) from the snapshotted chrome.
pub(crate) struct ToolBodyCtx<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) body_font: SharedString,
    pub(crate) code_font: SharedString,
    pub(crate) text_scale: f32,
    /// Cap markdown blocks per section (inline transcript = Some, focused
    /// subagent view = None to show the whole report).
    pub(crate) markdown_block_cap: Option<usize>,
}

/// Clamp a text payload to `TOOL_TEXT_MAX_BYTES`, appending a note when clipped.
fn cap_text(s: &str) -> std::borrow::Cow<'_, str> {
    if s.len() <= TOOL_TEXT_MAX_BYTES {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut end = TOOL_TEXT_MAX_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!(
        "{}\n\n… truncated ({} KB total)",
        &s[..end],
        s.len() / 1024
    ))
}

/// Render one planned section as a titled tile (UXI-AgentTile-25).
pub(crate) fn render_tool_section(ctx: &ToolBodyCtx, s: &ToolSection) -> gpui::AnyElement {
    let at = &ctx.theme.agent;
    let scale = ctx.text_scale;
    let dim = nc(at.dim);
    let (border, bg) = match s.role {
        SectionRole::Input => (nc(at.tool_card_border), nc(at.tool_body_bg)),
        SectionRole::Output => (nc(at.agent_tint), nc(at.tool_output_bg)),
    };
    let border = if s.emphasis { nc(at.warm_accent) } else { border };

    let label = div()
        .text_size(px(10.0 * scale))
        .text_color(dim)
        .font_family(ctx.body_font.clone())
        .font_weight(FontWeight::MEDIUM)
        .pb(px(3.0 * scale))
        .child(SharedString::from(s.label.to_uppercase()));

    let body_el: gpui::AnyElement = match &s.body {
        SectionBody::Chips(pairs) => render_chips(ctx, pairs).into_any_element(),
        SectionBody::Prose(text) => div()
            .text_size(px(13.0 * scale))
            .text_color(nc(at.frozen_fg))
            .font_family(ctx.body_font.clone())
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        SectionBody::Code { text, max_lines } => {
            render_mono(ctx, &cap_text(text), *max_lines, false).into_any_element()
        }
        SectionBody::Diff { header, text } => {
            let mut col = div().flex().flex_col();
            if let Some(h) = header {
                col = col.child(
                    div()
                        .text_size(px(11.0 * scale))
                        .text_color(dim)
                        .font_family(ctx.code_font.clone())
                        .pb(px(2.0 * scale))
                        .child(SharedString::from(h.clone())),
                );
            }
            col.child(render_mono(ctx, &cap_text(text), None, true)).into_any_element()
        }
        SectionBody::Markdown { text } => {
            let capped = cap_text(text);
            let blocks = render_with_wiki(&capped, ctx.theme, None);
            render_markdown_column(
                &blocks,
                ctx.markdown_block_cap,
                ctx.theme,
                &ctx.body_font,
                &ctx.code_font,
                scale,
            )
        }
        SectionBody::Json(text) => render_mono(ctx, &cap_text(text), None, false).into_any_element(),
    };

    div()
        .flex()
        .flex_col()
        .mt_1()
        .mx_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_l_2()
        .border_color(border)
        .bg(bg)
        .child(label)
        .child(body_el)
        .into_any_element()
}

/// Render `label → value` chips, wrapped, values in the code font.
fn render_chips(ctx: &ToolBodyCtx, pairs: &[(String, String)]) -> gpui::Div {
    let at = &ctx.theme.agent;
    let scale = ctx.text_scale;
    let mut row = div().flex().flex_row().flex_wrap().gap_2().items_center();
    for (k, v) in pairs {
        row = row.child(
            div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap_1()
                .px_1p5()
                .py(px(1.0 * scale))
                .rounded_sm()
                .bg(nc(at.tool_body_bg))
                .child(
                    div()
                        .text_size(px(10.0 * scale))
                        .text_color(nc(at.dim))
                        .font_family(ctx.body_font.clone())
                        .child(SharedString::from(k.clone())),
                )
                .child(
                    div()
                        .text_size(px(11.0 * scale))
                        .text_color(nc(at.tool_body_fg))
                        .font_family(ctx.code_font.clone())
                        .child(SharedString::from(v.clone())),
                ),
        );
    }
    row
}

/// Render monospace lines with optional truncation and optional diff coloring.
fn render_mono(ctx: &ToolBodyCtx, text: &str, max_lines: Option<usize>, diff: bool) -> gpui::Div {
    let at = &ctx.theme.agent;
    let scale = ctx.text_scale;
    let fg = nc(at.tool_body_fg);
    let display = match max_lines {
        Some(n) => truncate_lines(text, n),
        None => text.to_string(),
    };
    let mut col = div()
        .flex()
        .flex_col()
        .text_size(px(11.0 * scale))
        .text_color(fg)
        .font_family(ctx.code_font.clone());
    let (add, remove, header) = (nc(at.diff_add), nc(at.diff_remove), nc(at.diff_header));
    for line in display.lines() {
        let color = if diff {
            if line.starts_with("+ ") || line.starts_with("+\t") || line == "+" || line == "+ (new)" {
                add
            } else if line.starts_with("- ") || line.starts_with("-\t") || line == "-" || line == "- (old)" {
                remove
            } else if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ") {
                header
            } else {
                fg
            }
        } else {
            fg
        };
        col = col.child(div().text_color(color).child(SharedString::from(line.to_string())));
    }
    col
}

/// Drop-in rich replacement for the old `append_tool_body`: plan the sections,
/// render each as a tile, append to `block`. Both call sites use this.
pub(crate) fn append_tool_body_rich(
    mut block: gpui::Div,
    tc: &yalda::acp_channel::ToolCall,
    policy: ToolRenderPolicy,
    ctx: &ToolBodyCtx,
) -> gpui::Div {
    for s in plan_tool_sections(tc, policy) {
        block = block.child(render_tool_section(ctx, &s));
    }
    block
}

/// Convenience: the inline-transcript variant caps markdown blocks.
pub(crate) fn tool_body_ctx_inline(
    theme: &Theme,
    body_font: SharedString,
    code_font: SharedString,
    text_scale: f32,
) -> ToolBodyCtx<'_> {
    ToolBodyCtx {
        theme,
        body_font,
        code_font,
        text_scale,
        markdown_block_cap: Some(TOOL_MARKDOWN_MAX_BLOCKS_INLINE),
    }
}
