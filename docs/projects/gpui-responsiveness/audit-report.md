# Yalda-gpui Responsiveness Roadmap

## 1. Executive summary

Every confirmed finding is a symptom of one root cause: **`YaldaGpuiView` is the only GPUI entity, so any `cx.notify()` dirties the root and re-runs the entire render + layout tree — there is no subtree skip.** The dominant pattern is "typing/interacting in surface A re-lays-out unrelated, unchanged subtree B" (chatbox→transcript, overlay→transcript, worksheet→compose+strip, drag→all desktop tiles). Prior virtualization + S1 memoization already capped most of these at O(visible) rather than O(all-data), so the remaining costs are bounded — but they still fire on the most-touched surface in the app (typing in the Message Box). The single highest-leverage move is to build **one reusable "cached panel child-entity with fingerprint-gated notify" helper** and apply it first to the transcript: that is GPUI's only render-skip mechanism, it kills the flagship chatbox-keystroke item, and it transitively fixes the overlay, worksheet-neighbor, and desktop-drag cases for free. A second, independent theme — **blocking subprocess/syscall I/O on the paint thread** (`pbpaste`/`pbcopy`/recursive `fs` walk) — is unrelated to the entity model and is cheap to fix in parallel.

## 2. Ranked findings (by leverage = frequency × cost × perceptibility)

