# Common — Blockquote

Shared component. Owns `UXI-Blockquote-1`.

## Description

**Blockquote** is the markdown construct for quoted text: a line whose first
non-whitespace character is `>` (repeated for nesting — `>>`). It is the standard
"I am quoting something" element, and yalda renders it on many surfaces at once:
the rendered doc view, the agent transcript (both as a parsed markdown block and
as a source-highlighted line), the two edit views, and — since
[UXI-AgentTile-21](../agent-tile/compose.md) — the **compose / You-block**, where
`[N]r` seeds a reply with `> <quoted sentences>`.

Because the same construct is produced by several independent render paths, its
styling has to be stated once, centrally, or the surfaces drift apart (which is
exactly what happened: three surfaces italicised quoted text and three did not).

## References

- `docs/components/agent-tile/compose.md` — `UXI-AgentTile-21` (`[N]r`
  reply-with-quotation) is the main producer of blockquotes in the compose.
- `docs/components/common/text-editing.md` — the compose is an editable surface;
  blockquote styling must not disturb `UXI-TextEditing-1` caret math.

## UX invariants

### UXI-Blockquote-1 — Blockquoted text renders italic on every surface

**Statement.** Text in a markdown blockquote — a line whose first non-whitespace
character is `>` — renders in **italic**, on **every** surface that displays it.
No surface shows quoted text upright. Specifically, all six render paths agree:

| # | Surface | Path | Before |
|---|---------|------|--------|
| 1 | Rendered doc view (parsed block) | `render_blocks.rs::block_inner` `RenderedBlock::BlockQuote` | already italic |
| 2 | Agent transcript, parsed markdown block | same `block_inner` | already italic |
| 3 | WP edit view | `screens.rs` `WpLineKind::Blockquote` | already italic |
| 4 | Agent transcript, source-highlighted line | `md_highlight::highlight_source_line` | **was upright** |
| 5 | RAW / Code edit view | same `md_highlight` path | **was upright** |
| 6 | Compose, virtualized compose, inline You-block | `agent.rs::build_chatbox_wrapped_line` | **was upright** |

Two supporting rules:

1. **Italic covers the WHOLE quote, including nested inline spans.** On the
   `md_highlight` path the italic is applied to *every segment the quote line
   produced*, not just the blockquote base style — because `tokenize_inline`
   gives nested `**bold**` / `` `code` `` / link spans their own style, which
   would otherwise overwrite the blockquote styling and leave holes of upright
   text inside an italic quote. Nested quotes (`>>`) are italic too.
2. **Only a LINE-LEADING marker counts.** `a > b`, `if x >= 3`, and a `>` inside
   prose are not quotes and stay upright. Leading whitespace before the marker is
   allowed (`   > indented`). Every surface uses the same rule
   (`md_highlight::split_quote_prefix` / `agent.rs::is_blockquote_line`).

**Applies to.** `src/md_highlight.rs` — the blockquote branch of
`highlight_source_line` (adds `Modifier::ITALIC` to every segment of the quote);
`src/bin/yalda-gpui/agent.rs` — `is_blockquote_line` + the `.italic()` on the
line container in `build_chatbox_wrapped_line` (which all three compose/You-block
call sites route through). The already-correct surfaces (`block_inner`,
`WpLineKind::Blockquote`) are untouched. `Modifier::ITALIC` is honoured by the
shared `Style → gpui::Font` converter `render_blocks.rs::font_for`.

**Why.** Quoted text needs to read as *someone else's words* at a glance. Half
the surfaces already italicised it, so the ones that didn't looked like a bug —
most visibly with `UXI-AgentTile-21`: `r` seeds `> …` into the You-block, which
rendered upright while composing and then flipped to italic once submitted and
re-rendered as a parsed block. Stating it once here stops the six paths drifting.

**Status.** `implemented` — surfaces 4/5 via `md_highlight`, surface 6 via
`build_chatbox_wrapped_line`; 1/2/3 already satisfied it.

**Enforcement.** Data-level (the italic *paint* is a human-eye check, harness
gap #1 — the layout probe returns rects, not glyphs):
`md_highlight.rs::blockquote_italic_tests::blockquote_segments_are_italic` — every
segment of `> quoted **bold** text` carries `Modifier::ITALIC` (nested `>>` too),
and a plain line carries none. Negative control observed: dropping the
`add_modifier` fails with `segment "> " on a blockquote line must be ITALIC`.
Classification for surface 6 is pinned by
`tests.rs::is_blockquote_line_matches_leading_marker_only` (leading marker yes,
`a > b` no). **Runtime check pending:** that quoted text visibly renders italic in
the live compose / You-block and RAW view (gap 1).
