# Spec: State-first architecture & overhaul plan

- **Status:** Draft for review
- **Date:** 2026-06-05
- **Scope:** The whole GPUI app (`sketch-gpui`), the session server, and the
  shared lib crates — viewed through the single lens of **who owns which state**.
- **Provenance:** Derived from a full state inventory (162 items across 6
  regions, see [Appendix A](spec-state-architecture-appendix.md)), a state-first
  module synthesis, and an adversarial completeness critique. Companion to the
  regression diagnosis that motivated it.

> **Why this spec leads with state, not modules.** The constant-regression
> diagnosis found that the bugs are not diverse — they are the *same* disease in
> many costumes: **the same state is copied across 4–5 places and reconciled by
> hand-written heuristics.** So the module boundaries here are *derived from*
> ownership of state, not from UI concerns. Architecture is designed around
> state as the primary concern; the modules are the consequence.

---

## 1. The problem, in one number

Of 162 inventoried state elements:

| Classification | Count | Meaning |
|---|---|---|
| source-of-truth | 54 | the authoritative copy — fine |
| **derived-cache** | **39** | recomputable, kept fresh **by hand** |
| **duplicated-copy** | **9** | the *same* fact stored in ≥2 places, synced **by hand** |
| transient-ui | 27 | ephemeral interaction state |
| persistence | 18 | on-disk |
| external-handle | 15 | sockets/threads/focus |

**30% of all state (48 items) must be kept coherent by manual discipline.** Every
one of those is a latent regression: forget to invalidate a cache or sync a copy
and you get a wrong render that *compiles, type-checks, and only fails at
runtime* — which, per `CLAUDE.md`, can't be headlessly verified, so a human is
the only detector. That is precisely the regression profile observed.

The nine `duplicated-copy` items are the sharpest edge — the turn counter `k`
alone exists in five places (agent `AtomicUsize` → server `session.turns` →
server-pump `last_turns` → `replay_fence` → GUI `last_seen_turns`+`replay_turn`),
and the transcript itself is materialized 4×. Both recent bug clusters
(double-render, resume) live here.

---

## 2. State-first design principles

These are the rules the target architecture enforces. Each directly kills a
class of inventoried drift.

1. **Single source of truth; projections, not copies.** Each state item has
   exactly one owning module. Every other view is a *pure derivation*
   recomputed from that owner, keyed on a generation/fingerprint — never a
   hand-synced mirror. *(Kills: 5 turn-count copies, `subagents` mirroring
   `tool_calls`, dual ropes, `last_seen_turns` written by 3 pumps.)*
2. **Make illegal states unrepresentable.** Replace convention-enforced
   invariants with sum types. Five mutually-exclusive overlay `Option`s → one
   `ActiveOverlay` enum; `input_mode` + `chatbox: Option` → `InputSurface
   { Worksheet | Chatbox(Chatbox) }`. `TurnPhase` (already done) is the template.
3. **Identity, not position.** Cross-boundary references use stable ids, never
   positional indices. Slot lookups across rings key on `server_session_id`
   (the ring-local `index` collides at 0 — a real past bug); `focused_subagent`
   and rename targets carry a stable id.
4. **Pure core / impure shell.** All logic that *can* be GPUI-free *is*
   (the lib crates + `replay_turns`, `reconciler`, `tool_calls`,
   `agent_view_model`, `settings`, `overlay`). GPUI views and IO boundaries are
   thin: they own only handles + per-frame scratch and orchestrate the pure
   modules — they hold no business state.
5. **One atomic mutator per coupled invariant.** When two fields must move
   together, exactly one method touches both (`reconcile_list` owns
   `list_state`+`list_item_count`; `record()` owns `event_log`+broadcast;
   editor insert/delete shifts frozen+anchors+llm-line in lock-step). Bypassing
   the chokepoint is the *only* way to desync — so there are no bypasses.
6. **Automatic invalidation over manual.** Derived caches key on a generation
   counter (`edit_seq`) or an explicit fingerprint that names **every**
   structural input — never a coarse proxy (`frozen_line_count`) and never
   "remember to call `invalidate_x`". Adding a render dependency must *require*
   adding a fingerprint field, so staleness can't sneak in.
