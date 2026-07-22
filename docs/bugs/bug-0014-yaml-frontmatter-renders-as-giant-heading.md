# bug-0014: yaml-frontmatter-renders-as-giant-heading

**Status:** FIXED
**First seen:** 2026-07-21 (evidence: `~/ws/fulcrum/tmp/Screenshot 2026-07-21 at 11.58.21 AM.png`)
**Component:** `docs/components/buffer.md` (doc view) — shared `src/render.rs`

## Symptom

Opening any markdown file with YAML frontmatter (every `.claude/agents/*.md` and
`.claude/skills/*/SKILL.md`) renders the frontmatter as one **enormous bold heading**
spanning several lines, with its internal line breaks collapsed:

> **name: docs description: Editorial documentation agent for autonomous or dispatched
> runs — … tools: Read, Edit, Write, Bash, Grep, Glob**

Expected: frontmatter is metadata, not the document's title. It should read as a
de-emphasized metadata block (or not at all), and the document's real first heading
should be the visually dominant one.

## Context / root cause

Nothing in the pipeline knows what frontmatter is — `rg -i 'frontmatter|yaml'` over
`src/render.rs`, `src/blocks.rs`, `src/md_highlight.rs` returns zero hits — and
`src/parse.rs` builds its `Options` with only `ENABLE_TABLES`, `ENABLE_STRIKETHROUGH`,
`ENABLE_TASKLISTS`. So CommonMark rules apply to the raw text:

```
---              ← thematic break (HorizontalRule)
name: docs
description: …   ← a paragraph; single newlines are soft breaks, hence the run-on
---              ← a setext underline, which PROMOTES that paragraph to a level-2 heading
```

The closing `---` is what makes it enormous: it converts the metadata paragraph into
an `<h2>`. This is standard CommonMark behavior, not a yalda parsing bug — the fix is
to opt into the parser's metadata-block extension.

## Planned solution

pulldown-cmark 0.12.2 (already the pinned version) ships
`Options::ENABLE_YAML_STYLE_METADATA_BLOCKS` (`lib.rs:559`), which recognizes a
leading `---` … `---` block and emits it as
`Tag::MetadataBlock(MetadataBlockKind::YamlStyle)` instead of
thematic-break + setext heading.

1. `parse.rs` — enable that option (and the `+++`-delimited sibling, so TOML
   frontmatter behaves too).
2. `blocks.rs` — new `RenderedBlock::Metadata { lines: Vec<StyledLine> }` so the block
   is structurally distinct rather than a paragraph that happens to be styled dim.
3. `render.rs` — collect the metadata block's text into that variant, one
   `StyledLine` per source line (the line breaks the setext-paragraph collapse ate).
4. `render_blocks.rs` — render it de-emphasized (dim, small, no heading scale), and
   add it to the two non-exhaustive helper matches (`RenderedBlock::HorizontalRule |
   Image` arms at ~1878 / ~1901).

Verification seam: `render()` is a pure function over a markdown string, so the guard
is a plain lib test — parse a frontmatter document and assert the first block is
`Metadata`, NOT `Heading { level: 2 }`.

## Approaches already tried (do NOT repeat)

- <none — first attempt>

---

## Log

### 2026-07-21 — opted into the parser's metadata-block extension

**Changed**, as planned, no deviations:

- `src/parse.rs` — `ENABLE_YAML_STYLE_METADATA_BLOCKS` + `ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS`.
- `src/blocks.rs` — new `RenderedBlock::Metadata { lines: Vec<StyledLine> }`.
- `src/render.rs` — `Event::Start(Tag::MetadataBlock(_))` arm collects the raw block
  text and splits it back into one `StyledLine` per source line (blank lines dropped);
  an empty block pushes nothing.
- `src/bin/yalda-gpui/render_blocks.rs` — `block_inner` renders it dimmed (opacity .7),
  monospace, 12px × `text_scale`, behind a left rule; `expand_wiki_links_in_block` and
  `block_contains_link` treat it as inert (metadata is not prose, carries no links).

**Verified.** `src/render.rs::frontmatter_tests::yaml_frontmatter_is_metadata_not_a_heading`
— asserts the first block is `Metadata`, that NO block is a `Heading{level:2}` carrying
the frontmatter text, that the document's real `# Real Title` H1 still renders, and
that the metadata keeps its 3 separate lines (the run-on collapse is what the
screenshot showed). Suites: 157 lib + 399 bin green; `cargo check --bin yalda-gpui`
clean.

**Negative control observed RED.** Commented out the YAML option in `parse::parse` →
`frontmatter renders as its own metadata block; got Some(HorizontalRule)` — i.e. the
exact CommonMark misparse (thematic break + setext promotion) the bug describes.
Restored, re-ran green.

**Unverified / caveats.** (1) The de-emphasized *pixels* are harness gap #1 — the test
proves the block type and line structure, not how it looks; needs a human look at a
`.claude/agents/*.md` in the doc view after a rebuild. (2) Only a LEADING metadata
block is recognized (parser behavior); a `---` fence mid-document still parses as a
thematic break, which is correct. (3) The RAW/edit view and `md_highlight` are
untouched — frontmatter there still highlights as plain source, which is fine, but the
WP view's `classify_wp_line` has not been checked for a similar setext promotion.
