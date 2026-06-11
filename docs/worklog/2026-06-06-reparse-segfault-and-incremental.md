# Worklog — 2026-06-06 — Edit-view reparse: segfault fix + incremental (10–20×)

Found and fixed while testing 5c (the crash surfaced during a two-pane edit),
but both are **independent of 5c** and landed on `master`.

## 1. Crash fix — `d32edf9`
`EditorCore::reparse` fed tree-sitter the previous tree for *incremental* reuse
(`parser.parse(source, self.tree.as_ref())`) but **nothing ever called
`tree.edit()`** — so every reparse after the first handed tree-sitter stale byte
offsets. On some markdown edits (tables / fenced code / blockquotes) the markdown
external scanner's `serialize` then read/wrote out of bounds → a
heap-nondeterministic **SIGSEGV** while typing (observed:
`tree_sitter_markdown_external_scanner_serialize → memmove`). Fix: parse fresh
(`None`) — correct but O(doc) per keystroke. Removed the unused, wrong (`Point(0,0)`)
`TreeState::edit` helper.

## 2. Incremental reparse — `413da19`
The fresh-parse fix made every keystroke a full parse. **Measured (debug):**
686µs / 50 lines, 2.7ms / 200, 13ms / 1000, 67ms / 5000 — O(doc), the felt
latency.

Restored incremental parsing **safely**, confining the crash hazard (a wrong
`InputEdit`) to one gated spot:
- `Document::record_splice` runs before every rope mutation (old rope) →
  `note_pending_edit` computes the exact `InputEdit` (byte ranges + byte-column
  `Point`s; `advance_point` handles multi-line). `take_pending_edit` hands it to
  reparse **only when exactly one clean splice** happened since the last reparse;
  zero/multiple → `None` → full parse.
- `TreeState::parse(source, edit)`: `Some` → `tree.edit()` + incremental reuse;
  `None` → drop tree + full parse. Never feeds a stale tree.
- `reparse()` consumes `take_pending_edit()` — **no changes to the 6 reparse
  call sites or the begin/end-insert cadence**.

**Safety guard (the reason this was landable without runtime):** a fuzz test
drives the editor through ~27k random edits (single + multi-char/multi-line
inserts, backspace, delete; markdown structures, newlines, multibyte) and after
*every* edit asserts the incremental tree's full **s-expression** equals a fresh
full parse's. Zero divergence, zero crash. Adversarially verified
(`is_correct: true`, high confidence) — char_point@EOF, advance_point
newline/CRLF/multibyte, delete-vs-replace, the single-splice gate all sound.

**Result: 10–20× faster/keystroke** (143µs vs 1443µs @200 lines; 361µs vs 7ms
@1000). `#[ignore]`d speed + parse-cost measurements kept; `YALDA_PARSE_TIMING=1`
logs per-reparse kind+time.

## Aside to follow up
The verifier noted the tree-sitter tree may be "used only in tests, not GPUI
rendering." If true, reparse-per-keystroke is partly wasted work and could be
made lazy/skipped for an even bigger win — worth a quick check (no runtime
needed). The incremental fix already cut the cost 10–20× regardless.

## Also this session (separate worklog `2026-06-06-phase-b-batch.md`)
R, 9′ landed; 8b stage-1 (additive `TurnEnded`); reconnect-storm instrumentation
(`6eb9660` — every disconnect now names its cause); 5c step-1 + all Doc-open
paths pool-bound (on `doc-edit-5c`, awaiting runtime verification of live
tracking).