7. **Reset is total and centralized — by delegation.** Session/replay teardown
   clears every derived + reconciliation field in one place
   (`reset_for_replay`), which **delegates to each sub-module's own `reset()`**
   rather than reaching into their fields. A new derived field is wrong unless
   its module's `reset()` clears it. *(Note: the live code's `reset_for_replay`
   does not clear `agent_mode` — the delegation refactor is the moment to decide
   if that omission is intentional.)*

---

## 3. The primary modules (state-first decomposition)

20 modules in three purity tiers. Each owns a disjoint slice of state behind a
narrow API; nothing else touches that state directly. This answers **"what are
the primary modules that make up the app?"** — they are the owners of state.

### Tier 1 — Pure core (no GPUI, unit-testable today)

| Module | Owns (key state) | Public API sketch | Extracted from |
|---|---|---|---|
| **`editor_core`** | `Document` (rope, `edit_seq`, undo), `EditorCore` (frozen_lines, lockable_through_line, anchors, `line_turn` map, last_llm_line), `EditorView` (cursor/selection/insert-mode) | `insert/delete` (shift frozen+anchors+llm atomically), `freeze_as_user_turn`, `line_for_anchor`, `line_turn`, `undo/redo`, `edit_seq()` | `document.rs`, `editor.rs` |
| **`replay_turns`** | the turn number `k` (`last_seen`, `replay_turn`) | `current_turn`, `advance_user_boundary`, `finish_replay`, `on_turn_ended` | `acp_channel.rs:210` (already pure) |
| **`user_turn_reconciler`** | `pending_local`, `last_inserted`, `user_turn_ks` tripwire | `reconcile(origin,text,replaying)`, `note_turn_progressed`, `reset` | `agent_transcript.rs` (already pure) |
| **`tool_calls`** | `tool_calls`, `tool_call_order`, `tool_call_anchor_line`, `expanded` (ToolCallKey), **`subagents` (derived)**, `focused_subagent` (ToolCallKey) | `on_tool_call_start/update`, `toggle_expanded`, `subagents()` (derived projection), `re_resolve_anchors`, `fingerprint_inputs()` | `main.rs:4960–5040` |
| **`agent_view_model`** | `lines_cache`, `flat_items_cache`, `gutter_cache`, `view_model_fp/seq`, `block_cache`/`block_cache_frozen_count` | `build(editor,&theme,tool_fps,…) -> (flat,gutter)` memoized on **one `Fingerprint`** | `main.rs:4976–5011`, `memoize_view_model` |
| **`highlight_cache`** | per-view incremental highlight caches (shares one injected `Highlighter`) | `snapshot_syn(lines,&theme,edit_seq)`, `reset` | `main.rs:14232`, `highlight_cache.rs` |
| **`settings`** | `text_scale`, `body_font`, `code_font` (+ consolidates the **already-persisted** `theme`/`agent_status_position` from `preferences.json`) | `theme()`, `text_scale()->f32`, `set_*`, `zoom_*`, `load/save` | `main.rs:5837–5844`, `Preferences` 2108 |
| **`overlay`** | one `ActiveOverlay` enum (was 5 `Option`s), `transient_status`, `splash_until` | `active()`, `open(Overlay)` (replaces, never stacks), `clear()`, `toast()` | `main.rs:5852–5866` |
| **`workspace`** | `tabs`, `active_tab`, split tree, `focused`, per-tab `rail`, id allocators | `split/close/focus_*`, `replace_focused_content`, `restore_from_persisted` | `workspace.rs` (already correct owner) |
| **`buffer_pool`** | `file_buffers`, `path_index`, `SharedCore` (one rope per canonical path) | `open(path)->SharedCore`, `gc()` (single liveness: strong_count + modified) | `workspace.rs:355–559` |

### Tier 2 — GPUI view shell (thin; orchestrates the core)

