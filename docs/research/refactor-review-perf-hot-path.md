> **Editor's note (added when landing):** This `/refactor` review ran against base commit `f282130`, *before* the `perf` synthesis branch (`9a41bfc`) landed. Several findings are therefore **already fixed on `perf`** and should be skipped: **#3** (highlight-cache fence re-highlight → fixed via `advance_fence`), **#5** (`shift_for_insert` rebuild → fixed, now O(shifted)), **#8** (`apply_server_batch` per-event → fixed, contiguous same-session events coalesced), **#10** (`full_text()` tail checks → fixed via O(1) rope probes). The **net-new** findings worth acting on are **#1/#2** (render_agent view-model rebuild + deep clones — partially addressed on `perf`, fully addressed by the tachyon `perf-tachyon` branch's S1), **#4** (undo full-rope snapshot), **#6/#7** (server `event_log` unbounded + global lock), and **#9** (insertion-point scan). Lenses ran with the Fulcrum philosophy preamble (Python/PyO3/EARS) which does not apply to this Rust app — treat "enforcement hook" as a Rust type/test/`debug_assert!`.

# Refactor review — sketch-gpui agent/transcript performance hot path

_Scope: 10 file(s). Lenses: algorithms, concurrency_resources, liveness_termination, effects_purity._

## Summary
The architectural through-line is a broken "O(changed)" contract: a perf pass installed an `edit_seq`-gated `HighlightCache` fast-skip, but every other per-frame derivation in `render_agent` — line extraction, gutter tags, flat-item build, blank-collapse, and the deep clones of `lines`/`gutter_tags`/`tool_calls` into the `'static` render closure — still runs unconditionally, so a cursor blink or 8x/sec thinking-animation tick on a multi-thousand-line transcript pays full O(transcript) allocation. The single highest-leverage move is to extend the existing `edit_seq` fingerprint to memoize the whole derived view-model behind one `Rc` snapshot (collapsing four findings into one) and to make the closure-captured fields `Rc<…>` so deep clones become unrepresentable at the type level. Behind that, three independent quadratics survive on the streaming path that the cache was meant to kill — the highlight-cache fence advance still calls the full highlighter for every line (`highlight_cache.rs:224`), `shift_for_insert` rebuilds both anchor BTreeMaps per chunk, and undo snapshots the entire rope per insert-group — each O(n)-per-chunk and thus O(n²)-per-turn. On the server side, the per-session `event_log` grows unbounded and is deep-cloned under a global lock on attach/persist. The strongest, lowest-risk wins are type-level: `Rc`-wrapping the closure captures and the server attach snapshot make the bad allocation literally uncompilable.

## Findings (ranked)

### 1. render_agent re-derives the entire transcript view-model every frame; only highlighting is gated on edit_seq  ·  algorithms + concurrency_resources + liveness_termination + effects_purity  ·  effort L  ·  confidence high
- **Location:** src/bin/sketch-gpui/main.rs:11194-11418 (edit_seq read at 11194 but consumed only by highlight snapshot at 11213); lines extract 11195-11203; gutter_tag_per_line 11241-11247; block_at_start scan 11296-11307; flat_items build 11313-11365; blank-collapse pass 11372-11418; animation notify 8976-8983
- **Evidence:** `edit_seq` is read at 11194 but only `highlight_cache.snapshot_syn` (11213) uses it. Everything else runs unconditionally per call: `lines: Vec<String>` built per line, `gutter_tag_per_line` resolving an `anchor_for_line_opt` BTreeMap lookup plus a `metadata::<TurnId>()` lookup per line, the block-range scan, the flat_items construction, and the O(n) blank-collapse. Cursor blink, cross-pane `cx.notify()`, and the 120ms awaiting animation tick each trigger a full render with zero document change. During a streaming turn the transcript grows while one render fires per pump cycle, so the summed cost is O(n²) over the turn.
- **Refactor move:** Lift the flat_items / blank-collapse / block-range / gutter derivation into a pure function and memoize its output (`Rc<Vec<String>>`, gutter tags, `flat_items: Rc<Vec<FlatItem>>`, block_at_start map) on `AgentState` behind a fingerprint of `(edit_seq, frozen_line_count, tool_call_order.len(), awaiting_reply, theme_fp)`, mirroring `HighlightCache`'s fast-skip. **(Implemented on `perf-tachyon` as S1.)**
- **Enforcement hook:** A fingerprint newtype on `AgentState` plus a `#[test]` asserting a second render with unchanged fingerprint returns the same `Rc` and recomputes 0 lines.