| Rank | Finding | Surface | Trigger | Cost | Treatment | Impact | Effort |
|---|---|---|---|---|---|---|---|
| 1 | `chatbox-keystroke-relayouts-transcript` / `…notifies-root-reruns-render-agent` (same item) | Message Box compose, transcript above | chatbox-keystroke | O(visible-transcript-rows) layout + strip/snapshots, every keystroke | cached-child-entity | high | L |
| 2 | `browser-filter-keystroke-recursive-fs-walk` (= `…relayouts-siblings`) | Cmd+O / rail filter field | filter-keystroke | Blocking recursive `fs::read_dir`+`metadata` to depth 8 on paint thread, **per keystroke**, no debounce | move-io-off-thread + debounce | medium | M |
| 3 | `expanded-toolgroup-body-not-virtualized` | Transcript, expanded Full/diff ToolGroup | chatbox-keystroke | O(expanded-group-body-lines), up to ~thousands of divs, re-laid-out per keystroke when visible | virtualize-list / cap body | medium | M |
| 4 | `clipboard-pbpaste-blocks-paste-keystroke` | Edit + agent paste | paste gesture | Blocking `pbpaste.output()` fork+exec+wait, ~tens of ms, once per paste | move-io-off-thread | medium | S |
| 5 | `clipboard-pbcopy-blocks-yank-and-delete-keys` | Edit + agent Normal-mode x/dd/yank | yank/delete key | Blocking `pbcopy` fork+exec + sync stdin write per key | move-io-off-thread | medium | S |
| 6 | `compose-longdraft-lines-rebuild-no-seq-guard` | Message Box, >8-line draft | chatbox-keystroke | O(draft) String allocs/keystroke, no `lines_cache` | memoize-derivation | medium | S |
| 7 | `worksheet-keystroke-reruns-compose-and-strip` / `…relayouts-neighbors` | Worksheet, sibling compose+strip | worksheet-keystroke | O(strip)+O(≤8 compose lines) wasted rebuild per keystroke | cached-child-entity (shared w/ #1) | medium→low | (covered by #1/#10) |
| 8 | `overlay-textfield-relayouts-underlying-agent` | Rename/tag/buffer-switcher overlay over agent | overlay-keystroke | O(visible-transcript-rows) wasted (occluded, unchanged) | cached-child-entity (shared w/ #1) | low | (covered by #1) |
| 9 | `desktop-drag-resize-notify-per-pixel` | Desktop tile drag/edge-resize | mouse-move | O(visible desktop tiles) re-layout per pixel; resize notifies mostly redundant | coalesce-input (resize only) | medium→low | S |
| 10 | `wp-edit-line-classification` / `wp-line-classify-full-scan` (same item) | WordProcessor edit view | edit-keystroke | O(document) `classify_wp_line` fold + Vec alloc/render, no seq guard | memoize-derivation | low | S |
| 11 | `agent-picker-rows-unwindowed` | Agent selector (unbound tile) | picker j/k | O(sessions) rows, no window | virtualize-list | low | S |
| 12 | `render-agent-frozen-lines-double-tovec` / `frozen-lines-tovec-per-render` (same item) | render_agent snapshot | chatbox-keystroke | 2× O(frozen-ranges) tiny Vec alloc/frame; 2nd is pure dup | cheap-snapshot-guard | low | S |
| 13 | `render-agent-process-cwd-syscall-per-frame` | render_agent status strip | chatbox-keystroke | O(1) `getcwd(2)` syscall/frame | cheap-snapshot-guard (OnceLock) | low | S |
| 14 | `thinking-indicator-1hz-notify-dirties-root` | Thinking indicator | ~1Hz timer (8Hz legacy) | O(visible-rows)+strip/tick; legacy path 8× over-notifies | scoped-notify + pump parity | low | S |
| 15 | `save-workspace-state-current-dir-syscall` | Workspace persist | structural action only | Blocking serialize+`fs::write`/action (not per-keystroke) | move-io-off-thread (defensive) | low | M |

Note: several IDs are duplicate reports of the same code site (1≡the two chatbox items; 7's pair; 10's pair; 12's pair) — collapsed above.

## 3. Shared abstraction — the one helper to build

**Yes — findings 1, 7, 8, 9, 14 all want the identical mechanism**, and it is GPUI 0.2.2's *only* render-skip lever: a **cached child `Entity` whose `render()` is gated by a render-fingerprint**. Build this once.

The mechanism has two halves that must ship together:

1. **Embed the panel as a cached child:** `child_entity.into_any().cached(StyleRefinement::default().size_full())` (or `flex_1`). When the child's entity-id is *not* in `window.dirty_views` and its bounds/content-mask/text-style are unchanged, GPUI **skips its `render()` and reuses its prepaint/paint**. The cached slot is sized from the style, so the style *must* carry `size_full`/`flex_1` (the content no longer sizes it).

2. **Fingerprint-gated notify:** the child only enters `dirty_views` when *its own* inputs change. Don't blanket-`cx.notify()` the root; notify the child entity, and have the child early-out if its fingerprint is unchanged.

Proposed helper API (new module, e.g. `src/bin/yalda-gpui/cached_panel.rs`):

```rust
/// A child view whose laid-out element tree GPUI reuses until its
/// render-inputs change. The single render-skip primitive in this app.
pub(crate) trait FingerprintedPanel: Render {
    /// Cheap, allocation-free hash of everything render() reads.
    /// Equal fp across frames => GPUI may reuse the cached subtree.
    fn render_fp(&self) -> u64;
}

pub(crate) struct CachedPanel<V: FingerprintedPanel> {
    view: Entity<V>,
    last_fp: u64,
}

impl<V: FingerprintedPanel> CachedPanel<V> {
    /// Notify the child ONLY if its fingerprint moved. Returns true if it
    /// dirtied (i.e. its cached subtree will be rebuilt this frame).
    pub fn notify_if_changed(&mut self, cx: &mut App) -> bool {
        let fp = self.view.read(cx).render_fp();
        if fp != self.last_fp { self.last_fp = fp; self.view.update(cx, |_, c| c.notify()); true }
        else { false }
    }

    /// Render as a cached child sized from style (carries size_full/flex_1).
    pub fn element(&self, style: StyleRefinement) -> AnyElement {
        self.view.clone().into_any_element().cached(style)
    }
}
```

**What plugs in, and what each gets:**

- **Transcript panel (#1, the flagship):** fingerprint = `view_model_fp + transcript edit_seq + theme id + cursor-in-transcript flag + frozen-gen + scroll/follow intent`. A chatbox keystroke now mutates only the *compose* editor; the transcript entity-id stays out of `dirty_views`, its laid-out tree is reused, and the root re-render becomes cheap. This is the move that pays for the helper.
- **Compose/Message-Box panel (#1, #7):** make it a *separate* small cached child so its keystroke notify never reaches the transcript's dirty set. Symmetric: worksheet keystrokes then skip the compose subtree automatically.
- **Status/footer strip (#7, #14):** a tiny cached child keyed on `mode label + cursor L:C + awaiting + status msg + model/perm/cwd badge`. Stops the strip re-assembling on every transcript keystroke and on the 1Hz thinking tick.
- **Thinking indicator (#14):** its own cached child notified only on the anim fingerprint — clock ticks re-lay-out only the indicator, not the transcript/compose.
- **Desktop tiles / split leaves (#8, #9):** once *each leaf* is a cached child gated on its own fingerprint, overlay-keystrokes (#8) and per-pixel drag/resize (#9) stop re-laying-out unchanged sibling/occluded leaves. This is the largest blast radius — but also the riskiest refactor (every screen becomes an entity), so it comes last.

Findings that do **not** want this helper (independent fixes): #2/#4/#5/#15 are blocking-I/O (background-executor / in-process clipboard); #3/#11 are list-virtualization; #6/#10/#12/#13 are local seq-guard / OnceLock memoization. Keep those orthogonal.

## 4. Recommended execution order

**Phase 0 — cheap independent wins (parallel, no shared infra, S each):**
- `render-agent-process-cwd-syscall-per-frame` → OnceLock/static `process_cwd` (mirror `perf_enabled()` at `main.rs:130`). `screens.rs:1603`.
- `frozen-lines-tovec-per-render` → delete the duplicate `.to_vec()` at `screens.rs:990`, reuse `frozen_ranges` from `screens.rs:898`; gen-gate as Rc later.
- `compose-longdraft-lines-rebuild-no-seq-guard` → add `lines_cache`/`lines_cache_seq` to `Chatbox` (`agent.rs:1456`), gate at `screens.rs:1996-2005`; splice instead of `list_state.reset` at `:2006`.
- `wp-edit-line-classification` → cache `Vec<WpLineKind>` on `EditState` keyed by `edit_seq` (`screens.rs:585-593`).
- `thinking-indicator` part (1): gate the legacy direct-spawn pump notify on `awaiting_anim_fingerprint()` (`agent_ui.rs:1213-1219`) to match the 1Hz server path.

**Phase 1 — blocking I/O off the paint thread (parallel, S/M):**
- Replace `pbpaste`/`pbcopy` shell-outs with in-process `cx.read_from_clipboard()` / `cx.write_to_clipboard(ClipboardItem::new_string(..))` (`main.rs:5794`, `:5786`). Single change fixes #4 and #5. **S, do first** — highest perceptibility-per-effort of the I/O group.
- `browser-filter-keystroke-recursive-fs-walk` (#2): debounce (~80–120ms cancellable task keyed on filter text) + run `search_recursive` on `cx.background_executor()`, apply via notify; cheap incremental win when new query extends old (in-memory prefix filter). `file_browser.rs:195/313/524`, `browser_ui.rs:270/684`. **M.**

**Phase 2 — build the shared helper, apply to the flagship (L):**
1. Build `CachedPanel` + `FingerprintedPanel` (Section 3). This is the keystone infra; do it once, correctly (verify the `cached()` size-from-style requirement with a headless render test).
2. Extract the **transcript** into a child `Entity` implementing `FingerprintedPanel`; render via `CachedPanel::element(size_full())`; notify only on fingerprint change. **Verifies #1 — the flagship.** Runtime-check required (GPUI can't be driven headlessly for paint).
3. Extract the **compose/Message-Box** panel as a separate cached child. Confirms chatbox keystroke dirties only compose. Closes #1 and the wasteful half of #7.
4. Extract the **status/footer strip** + **thinking indicator** as small cached children. Closes #14 part (2) and the strip half of #7.

**Phase 3 — generalize to leaves (L, highest blast radius, do last):**
- Make each split/desktop leaf (`render_doc`/`render_edit`/`render_agent`/`render_browser`) a cached child gated on its own fingerprint. Transitively closes **#8** (overlay over occluded agent) and the sibling-relayout half of **#9** (also add the resize-only span-delta notify gate at `chrome.rs:555`; leave drag per-pixel — it legitimately follows the cursor).

**Phase 4 — opportunistic (M, schedule when a large session surfaces it):**
- `expanded-toolgroup-body-not-virtualized` (#3): cap Full/diff expanded bodies with a hard `max_lines` + "+N hidden" footer (mirror the existing `Truncated` policy at `agent.rs:609`), and/or flatten each tool block into its own top-level `FlatItem`. `screens.rs:1299-1323`, `agent.rs:359`.
- `agent-picker-rows-unwindowed` (#11): apply `scroll_to_keep_visible` window or `gpui::list` if session counts grow. `screens.rs:2426/2471`.
- `save-workspace-state` (#15): defensive — coalesce serialize+write onto a background task. Not a current defect.

**Sequencing rationale:** Phase 0/1 are independent, low-risk, and ship value immediately while the helper is designed. Phase 2 is the single highest-leverage deliverable (kills the flagship typing-lag item and proves the helper). Phase 3 reuses that exact helper to clear three more findings with zero new mechanism — the answer to "what else benefits from this treatment": the compose panel, status strip, thinking indicator, and every split/desktop leaf.