| Module | Owns (key state) | Notes |
|---|---|---|
| **`view_shell`** (`SketchGpuiView`) | `focus_handle`, `viewport_width_px` (frame scratch); **physically holds** `workspace` but only `workspace`-module methods mutate it | The thin root: clears frame caches, branches on `WindowContent`, routes actions to owning modules' APIs. Holds nothing authoritative except GPUI-required handles. |
| **`window_content`** (Doc/Edit/Browser) | `DocState` (blocks-derived, `cursor_block`/`last_cursor_block`, `doc_selection`, `line_layouts` hit-test scratch), `EditState`, `BrowserWindow`/`FileBrowser` (`filtered_indices`/`search_results` rebuilt together) | Doc & Edit bind a `buffer_pool` `SharedCore`; view-mode toggle is one buffer, no stashed parallel editor. |
| **`follow_tail`** | `list_state`, `list_item_count`, `last_scrolled_edit_seq`, `follow_output`, **`block_ranges`** (moved here — see fix below) | `reconcile_list(count)` is the sole mutator of `list_state`+`list_item_count`; owning `block_ranges` removes the cross-module hot-path read. |
| **`agent_session`** (per slot) | `channel`, `attach_pending`, `_pump`, `server_managed`, `turn_phase`, `InputSurface`, `mode`/`keybinds`, `status`, `current_plan`/`agent_mode`/`usage`, `tasklist_open`/`subagents_open` | Composes the Tier-1 agent slices for one slot. `submit()` is the single chokepoint; `reset_for_replay()` **delegates** to each sub-module's `reset()`. |
| **`agent_ring`** | `slots`, `active`, `next_index`, `underlying`; `AgentSlot` identity (`server_session_id`/`resume_id`/`pending_open_token`) | Cross-ring ops key on `server_session_id`; `has_unseen_activity` marking scoped to the originating ring (fixes cross-ring index contamination). |

### Tier 3 — IO boundaries

| Module | Owns (key state) | Notes |
|---|---|---|
| **`persistence`** | `workspace.json` + `acp_sessions.json` behind **one canonical cwd key**; the not-persisted policy for per-window scroll/cursor | Guarantees the two files agree on which agent windows exist; saves **all tabs** (fixes active-tab-only bug). |
| **`agent_transport`** | `session_server` handle, `is_candidate`, `candidate_promote_ready`, client `Core` (request_tx/pending/connected/next_id), reader/writer threads | `reconnect()` swaps a shared `Arc<Core>` in place so handles stay valid; `take_pump_channels()` bundles take-once + reattach + transcript-wipe. |
| **`session_server`** | `ManagedSession` (channel, generation, **turns**, permission_mode, owner, pending_prompts, `event_log`, replay_fence), sessions registry, conn table | **The authoritative transcript owner.** `record()` fuses log+broadcast; `spawn_channel_then_apply_state()` re-applies permission_mode + drains prompts + bumps generation on every swap. |
| **`acp_channel`** | `turns` (`AtomicUsize` — **the turn-count truth**), `session_id`, generation token, **applied permission policy (`AtomicU8`)**, `ReplyEvent` vocab incl. explicit `TurnEnded` + `ReplayComplete` | The single honest "turns completed on this connection." Emits an explicit `TurnEnded{count,generation}` so no consumer re-infers turn-end. |

### Critique fixes baked into the tables above
- **`block_ranges` moved to `follow_tail`** (not `agent_view_model`): it is read only by `reconcile_list`'s reset-vs-splice decision and scroll math — both list concerns. This removes a module reaching into another's state on the hottest path.
- **`settings` exposes `text_scale()->f32`**; the *shell* threads it into `RenderCtx` (settings is pure and never touches the render path).
- **`acp_channel` owns the applied permission policy (`AtomicU8`)**; `session_server.spawn_channel_then_apply_state` is the single writer that syncs it on every channel swap (it currently reverts silently on restart).
- **`subagents` is a derived projection** of `tool_calls`, never a hand-synced mirror.
- **`reset_for_replay` delegates** to each module's `reset()` (principle 7) rather than clearing 6 modules' fields itself.
- **Missed state now owned:** `DocState.cursor_block`/`last_cursor_block`, `FileBrowser.filtered_indices`/`search_results` (with a rebuild-together method à la `reconcile_list`), the channel permission `AtomicU8`, and the explicit "per-window scroll/cursor is *not* persisted" policy (owned by `persistence`).

---

## 4. State → owner map

The complete 162-item map with file:line, classification, owner, and drift risk
is **[Appendix A](spec-state-architecture-appendix.md)**. The items that *matter
most* — the 9 `duplicated-copy` hazards — and their resolution:

