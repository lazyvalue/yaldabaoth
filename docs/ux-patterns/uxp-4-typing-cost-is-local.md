# UXP-4 — Input on one surface never re-renders unrelated surfaces (cost is O(changed))

**Statement.** A keystroke, stream event, or tick re-renders only the surfaces whose
inputs it changed. Expensive or usually-stable surfaces are their own cached view
entities, so typing latency stays proportional to what changed, not to the whole
window.

**Applies to.** Every surface that is expensive to render or stable while the user
works elsewhere; built via `yux/cached.rs` (`cached_child`, `record_render`).

**Why.** GPUI re-renders the root every frame; `AnyView::cached` is the only
render-skip lever. Without it, cost is O(whole tree) per keystroke
(`docs/reference/gpui-render-model.md`).

**Status.** `implemented` for the transcript, Linear, and Diff bodies, and (D11,
2026-09-27) the worksheet's inline You-block — typing into it re-renders only its
own `YouBlockView` overlay, never the cached transcript.

**Enforcement.** Every cached surface ships a render-count test, e.g.
`verify_harness.rs::transcript_021_chatbox_keystroke_is_render_flat`,
`verify_harness.rs::worksheet_inline_typing_rerenders_you_block_not_transcript`. Wall-clock
timing is harness gap #3 (human `sample` under `--release`).

**Realized by.** The cached surfaces listed in `src/bin/yalda-gpui/yux/CLAUDE.md`.
