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

## 5. Decisions required (write these ADRs first) — ✅ ALL RESOLVED

The overhaul had six forks that are **product/architecture decisions, not
mechanical** — a migration step whose target behavior is an open question is not
yet executable. Each became a short ADR in `docs/decisions/` before its gated
step runs. **As of 2026-06-08 all six are written** (see the ADR pointer on each
below); they are no longer a blocker on the user. What remains gated is the
*execution* of the behavior-changing steps, which is held on the verification
harness (GPUI can't be driven headlessly), not on any open decision.

- **D1 — Turn-end signal.** ✅ **Resolved (ADR-0006, agent-event-stream).**
  Replace the "queue-empty + counter-climbed" inference (re-run in 3 pumps) with
  an explicit turn-end event. Resolution: the phase-8 `AgentEvent` stream carries
  the explicit `TurnEnded` signal; the additive `TurnEnded{generation}` emit
  (8a) landed (`8cdbdd1`). The inference is still in place pending the emit-then-
  observe-then-delete rollout (8b) — held on runtime soak, not on the decision.
  *Gates step 8b.*
- **D2 — Doc/Edit single rope.** ✅ **Resolved (ADR-0007, doc-edit-shared-rope).**
  Binding Doc and Edit views of one file to one `SharedCore` so the Doc view
  tracks live edits from a concurrent Edit window — decided yes. *Gates step 5c
  (49-site staged rewrite, held on the harness).*
- **D3 — Reconnect handle semantics.** ✅ **Resolved (ADR-0008,
  reconnect-handle-semantics).** Swap-in-place keeps old `SessionServerHandle`s
  valid; ADR-0008 settles the trigger-deferred semantics. *Gates step 10.*
- **D4 — Server durability.** ✅ **Resolved (ADR-0009, durable-session-log; +
  ADR-0016 ringbuffer compaction, ADR-0017 WAL-discard migration).** Durability
  + compaction landed via the phase-8 eventlog work (logical `log_base` offset).
- **D5 — cwd migration.** ✅ **Resolved (ADR-0010, canonical-cwd-key).** Drop
  stale un-canonicalized entries (lazy fallback-read); step 4 landed (`c46f023`).
- **D6 — Editor metadata store + agent-slice crate boundary.** ✅ **Resolved
  (ADR-0011, pure-slices-to-lib).** (a) Keep the metadata store typed-multi-field.
  (b) The pure agent slices land as lib-testable modules. Steps 6/7 done
  (`f10486e`, `9253139`).

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
9. **Session-server fusions:** `record()` (log+broadcast) is **done** — the
   only writes outside it are the two intentional carve-outs (`prompt()`
   append-without-broadcast; `broadcast_owner_changed` broadcast-without-log),
   both documented. The remaining `apply_channel_state()` unification (re-apply
   permission mode, drain `pending_prompts`, bump generation on every swap) is
   **done** (9′, `74c4f73`): all three sites route through one chokepoint and
   `restart_session` now drains `pending_prompts` (was: lost prompts queued
   mid-restart). Behavior-changing — ⚠️ owes a runtime check: restart in Plan
   mode stays Plan; prompt during restart reaches the new channel.
11. **Sum-type cleanups:** `InputSurface` enum **done**; `has_unseen_activity`
   was write-only dead code → **removed** (`15fe390`). `ChannelAttachState`
   (folding `channel`+`attach_pending`) **deferred**: those two `Option`s reach
   a real "both `Some`" transient (re-attach while the old channel is still
   live), so they are a 4-state space, not a clean 3-variant enum — a naive
   collapse would change reconnect behavior. Remaining ideas: cross-ring lookups
   by `server_session_id`; collapse the 1:1 `Editor` wrapper.

### Phase B — GPUI-view / behavior-changing / gated (verify via the harness from 0.2)
- **5c.** `DocState` → pooled `SharedCore` (gated on **D2**). ✅ **LANDED**
  (2026-06-08). The foundation was already live (`DocState.source`, `DocSource`,
  `SharedEditor`, `open_and_retain` dedup-by-path, `refresh_blocks` per-frame
  re-derive on `edit_seq`); open/split/restore all bind to the pooled core, so
  Doc + Edit (and splits) of one file share a rope with unified undo. Final fix
  this pass: `re_render_layout_docs` (theme switch) read from **disk**, silently
  reverting unsaved shared-core edits and not self-correcting (since
  `rendered_seq` didn't advance) — now sources the live core via the new
  `re_render_one_doc`. Headless tests: `pool_dedups_by_path_so_two_views_share_one_core`
  (shared rope + unified undo) and `re_render_one_doc_sources_live_core_not_disk`.
  ⚠️ The per-frame cross-pane *paint* (two simultaneous panes visibly updating)
  is the one piece needing a GPUI runtime eyeball.
- **8b.** Delete the turn-end inference in all 3 pumps (gated on **D1** + the 8a
  agreement holding). ⏸️ **Architectural goal ACHIEVED by phase-8** (the canonical
  `AgentEvent` stream is sourced once at the worker boundary via `TurnCount`,
  forwarded verbatim, and folded by the total reducer with a `(generation,turn)`
  exactly-once ledger; agreement pinned by `agent_stream_agrees_*` tests). The
  remaining *deletion* of the legacy inference is the **content-application
  cutover** (before the §9 gate flips, first-turn chunks still come from the
  ReplyEvent path) — the exact double-render risk the gate prevents, deferred to
  post-real-session soak. Making the worker emit unconditionally would also
  inject `TurnEnded` into the durable `event_log`/WAL (server records every
  reply) and perturb the freshly-stabilized replay/cursor/compaction path. NOT
  safely completable headlessly; **held by design**, not by an open decision.
- **10.** `agent_transport` reconnect swaps a shared `Arc<Core>` in place; bundle
  take-once + reattach + wipe into one method; join old threads before swap
  (gated on **D3**). ✅ **Decided scope DONE; swap-in-place deferred per ADR-0008.**
  The decided D3 work — surface reconnect re-attach failures as a visible slot
  error instead of permanent "reconnecting…" — is in the tree:
  `reconnect_session_server` routes re-attach through `spawn_attach_sessions`
  (off paint thread, Owner-reclaim retry), which surfaces read-only / "attach
  failed" / dead-slot outcomes per slot. The `Arc<Core>` swap-in-place is
  **explicitly deferred (HIGH risk, rare path, trigger not fired)** by ADR-0008;
  it is a recorded non-goal, not unfinished work.
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
4. `[x]` **DONE (2026-06-08).** `clippy -D warnings` + `fmt --all --check` now
   gate every push/PR (the `quality` job in `ci.yml` is enabled). The whole tree
   is clippy-clean and fmt-clean.

**Decisions (write ADRs — unblock Phase B):**
5. `[x]` **ALL RESOLVED (2026-06-08).** D1 turn-end signal (ADR-0006) · D2
   Doc/Edit rope (ADR-0007) · D3 reconnect semantics (ADR-0008) · D4 durability
   (ADR-0009 + 0016 + 0017) · D5 cwd migration (ADR-0010) · D6 metadata store +
   crate boundary (ADR-0011). Phase B is now gated on the verification harness,
   not on any open decision.

**Pure extractions (Phase A — each behind CI, no behavior change):**
6. Pure extractions — status (full table in `HANDOFF.md`):
   - `[x]` `replay_turns` (A.1, `6168157`) · `[x]` `overlay` enum (A.2, `e5be921`)
   - `[x]` `settings`/text-zoom (A.3, `e66a54c`) · `[x]` canonical cwd key (A.4, `c46f023`)
   - `[x]` `buffer_pool` (A.5a) — **wired** (landed with 5c): `open_and_retain`
     dedup-by-path + `gc_buffers` strong-count liveness back every file-backed view.
   - `[x]` `DocState` auto-derive (A.5b) — `refresh_blocks` re-derives blocks from
     the shared core keyed on `edit_seq` (`rendered_seq`); no manual invalidation.
   - `[x]` `tool_calls` → `ToolCalls` owner (A.6, `f10486e`)
   - `[x]` `agent_view_model` → `AgentViewModel` owner (A.7, `9253139`) · `[x]` additive `TurnEnded{generation}` (A.8a, `8cdbdd1`) · `[x]` server fusions (A.9 — `record()` already fused; rest is behavior-changing, see below)
   - `[x]` `InputSurface` (`761dfe6`) · `[x]` `has_unseen_activity` dead-code removed (11, `15fe390`); `ChannelAttachState` enum deferred (dual-Option is 4-state, not a clean sum-type — see HANDOFF)
   - `[x]` *(reactive, off-plan)* pipelined-turn crash `50021fc` · mutex-poison `d4cce77`

**GPUI / gated (Phase B — verify via the harness from item 2):**
7. `[x]` `reset_for_replay` delegation (R, `eca7759` — `HighlightCache::reset()`) ·
   `[x]` `apply_channel_state` unification + restart prompt-drain fix (9′,
   `74c4f73`, ⚠️ owes runtime check) · `[x]` **Doc/Edit single rope (5c) LANDED
   (2026-06-08)** — foundation was live; the remaining fix made theme-switch
   re-render source the live shared core instead of disk (`re_render_one_doc`),
   with headless tests for pooled sharing + unified undo + live-core re-render
   (⚠️ cross-pane *paint* owes a GPUI eyeball) · `[~]` delete turn-end inference
   (8b) — **architectural goal met by phase-8 AgentEvent stream; the remaining
   legacy-inference deletion is the content-application cutover, runtime+soak-
   gated (would also perturb the durable WAL); held by design, not by decision**
   · `[~]` `ChannelAttachState` faithful enum (11′ — **held**, refactors the same
   reconnect path as the active reconnect-storm bug; stabilize that first) ·
   `[x]` **reconnect failure-surfacing (10) decided scope DONE** — re-attach
   routes through `spawn_attach_sessions` (read-only / failed / dead-slot
   surfaced); the `Arc<Core>` swap-in-place stays **deferred per ADR-0008**
   (HIGH risk, rare path, trigger not fired — a recorded non-goal).

**Standing rule (the regression→prevention loop):**
8. `[ ]` Every `fix(...)` lands with a failing-test-first; every new derived
   field adds a `Fingerprint` field and a module `reset()`; revive the worklog.

---

## Appendix
- [Appendix A — Full 162-item state inventory with owners](spec-state-architecture-appendix.md)