| Duplicated fact | Copies today | Single source of truth (target) |
|---|---|---|
| turn number `k` | agent `AtomicUsize`, `session.turns`, pump `last_turns`, `replay_fence`, GUI `last_seen_turns`+`replay_turn` | `acp_channel.turns` is the connection truth; `replay_turns` is the GUI's only `k`; server `turns` set only from the explicit `TurnEnded` event |
| the transcript | server `event_log`, GUI `editor` rope, (×panes) | server `event_log` is durable truth; GUI editor is a projection rebuilt on attach/replay |
| `subagents` | mirror of `tool_calls` | derived projection (computed) |
| permission mode | `ManagedSession.permission_mode` + channel `AtomicU8` | channel policy applied by the one swap method |
| which agent windows exist | `acp_sessions.json` + `session_server.json` (reconciled by cwd) | server sessions by id; GUI persists only "which sids were open where" |
| Doc vs Edit text | `DocState` Document + `EditState` Document of same file | one pooled `SharedCore` per canonical path *(gated — see D2)* |

---

## 5. Decisions required (write these ADRs first)

The overhaul has six forks that are **product/architecture decisions, not
mechanical** — a migration step whose target behavior is an open question is not
yet executable. Each becomes a short ADR in `docs/decisions/` before its gated
step runs.

- **D1 — Turn-end signal.** Replace the "queue-empty + counter-climbed"
  inference (re-run in 3 pumps) with an explicit `AcpChannelClient::TurnEnded`
  event? Or is the inference load-bearing for tool-only turns / compaction?
  *Gates step 8b.*
- **D2 — Doc/Edit single rope.** Binding Doc and Edit views of one file to one
  `SharedCore` makes the Doc view track live edits from a concurrent Edit
  window. Is that desired, or must Doc be a frozen snapshot until reload?
  *Gates step 5c.*
- **D3 — Reconnect handle semantics.** Swap-in-place keeps old
  `SessionServerHandle`s valid but lets a handle silently target a new server.
  Desired, or should stale handles fail loudly? *Gates step 10.*
- **D4 — Server durability.** Add periodic atomic checkpoints + `event_log`
  compaction/bounding now (turns the forwarder cursor into a logical offset), or
  defer? *Affects whether `sent`/`replay_fence` stay absolute indices.*
- **D5 — cwd migration.** Canonicalizing the cwd key orphans existing on-disk
  entries written under the un-canonicalized key. One-time re-key migration, or
  is dropping stale entries acceptable? *Gates step 4.*
- **D6 — Editor metadata store + agent-slice crate boundary.** (a) Collapse the
  type-erased `LineMetadataStore` to a concrete `line_turn` map, or keep it
  typed-multi-field for a near-term second metadata type (per-line
  comments/diagnostics)? (b) Do the pure agent slices (`replay_turns`,
  `reconciler`, `tool_calls`, `agent_view_model`, `follow_tail`,
  `highlight_cache`) live in a lib crate (testable without GPUI, reusable by the
  TUI) or stay in the bin? **Decide before steps 6/7.**

---

## 6. Migration plan (incremental, each step verifiable)

**No big-bang.** The sequence is critique-revised: pure, unit-testable
extractions are front-loaded (they land behind CI immediately); every
GPUI-touching or behavior-changing step is split out and gated. The two
cross-cutting enablers come first because they make everything after them
verifiable.

### Phase 0 — Enablers (do first; unblock verification)
- **0.1 CI gate** — ✅ *done* (`.github/workflows/ci.yml`: build both bins + test on every push/PR).
- **0.2 Keymap extraction** — ✅ *done* (`704e13d`). Extracted `register_keymap(app)`
  (all 96 bindings, verbatim) callable from both `main()` and the test harness;
  landed the first headless action smoke (`cmd_b_toggles_file_browser_rail` in
  `verify_harness.rs`) driving the full keymap→action→handler chain. **This is
  the single unblock that makes every Tier-2 (gpui-view) step verifiable**
  instead of "human eyeballs it" — `vcx.simulate_keystrokes(...)` now works.