### 2. Closure-captured snapshots (lines, gutter_tags, tool_calls) are deep-cloned every frame instead of shared by Rc  ·  concurrency_resources + liveness_termination + effects_purity  ·  effort M  ·  confidence high
- **Location:** src/bin/sketch-gpui/main.rs:11459 (`lines_snap = lines.clone()`), 11463 (`gutter_tag_snap`), 11464 (`tool_calls_snap = c.tool_calls.clone()`), 11465 (`expanded_snap`); contrast hl_snap Rc clone at 11460-11462
- **Evidence:** `lines_snap`/`gutter_tag_snap` deep-copy freshly-built Vecs; `tool_calls_snap` deep-copies `HashMap<String, ToolCall>` where each carries content/diff/raw_input/raw_output strings capped at 64K chars each, never pruned. All runs on every notify including idle frames. `hl_snap` already does the right thing (Rc pointer clone).
- **Refactor move:** Make the cached fields `Rc<…>` and hand the closure Rc clones; store `tool_calls` as `Rc<HashMap<…>>`, mutate via `Rc::make_mut`. `TurnId` is `Copy`. **(Implemented on `perf-tachyon` as part of S1.)**
- **Enforcement hook:** Type-level — `Rc<…>` makes a deep `.clone()` a refcount bump by construction.

### 3. HighlightCache re-runs the full highlighter on every line each snapshot solely to advance fence state  ·  algorithms + liveness_termination  ·  effort S–M  ·  confidence high  ·  ⚠ ALREADY FIXED on `perf`
- **Location:** src/bin/sketch-gpui/highlight_cache.rs:211-226 (line 224)
- **Note:** The `perf` branch landed exactly the recommended `advance_fence()` byte-scan. Skip.

### 4. Undo snapshots the entire rope to a String on every insert-group and every undo/redo  ·  algorithms  ·  effort M  ·  confidence high  ·  NET-NEW
- **Location:** src/document.rs:223 (begin_undo_group), 254 & 263 (undo), 282 & 291 (redo); call sites src/editor.rs:838, 794/907/935/958/975
- **Evidence:** `begin_undo_group` does `before_text: self.rope.to_string()` (full O(n)); `undo`/`redo` each `to_string()` + `Rope::from_str`. `begin_insert` calls it once per entry into insert mode, and the worksheet/chatbox compose lives in the same `Document` as the multi-thousand-line frozen transcript — so typing one character snapshots the whole transcript, and `undo_stack` retains a full copy per group (unbounded growth ∝ edits × transcript size).
- **Refactor move:** Replace whole-text snapshots with a delta record (`(char_range, prev_slice)`); ropey supports cheap range reads.
- **Enforcement hook:** Change `before_text: String` to a delta enum so the whole-snapshot state is unrepresentable; `#[test]` asserting the pushed entry is O(edit), not O(N).

### 5. shift_for_insert rebuilds both anchor BTreeMaps over all anchors on every newline-bearing insert  ·  liveness_termination  ·  effort M  ·  confidence med  ·  ⚠ ALREADY FIXED on `perf`
- **Note:** The `perf` branch took render's in-place suffix re-key (true O(shifted)). Skip.