### Phase A — Pure, front-loaded, low/med risk (unit-testable, no behavior change)
1. ✅ **`replay_turns` owns its fields — DONE.**
   - ✅ **Field-ownership refactor** (`6168157`). `AgentState` now holds one
     `ReplayTurns` (the two loose `last_seen_turns`/`replay_turn` fields are
     gone); the reconstruct-on-read accessor + copy-back-out mutators are
     deleted, the turn methods delegate in place, and the pump/server turn-end
     sites read/write `replay_turns.last_seen` directly. Pure, no behavior
     change; net −23 lines; full suite green.
   - ✅ **Plus** the pipelined-submit crash fix (`50021fc`): non-replay inserts
     take `max(current_turn(), next_unused_user_turn())` so a submit made while
     the previous turn is in flight gets a distinct `k` instead of tripping the
     M3 tripwire. ⚠️ owes a human runtime check.
   - ✅ **Worksheet submit now routes through the reconciler chokepoint**
     (`a6f2829`). Extracted `register_user_turn() -> Option<k>` (reconcile +
     `current_turn()` k-derivation + `user_turn_ks` tripwire) as the shared
     core; `insert_user_turn` appends, the new `commit_worksheet_turn` freezes
     authored lines in place. `submit_worksheet` dropped its hand-rolled
     `last_seen_turns + 1` and sends-first/commits-on-success (also closing a
     freeze-on-failed-send phantom). *Fixed the live worksheet double-render.*
     Tests: `agent_seam_worksheet_submit_suppresses_double_render` + a
     non-vacuous negative control + a pure multi-line reconciler test. ⚠️ The
     send-first reorder is a send-FAILURE behavior change the headless harness
     can't verify — **owes a human GUI runtime check**.
2. ✅ **`overlay`: 5 `Option`s → `ActiveOverlay` enum** (`e5be921`). One
   `active_overlay` field + per-variant accessors + open/clear/has_overlay;
   ~65 sites migrated; `transient_status`/`splash_until` left separate. Unit
   test `active_overlay_open_replaces_and_clears`. ⚠️ one intentional
   strictly-better divergence (rename-behind-menu no longer strands the menu) —
   owes a runtime eyeball.
3. ✅ **`settings`: persist text zoom + one `save_settings()`** (A.3). Added
   `text_scale` persistence (restored on launch); consolidated the two
   hand-rebuilt `save_preferences` sites into one snapshot method. Fonts not
   persisted (no setter yet). Round-trip + forward-compat test. ⚠️ relaunch
   zoom check owed.
4. ✅ **canonical cwd key (D5/ADR-0010)** + save-all-tabs (`persist_cwd_key`,
   lazy fallback-read, all 4 on-disk sites). save-all-tabs was already done;
   the `persistence` module FILE-extraction is deferred as organizational.
   Symlink round-trip test.
5a. **`buffer_pool` extraction**, single liveness (strong_count + modified).
   Verify: `gc` reaps unmodified zero-view buffers, retains modified.
5b. **`DocState.blocks` → `edit_seq`-keyed auto-derivation** while *still* owning
   its `Document` (removes manual invalidation, **no behavior change**). Verify:
   edit bumps `edit_seq` → blocks recompute without an explicit invalidate call.
6. **`tool_calls` registry** + derived `subagents` + anchor re-resolve hook +
   `ToolCallKey` expand set. *(Extract before the view-model — it's a
   dependency.)* Verify: a `ToolCallUpdated` reflects in the derived subagent
   with no second write.
7. **`agent_view_model`**: one `Fingerprint`-gated memoizer; **separate the pure
   flat-build from the gpui per-item element build** (the nontrivial surgery —
   not a lift-and-shift). Block-cache key → content hash of the frozen region.
   Verify: port the memo fast-skip test; a constant-line-count reshape bumps
   `view_model_seq`.
8a. **Emit `AcpChannelClient::TurnEnded{count,generation}` additively**, with a
   debug-assert/log that it agrees with the existing inference for a release
   (gated on **D1**). Verify: respawn resets counter + new generation; consumer
   rebaselines.
9. **Session-server fusions:** `record()` (log+broadcast) and
   `spawn_channel_then_apply_state()` (re-apply permission mode, drain
   `pending_prompts`, bump generation on every swap). Verify: restart in Plan
   mode stays Plan; prompt during restart reaches the new channel.
11. **Sum-type cleanups:** `InputSurface` enum; `has_unseen_activity` scoped to
   the originating ring; cross-ring lookups by `server_session_id`; collapse the
   1:1 `Editor` wrapper. Verify: same-index two-ring activity isolation;
   `InputSurface` makes `(Chatbox,None)` non-constructible; TUI suite green.

### Phase B — GPUI-view / behavior-changing / gated (verify via the harness from 0.2)
- **5c.** `DocState` → pooled `SharedCore` (gated on **D2**). Verify: edit in an
  Edit window reflects in the Doc view; harness round-trip.
- **8b.** Delete the turn-end inference in all 3 pumps (gated on **D1** + the 8a
  agreement holding).
- **10.** `agent_transport` reconnect swaps a shared `Arc<Core>` in place; bundle
  take-once + reattach + wipe into one method; join old threads before swap
  (gated on **D3**). Verify: pre-reconnect handle routes post-reconnect; exactly
  one transcript wipe.
- **R.** `reset_for_replay` delegation refactor (principle 7) — lands after the
  sub-modules it delegates to exist (after 1, 6, 7); decide the `agent_mode`
  reset question here.

---

## 7. Execution backlog — the consolidated solve-list

The ordered list to actually do this. `[x]` done, `[~]` decision-gated.
Front-loaded by leverage and verifiability.

**Stop-the-bleeding (days, do now):**
1. `[x]` CI gate (build + test on push/PR). *(merged via this branch)*
2. `[x]` **Keymap extraction → headless action smokes** (Phase 0.2, `704e13d`).
   Highest non-CI leverage — unblocks verifying every GUI change. Rail smoke
   (`cmd_b_toggles_file_browser_rail`) landed as the first one.
3. `[x]` **Worksheet double-render fix** (Phase A.1 worksheet part, `a6f2829`).
   Closed the "single chokepoint with two doors": `register_user_turn` core +
   `commit_worksheet_turn`. (The `replay_turns` field-ownership refactor — the
   other half of A.1 — remains; see item 6.) ⚠️ owes a human runtime check.
4. `[ ]` Turn on `clippy -D warnings` + `fmt --check` in CI once the 8 existing
   warnings are cleared (uncomment the staged `quality` job).

**Decisions (write ADRs — unblock Phase B):**
5. `[~]` D1 turn-end signal · D2 Doc/Edit rope · D3 reconnect semantics ·
   D4 durability · D5 cwd migration · D6 metadata store + crate boundary.

**Pure extractions (Phase A — each behind CI, no behavior change):**
6. Pure extractions — status (full table in `HANDOFF.md`):
   - `[x]` `replay_turns` (A.1, `6168157`) · `[x]` `overlay` enum (A.2, `e5be921`)
   - `[x]` `settings`/text-zoom (A.3, `e66a54c`) · `[x]` canonical cwd key (A.4, `c46f023`)
   - `[~]` `buffer_pool` (A.5a) — deferred: dead/unwired, do with D2 (5c)
   - `[~]` `DocState` auto-derive (A.5b) — deferred: memo half already done
   - `[x]` `tool_calls` → `ToolCalls` owner (A.6, `f10486e`)
   - `[ ]` `agent_view_model` (A.7 — see HANDOFF gotcha) · `[ ]` additive `TurnEnded` (A.8a) · `[ ]` server fusions (A.9)
   - `[x]` `InputSurface` (`761dfe6`) · `[ ]` rest of sum-type cleanups (11: `has_unseen_activity` scoping, …)
   - `[x]` *(reactive, off-plan)* pipelined-turn crash `50021fc` · mutex-poison `d4cce77`

**GPUI / gated (Phase B — verify via the harness from item 2):**
7. `[~]` Doc/Edit single rope (5c, gated D2) · delete turn-end inference
   (8b, gated D1) · reconnect Arc<Core> (10, gated D3) ·
   `reset_for_replay` delegation (R).

**Standing rule (the regression→prevention loop):**
8. `[ ]` Every `fix(...)` lands with a failing-test-first; every new derived
   field adds a `Fingerprint` field and a module `reset()`; revive the worklog.

---

## Appendix
- [Appendix A — Full 162-item state inventory with owners](spec-state-architecture-appendix.md)