### 6. Server per-session event_log grows unbounded and is deep-cloned under the global lock on attach/persist  ·  algorithms + concurrency_resources + liveness_termination + effects_purity  ·  effort L  ·  confidence med  ·  NET-NEW
- **Location:** src/bin/sketch-session-server/main.rs:51-53 (event_log), pushes throughout, save_to_disk clone at 143, attach clone at 440 (under lock); replay tail protocol at 1016-1079
- **Evidence:** `event_log` is only ever pushed, never trimmed; `save_to_disk` and `attach` clone the whole log under the global `sessions` lock. The self-host loop reconnects on every candidate relaunch, paying the full-log clone+replay repeatedly and unbounded with session age.
- **Refactor move:** (a) low-risk: store as `Arc<[Notification]>` so attach/save clone a pointer; remove `event_log` from `list_sessions` payload. (b) higher-risk: cap/compact resolved turns, preserving the resumable-tail `sent`-index protocol (1016-1079).
- **Enforcement hook:** `Arc<[Notification]>` makes the under-lock deep clone unrepresentable; a `MAX_EVENT_LOG_LEN` + equivalence test for compaction.

### 7. Pump and forwarders serialize every session and subscriber through one global sessions Mutex, per streamed event  ·  concurrency_resources  ·  effort L  ·  confidence med  ·  NET-NEW
- **Location:** src/bin/sketch-session-server/main.rs:653 + 737-773 (pump holds global lock through drain+log+broadcast), 1019-1044 (forward loop re-locks per wake), 748 (broadcast per ReplyEvent)
- **Refactor move:** Shard the lock per session (`HashMap<Id, Arc<Mutex<ManagedSession>>>` or `DashMap`); better, have the forwarder consume the broadcast payload directly for logged events and use `event_log` only for cold attach/replay.
- **Enforcement hook:** Integration test driving two sessions, asserting forward progress on one while the other is saturated.

### 8. apply_server_batch per-event scan/clone/scroll  ·  algorithms + liveness_termination + effects_purity  ·  effort M  ·  confidence med  ·  ⚠ ALREADY FIXED on `perf`
- **Note:** The `perf` branch coalesces contiguous same-session ReplyEvents into one slot lookup + one apply + one follow-scroll. Skip (the residual O(N) event application itself remains, but the per-event clone/scroll/walk is gone).

### 9. find_llm_insertion_point scans every allocated anchor in reverse on each streamed chunk  ·  effects_purity  ·  effort M  ·  confidence med  ·  NET-NEW
- **Location:** src/editor.rs:281-294 (last_line_with_meta), called from find_llm_insertion_point (1237) on every append_llm_chunk (1210)
- **Evidence:** `last_line_with_meta` iterates `by_line.iter().rev()` with a metadata downcast per entry. Common "continue current turn" case returns fast near the tail, but worst case is O(transcript) per chunk.
- **Refactor move:** Cache the last insertion line/anchor for the in-flight turn tag on `EditorCore`, updated in `append_llm_chunk`, invalidated on shifts — O(1) for the common case.
- **Enforcement hook:** `#[test]` appending K chunks to an N-line transcript asserting work independent of N (anchor-iteration counter). Subtlety: cache invalidation across shift_for_delete/insert.

### 10. UserPrompt and send paths materialize the full rope multiple times per call for tail/suffix tests  ·  algorithms + liveness_termination + effects_purity  ·  effort S  ·  confidence med  ·  ⚠ MOSTLY FIXED on `perf`
- **Note:** The `perf` branch added O(1) rope probes (`document_trimmed_end_ends_with`, `last_char`, `is_empty`) on the agent streaming/prompt paths. A few `full_text()` calls outside that hot path (doc-view render, finalize_agent_turn last-char) remain but are not on the streaming path.

## Taste-leaning / low-confidence
- detect_block_ranges and the blank-collapse pass test frozen membership with a linear `frozen_ranges.iter().any()` per line (main.rs:2995-2997, 3031, 11376-11378, 11491-11493). `frozen_ranges` is coalesced (typically 1-2 entries), so this rarely degrades; a `partition_point` is exact but micro-optimization.
