# Backlog

Open, deferred, and flagged work. Higher fidelity than git: each item says what,
why it's here, and its status. Updated at session end (`/worklog`) and as items
move. Past work lives in `docs/worklog/`; the *why* of choices lives in
`docs/decisions/`.

Status legend: `IN-FLIGHT` (agent/branch active) · `READY` (scoped, not started)
· `DEFERRED` (deliberately not now, reason given) · `NEEDS-DECISION` (waiting on
the user) · `NEEDS-RUNTIME` (built + state-tested headlessly; awaiting human
confirmation of *pixels / timing / OS-behavior* specifically — not "no test was
possible." State-level behavior is testable headlessly via `verify_harness.rs`).

---

## Features

- **Image paste into a session** — `NEEDS-RUNTIME` (built 2026-07-09, branch
  `image-paste`, NOT yet merged; INV-UX-21). Cmd+V of a clipboard image stages it
  as a pending attachment (chip above the compose), sent on submit as an ACP
  `ContentBlock::Image` (both submit paths) with a `🖼 image N (EXT)` transcript
  marker; attachments clear after send and are ephemeral (not persisted). Wire
  carries `Request::Prompt.images` additively. Headless-tested end-to-end (paste
  staging, mixed content-block build, wire round-trip, real worksheet submit; 2
  negative controls RED). Human check (harness gap 2, live loop): the
  `claude-agent-acp` adapter actually advertises the `image` prompt capability +
  reads the pasted image — NOT gated on the capability yet, so verify it doesn't
  error; gap 1 for the chip glyphs. See `docs/worklog/2026-07-09-image-paste.md`.
- **Session recap panel** — `NEEDS-RUNTIME` (built 2026-07-09, on `main`
  `36bdc8a`; INV-UX-20). Agent space-menu `R` ("recap this session") generates an
  LLM prose summary of the focused session on a THROWAWAY isolated
  `AcpChannelClient` subprocess and pins it at the top of the jump panel, above
  the session list; re-runnable (`⟳`), dismissed (`✕`), pinned until dismissed.
  Reducer + panel + token-guard supersession are headless-tested (7 `recap_*`, 2
  negative controls RED). Human check (harness gap 2, live subprocess): with the
  agent on PATH, `R` streams a summary in, `⟳` re-runs, `✕` dismisses, and the
  throwaway worker EXITS (no lingering `claude-agent-acp`); gap 1 for the panel's
  exact look. The `spawn_recap_worker`→pump wiring is the only untested seam
  (`cfg(test)`-skipped).
- **Agent model switcher (per session, live)** — `NEEDS-RUNTIME` (built
  2026-07-09, merged to `main`; INV-UX-22, `docs/projects/agent-model-switch/`).
  Switch a tile's model (Opus / Fable / Sonnet / …) live from the agent's
  advertised picklist via ACP `session/set_config_option` — `space M` submenu or
  the clickable `model ▾` status-strip badge. Full suite green + 3 new headless
  tests (each negative-controlled) + `#[ignore]` live round-trip. Rebuild +
  restart to pick it up in the running binary. Follow-up: the `effort` option
  (low..max) could get the same treatment. Human check: the badge shows `▾` +
  opens the menu, picking a model flips the badge live and the next turn uses it.

- **Jump panel (root-level navigator)** — `NEEDS-RUNTIME` (built 2026-06-22,
  merged `e3fa254`/`720b7a0`; spec `spec-jump-panel.md`, ADR-0021). Always-visible
  left sidebar (Pinned placeholder · Workspaces · Agent sessions), `cmd-j`/`?`
  toggle (persisted), free-session select → ephemeral virtual workspace. Inline
  render (cheap; a root-reading cached child double-leases). Human check: visible
  across workspace switches, active workspace highlighted, free-session
  open-then-vanish on jump-away.
- **Universal agent roster** — `NEEDS-RUNTIME` (built 2026-06-22, merged
  `4ec7a62`; spec `spec-universal-agent-list.md`, ADR-0022). One `AgentRoster`
  (all server sessions, live on Created/Closed/Renamed broadcasts, seeded at
  boot); jump panel + tile selector both project from it. Human check: a session
  created/renamed/closed elsewhere updates both surfaces live; selecting one
  moves it free↔bound in both.
- **Workspace cwd is a required typed field** — `NEEDS-RUNTIME` (built
  2026-06-22, merged `1329898`/`e942960`; ADR-0023). `Tab.cwd: WorkspaceCwd`
  (private, required); a new agent inherits the LIVE active-workspace cwd; Set
  CWD persists across restart. Human check: Set CWD → new agent runs in that dir;
  survives relaunch. NOTE: pre-existing `~/.yalda/workspace.json` entries have no
  stored cwd → the first Set CWD per workspace populates it going forward.
- **Desktop mode** — `NEEDS-RUNTIME` (built 2026-06-10, spec
  `spec-desktop-mode.md`, engine `1f7c269^..1f7c269` on master). Fifth
  per-tab LayoutMode (`Ctrl-W Space` cycle, sigil `[#]`): fixed-size tiles
  (global `{cols}x{rows}` via `Ctrl-W p`, default 120×40) on a pannable slot
  grid; drag tiles by title bar (insert-and-shift, right-click cancels);
  spatial focus via the usual `Ctrl-W h/j/k/l`. Human checklist: drag feel +
  drop targeting, scroll-pan + edge auto-pan, typing/keys inside each tile
  kind (Doc/Edit/Browser/Agent), focus-offscreen recovery (focus a panned-out
  tile → auto-reveal), mode round-trips (Manual ↔ Desktop preserves both
  arrangements), restart persistence. Deferred polish, in spec but not v1:
  Esc-to-cancel drag at canvas root (global escape binding would shadow
  per-screen escape; needs a careful dispatch design); measured mono cell
  size (currently 0.6em/1.4em approximation in `desktop_tile_px`).

## Bugs

- **Worksheet resume: cursor lost / undo erased the buffer / tool calls at the
  bottom** — `FIXED` + `NEEDS-RUNTIME` (2026-06-22, merged `1560db7`/`a7beb83`;
  worksheet-frozen-blocks ticket 001). Data was always safe (server WAL). Three
  fixes, headless-tested: (F2) `programmatic_insert` didn't shift the view caret
  → `Editor::splice_insert/_delete`; (C3) `undo` reset line anchors → now SHIFTS
  them; (THE repro) `begin_insert` opens one undo group and agent chunks streamed
  mid-insert recorded into it → agent/programmatic splices are now non-undoable
  (`*_no_undo` + `shift_recorded_splices`). Human check: reopen a multiturn
  worksheet session, type, let it stream, undo — your edits revert, the
  transcript stays; caret findable; `G` reaches the bottom.
- **Worksheet caret rendered below the visible buffer (on entry / nav)** —
  `FIXED` + `NEEDS-RUNTIME` (2026-06-22, ticket-001 fingerprint item).
  `view_model_fingerprint` folded in neither the input surface nor the worksheet
  caret line, so entering Worksheet mode (or moving the caret onto a collapsible
  blank) reused a flat list that stripped the trailing editable tail → caret on
  a roomless line. Fix: fold `InputSurface::Worksheet` + the worksheet caret line
  into the fingerprint (option 1, worksheet-scoped — chatbox typing stays
  render-flat); `finish_replay` snaps the caret to the editable tail on reopen.
  Human check: a `--release` `sample` holding `j` in a huge worksheet to confirm
  the per-nav S1 rebuild is imperceptible.
- **Worksheet ticket-001 remaining (deferred deep)** — `SUPERSEDED by Model C`
  (2026-06-24, ADR-0024). The **floor-only-EOF** edge case no longer exists: the
  user draft lives in a separate `Compose` buffer, never in the transcript, so
  there's no mid-document draft for a stream to overwrite; `agent_tail_floor_char`
  always returns EOF and the `append_llm_chunk_floored` path is inert. Pinned by
  `inv_order_*`. Ticket closed.
- **Mid-turn message drops (lease gate + invisible rejection)** — `FIXED`
  (2026-06-09, `b7bdcde` on master); `NEEDS-RUNTIME` for the GUI
  PromptRejected surfacing (notice + chatbox restore — headless tests cover
  the server half only). Root cause was two-part: `prompt()` is
  fire-and-forget so a server rejection had no waiter (the optimistic echo
  made it look sent), and `do_prompt` demanded a LIVE lease — an App-Napped
  window's lease lapses during a long turn, so the first post-wake message
  raced the 5s heartbeat reclaim and silently lost. Fix:
  `acquire_or_renew_lease` (action-as-liveness, shared with Owner attach) on
  prompt/cancel/mode/restart + `Notification::PromptRejected` to the
  submitter with the text restored into the chatbox. Tests 3b/3c/3d in
  `session_transcript_test.rs` (red on old gate, green now).
- **Agent transcript typing lag (worksheet + while-streaming)** — `FIXED`
  (2026-06-09, `8af1d4c` merged to master); `NEEDS-RUNTIME` (worksheet typing
  feel + typing-while-streaming on the real resumed session). Both shared one
  hot path: every `edit_seq` bump (worksheet keystroke; every streamed chunk)
  misses the S1 view-model cache, and the rebuild (a) deep-cloned EVERY
  parsed `RenderedBlock` into per-rebuild lookup maps, and (b) on streaming,
  re-parsed (pulldown-cmark + syntect) the WHOLE frozen transcript per chunk
  because the block cache was keyed by `(start,end)` and chunk inserts shift
  every range. Fix: S1 rebuild extracted to `rebuild_agent_view_model()`
  (headlessly testable — first real seam into GPUI render cost, progresses
  the verification-harness goal); `FlatItem::Block(Rc<RenderedBlock>)` +
  `resolved_blocks` (Rc bumps, no clones); content-hash block-cache keys
  (parses survive range shifts); metadata-view hoist in the cursor-reveal
  loop. Probe: 3,151 lines / 50 code blocks → ~135µs per keystroke rebuild
  (debug). Identity/INV-10/probe tests in the gpui tests mod.
  Left open (minor): tag-bar `all_tags()` walk + per-leaf `mark_for_window()`
  scan per frame (new in 09e266b, small constants).
- **Theme switch leaves agent transcript caches stale** — `FIXED` (2026-06-12,
  `91a6885`; re-confirmed 2026-06-25). `set_theme` calls
  `AgentViewModel::invalidate_theme()` (clears `block_cache` +
  `block_cache_frozen_fp` + `view_model_fp`) for every live session, rebuilds the
  edit-view syntect highlighter, and busts every transcript view via
  `notify_transcript_views`. The READY entry was stale; the fix landed right after
  it was filed.

- **Resume hang (replay fence never cleared)** — `FIXED` (2026-06-09,
  `9112188` on master). After a server restart, a recovered session's pump
  fence waited for the channel turn counter to reach the restored count — but
  the counter restarts at 0 every spawn and `092c218` removed the post-load
  bump, so the fence never cleared and EVERY post-resume event (replay, marker,
  live turns) was silently discarded: prompts looked hung while the agent
  worked invisibly (a queued "integrate" actually ran + folded a branch to
  master unseen). Fix: marker-based fence (`src/replay_fence.rs`), worker emits
  `ReplayComplete` on every resume attempt incl. fallbacks, pump reports
  session-absolute TurnCounts (`turn_base +`), restart-with-resume arms the
  fence (kills the restart double-record). Regression test:
  `recovered_session_is_drivable_after_resume` (red pre-fix, green post-fix).
  Residual hazard noted in code: a timed-out `session/load`'s late replay
  notifications can record as live events (bounded duplication, not a wedge).
- **Leaked `claude-code-acp` adapter processes** — `FIXED` (2026-06-25,
  `fd858d7`). Graceful exits already reap via `kill_on_drop`; the leak was the
  crash/SIGKILL/panic path where Drop never runs and the adapter reparents to
  PID 1. Fix: a startup reaper in both binaries' `main()`
  (`acp_channel::reap_orphaned_adapters`) SIGKILLs adapter processes with
  `ppid == 1` (definitively orphaned — can't hit a live session's adapter) whose
  command matches an adapter needle. Pure parser `orphaned_adapter_pids` is
  unit-tested. (A deeper per-close pump-join was considered unnecessary — the
  graceful path already reaps; the reaper covers the rest.)
- **Reconnect bursts at GUI launch** — `NEEDS-RUNTIME` (probable root cause
  fixed 2026-06-10, `3f85365`). The shared server pump was stored in an agent
  SLOT; every slot-state replacement during startup (restore → re-bootstrap →
  set_screen) cancelled the pump, dropped the notification receiver, killed
  the connection, and triggered a reconnect — hence ~25 conns per launch and,
  once timing shifted, hard "attach failed: session server disconnected" for
  new sessions. Pump is now a view-lifetime singleton (like the lease
  heartbeat). Verify the burst is gone in the server log after a few
  launches.

- **Edit-view typing crash + latency** — `FIXED` (2026-06-06). `reparse` fed
  tree-sitter a stale (never-`edit()`'d) tree → nondeterministic SIGSEGV
  (`d32edf9`); then full-parse-per-keystroke was slow → proper incremental
  reparse, fuzz-guarded, 10–20× faster (`413da19`). See worklog
  `2026-06-06-reparse-segfault-and-incremental.md`.
- **Reparse may be wasted work** — `CONFIRMED + READY` (verified 2026-06-25).
  The tree-sitter tree (`tree_state` / `block_boundaries`) is consumed ONLY
  inside `tree.rs` + `editor.rs` + tests — nothing in the GPUI render path reads
  it. So the per-edit `reparse` (editor.rs:1029/1065/1136/1169/1197) maintains a
  tree the live app never renders from. Making it lazy/skippable would remove
  that cost. NOT done here: it touches the editor hot path and the incremental
  reparse already cut the cost 10–20×, so low priority — but the premise is now
  confirmed, not speculative.

- **Session-server reconnect storm** — `ROOT-CAUSED + FIXED on branch
  session-resilience` (2026-06-07); `NEEDS-RUNTIME` for the GUI reconnect path.
  **Root cause:** `SessionServerClient` had no socket shutdown on drop. Its
  reader thread is detached and blocks forever on `lines()`, so dropping the
  client (notably `reconnect()`'s `*self = fresh`) leaked the thread AND kept
  the socket fd open — the **server never saw the disconnect, so it never
  released session ownership**. Every in-place `reconnect()` orphaned a zombie
  owner; the next re-attach was rejected with "another GUI already owns this
  session", and the connection only truly closed at process exit. That is the
  489-reconnects / few-closes pattern and the `close/create … disconnected`
  round-trip failures. **Fix (4 files):** (1) `Drop` now `shutdown(Both)`s the
  socket so the reader unblocks and the server releases ownership at once;
  (2) reconnect re-attach moved off the paint thread via the existing
  `spawn_attach_sessions` (Owner-reclaim retry) instead of raw inline blocking
  attaches that also froze rendering; (3) `attach_owner_with_retry` added to the
  client lib for the residual teardown-vs-reattach race; (4) single-instance
  guard — a 2nd server on a live socket exits instead of stealing it and
  orphaning sessions; (5) `pid_file_path`/persist path now follow
  `YALDA_SESSION_SOCKET` (enables isolated instances + the guard). **Verified:**
  new headless harness `tests/session_resilience_test.rs` drives the REAL server
  binary (no agent needed) — reproduces the storm without the fix; with it, 30
  sequential restarts + in-place reconnect + duplicate-server guard all pass,
  every connection closes (no zombies). **Still owed:** human runtime check that
  the GPUI app reconnects seamlessly after the server reader thread sees EOF
  (GPUI can't be driven headlessly). Was suspected to be the attach-replay
  broadcast lag — that path was already self-healing; the real cause was the
  missing shutdown.

## Session-server hardening + actor extraction (all MERGED to `master`)

Phase-3 + phase-7 of `spec-session-server-actor.md`. All landed on `master`
(`bd796d4`,`1e2c881`,`03e8d10`,`23747a0`,`a70ef74`); branches deleted. Headlessly
verified via the resilience+transcript harness. Worklogs:
`2026-06-07-session-server-hardening.md`, `2026-06-07-actor-extraction-and-perm-ux.md`.

- **Permission default + `0600` socket** — `DONE` (ADR-0014 + addendum). Now
  **Yolo, config-driven** (`default-permission-mode` in config.kdl; server loads
  config in `create_session`); `DEFAULT_PERMISSION_MODE` is the no-config
  fallback. Socket `0600` (TOCTOU-closed via `umask`-around-`bind`). Owner-gated
  escalation. Runtime-confirmed by user.
- **Permission mode visible + cyclable in the server model** — `DONE` /
  runtime-confirmed. `AgentState.permission_mode` from `SessionInfo`; status-strip
  badge always renders; `<space> c m` cycles via the `SetPermissionMode` wire verb.
- **Structured tracing + `admin_status` verb** — `DONE`. Runtime-confirmed.
- **Actor extraction (phase 3, ADR-0012)** — `DONE` (`23747a0`). `Mutex<HashMap>`
  → single `run_manager` task (mpsc `Command` + oneshot); lock-free watch-based
  forwarder; pump owns the channel + forwards generation-stamped Commands.
  Behavior-preserving (conn_id ownership kept, no wire change); harness green 5×,
  test files unchanged, two adversarial reviews SOLID. Kills the shared-mutex race
  class + poison-tolerant lock.
- **Slow-subscriber disconnect** — `DONE` (`a70ef74`). All server→client writes
  bounded by a timeout (60s default; `YALDA_SLOW_SUB_TIMEOUT_MS`); stuck peer
  dropped → reconnects + replays; owner never gapped.
- **Headless start-work verb** — `DONE` (`f3585b0`, ADR-0015). `Request::AdminPrompt`
  + `yalda-session-server prompt <sid> <text>` CLI + `SessionServerClient::
  {admin_prompt,connect_existing}`; ungated `enqueue_prompt` core shared with the
  owner-gated `do_prompt`. Headless prompt takes no lease; WAL-durable; runs under
  the session's stored permission mode. Test: `admin_prompt_drives_turn_without_owner`.
- **Cursor reconnect (phase 5, additive)** — `DONE` (`a3650a4`). Optional
  `cursor:(generation,index)` on `Request::Attach`; forwarder tails `[index..]` on
  generation-match+in-range, else full replay (additive; GUI untouched). Test:
  `cursor_reconnect_streams_only_tail`. **GUI cursor-wiring is NEEDS-RUNTIME** (have
  the GUI send its last cursor on reconnect; the transcript reconciler must be
  checked under tail-only streams — GPUI not headless-drivable).
- **Lease ownership (phase 4)** — `DONE` — runtime-verified, merging to master
  (branch `phase4-lease` → `ba12d5d`, 2026-06-08). `owner: conn_id` → `Lease{
  client_id, expires_at: Instant}` + 5s client heartbeat / 15s TTL; dual-clock
  (actor owns monotonic `Instant`, wire carries display-only millis); stable
  per-install `client_id` (`~/.cache/yalda/client_id`, `YALDA_CLIENT_ID` override
  for blue-green candidates); `attach_owner_with_retry`→`attach_for_role`
  (deterministic same-`client_id` reclaim, retry/observer-fallback retired); wire
  `OwnerChanged→LeaseChanged`; WAL 1→2 with **discard** of v1. STAGED, not bundled
  with the eventlog collapse. **Verification:** workflow `wf_c45c440b-aac` (build +
  15/8 headless) → race review found 2 BLOCKING client races (owner-gap after
  promote; observer heartbeat steal/churn) → fixed (unconditional beater +
  per-tick `is_driver` self-gate; `is_driver` persisted on `AgentSlot`) → indep.
  re-review `MINOR`, both closed, found a leaked-beater → fixed (singleton
  `_lease_heartbeat` Task). Final: build clean, **17 + 8 headless pass**. **Runtime-verified
  (2026-06-08):** clean v2 daemon spawn + live v1-WAL discard; heartbeats accepted
  (no `bad frame`); idle-then-prompt holds the lease (no false expiry); `:promote`
  self-hosting handoff textbook (candidate observer-attach → original close →
  candidate promote → drives past >15s — the bug-1 owner-gap fix, confirmed in-app
  via daemon log + user drive). **Known limitation (App Nap):** two windows of the
  *same* install on one Mac — the backgrounded owner's heartbeat (collect step on
  GPUI's foreground executor) is throttled by macOS App Nap, so its lease lapses
  (~15s) and ownership follows focus. Fails safe (no double-drive / corruption).
  Acceptable per user — the self-hosting / blue-green loop is the real case and
  works; same-machine multi-window is the edge. Follow-up only if that matters
  (heartbeat off an App-Nap-immune timer / disable App Nap / longer TTL).
- **`AgentTransport` seam (phase 6)** — `DONE` / merging to master (branch
  `phase6-transport` → `b0375e9` + `1f80296`, 2026-06-08). `AgentTransport` trait
  (object-safe, sync, pump-facing) + `AgentSpawner` factory + `RealAgentSpawner`;
  `FakeTransport`/`FakeAgentControls`/`FakeAgentSpawner` in-process fake (gated
  `feature = "test-support"`); new `tests/agent_transport_fake_test.rs`. Real
  subprocess path byte-identical; crash/WAL/socket/back-pressure tests kept
  subprocess-backed. Workflow `wf_6ead8955-d04` → review `MINOR` (fake's
  `complete_turn` wrongly emitted a `TurnEnded` record the default worker doesn't)
  → fixed: `complete_turn` is counter-only, opt-in `emit_turn_ended_event` covers
  `YALDA_EMIT_TURN_ENDED=1`. Build + full suite + 8/8 fake tests green.
  Behavior-preserving → foldable after build-check. Overlaps
  `tests/session_resilience_test.rs` with phase 4 at integrate (kept additive).
  Unblocks the phase-8 eventlog reducer/forwarder headless tests.
- **GUI projection + full eventlog end-to-end (phase 8)** — `MERGED` to master
  (`f0710fc`, 2026-06-08; v3 WAL cutover landed). Post-merge runtime confirms still
  owed (see end of entry) but non-blocking — headless + reviews are green.
  Producer collapse `Notification::{ReplyEvent,TurnEnded,UserPrompt}` +
  `WorkerEvent::Reply` → one `AgentEvent` (`src/agent_event.rs`, byte-preserving
  `Unknown{tag,raw}`); emit chokepoint (worker stamps gen/turn, server `record()`
  assigns durable seq); generation-on-`ChannelOpened`; WAL 2→3 **discard**;
  **ringbuffer compaction** (`log_base` logical offset, §6 epoch predicate wired
  into phase-5 cursor seq-space, `CompactedSummary` trim marker, on-disk WAL
  append-only); GUI **total reducer** over `AgentEventKind` + idempotent finalize,
  **additive** per §9 (old inference kept behind a gate — deleting it is a
  post-soak follow-up). **Verification:** workflow `wf_73656668-97f` (build + all
  suites green: new `event_log`/`agent_event_stream`/`agent_reducer_*`/ringbuffer
  tests) → adversarial review `BLOCKING` (live forwarder gapped the owner across a
  trim; marker prepend shifted seq +1; finalize ledger keyed 0-vs-1-based so dedup
  never fired) → **fixed** (log_base-aware live forwarder + owner hard-ceiling;
  prepend decrements `log_base`; aligned finalize keys) with fail-before/pass-after
  tests → re-review `SOLID`, found a MAJOR (no high-water bound → an App-Napped
  owner pins in-memory growth) → **fixed** (spec §6 disconnect-before-gap:
  `enforce_high_water` evicts the slowest forwarder — owner included, lease-safe —
  before the trim) → eviction race self-checked clean (immutable `LogSnapshot` +
  evicted-check-first). Final: build clean, **full `--features test-support` suite
  green**. **Runtime check (2026-06-08, isolated v3 sandbox):** replay idempotency
  passed (a daemon-bounce full replay caused NO visible re-render — the reducer
  refolded identical state); WAL v3 reload + re-adopt clean. **Found + FIXED a real
  §9 bug:** a live prompt AFTER a resume stuck in "thinking" forever — `ReplayEnd`'s
  server-stamped envelope `turn` (`self.turns`) aliases the next live turn's
  finalize key (`completed_turn = turns-1`), so routing `ReplayEnd` through the
  per-turn idempotency ledger pre-occupied the live turn's `(gen,turn)` slot →
  live `TurnEnded` no-op'd finalize → `turn_phase` never returned to `Idle`. Fix
  (`e19b9d7`): `ReplayEnd` is a replay-PREFIX marker, routed through a one-shot
  `replay_prefix_finalized` (re-armed on `reset_for_replay`), never taking a
  per-turn slot. Reproduced + fixed headlessly (verify_harness), independently
  re-reviewed `SOLID` (multi-resume re-arm + no-other-aliasing-pair + no §9
  regression confirmed). **Post-merge confirms (NEEDS-RUNTIME, non-blocking):** (1) one-shot GPUI paint confirm —
  the now-`Idle` spinner visibly clears on screen (fold/`turn_phase` proven correct
  headlessly; only the paint is unverifiable without a GPUI run); (2) App-Nap-paused-owner
  high-water eviction → clean reconnect + lease reclaim in the live app; (3) the
  merge is the **v2→v3 WAL cutover** (discards v2 sessions — do at a quiet moment).
  **Deferred follow-ups:** delete the §9 gated old-inference after real-session
  soak; latent `event.seq` vs `seq_of` divergence (only bites when phase-5 cursor
  is wired client-side — commented).
- **GUI stale-session robustness** — `DONE` (`b0f1eb2`) / NEEDS-RUNTIME. GUI drops
  the slot + scrubs the persisted id (by id, across all cwd keys) on a permanent
  `no such session` attach error; transient errors keep the recoverable status.
  Compile-verified; runtime check owed (silent drop, no recur next launch,
  transient survives, last-slot restores underlying, multi-tab/tile).
- **In-app rebuild + reconnect-badge** — `NEEDS-RUNTIME`. `dev_rebuild_restart_gui`
  (`<space> c g`) and the permission badge after a sid-only reconnect (shows
  default until re-synced) need a human runtime check.

## Top priority

- **State-first architecture overhaul** — `PHASE-A-DONE / PHASE-B-GATED`
  (updated 2026-06-08). Root-cause fix for the constant-regression class (30% of
  state is hand-synced caches/copies). Full state→owner map (162 items),
  20-module state-first decomposition, 6 gating decisions, and a phased plan in
  `docs/specs/spec-state-architecture.md` (+ Appendix A inventory).
  - **Phase A (pure extractions) — essentially complete.** Landed: `replay_turns`
    field-ownership (`6168157`), `overlay` 5-Options→`ActiveOverlay` enum
    (`e5be921`), `settings`/text-zoom persist (`e66a54c`), canonical cwd key
    (`c46f023`), `tool_calls`→owner (`f10486e`), `agent_view_model`→owner
    (`9253139`), additive `TurnEnded{generation}` (`8cdbdd1`), server `record()`
    fusion + `apply_channel_state` unify (`74c4f73`), `InputSurface` enum
    (`761dfe6`), dead-code removal (`15fe390`), `reset_for_replay` delegation
    (`eca7759`). Deferred-on-purpose: `buffer_pool` (5a, folded into D2) and
    `DocState` auto-derive (5b, memo half already done).
  - **Decisions D1–D6 — written.** ADRs 0006–0011 cover them (0007 doc/edit rope
    = D2, 0008 reconnect semantics = D3, 0009 durability = D4, 0010 cwd = D5,
    0006/0011 turn-end + crate boundary ≈ D1/D6). No longer a blocker on the user.
  - **Stop-the-bleeding — done:** CI gate ✅, keymap extraction + headless action
    smokes ✅, worksheet double-render ✅, `clippy -D warnings` + `fmt --check`
    quality CI gate ✅ (2026-06-08).
  - **Phase B (behavior-changing, GPUI-runtime-gated) — HELD, by design.** Not
    blocked on a decision; blocked on the **verification harness** (GPUI can't be
    driven headlessly) and on stabilizing the active reconnect path. Status
    (updated 2026-06-08):
    - `5c` Doc/Edit single pooled rope — ✅ **LANDED**. The foundation was already
      live (`DocState.source`/`DocSource`/`SharedEditor`/`open_and_retain` dedup +
      `refresh_blocks`); open/split/restore bind the pooled core, so Doc+Edit and
      splits share a rope with unified undo. Final fix: theme-switch re-render
      (`re_render_one_doc`) sources the live core instead of disk (was silently
      reverting unsaved edits). Headless tests added (pool sharing + unified undo
      + live-core re-render). ⚠️ cross-tile *paint* owes a GPUI eyeball.
    - `8b` delete turn-end inference — ⏸️ **architectural goal already met by the
      phase-8 `AgentEvent` stream** (sourced-once + total reducer + exactly-once
      ledger; agreement pinned by `agent_stream_agrees_*`). The remaining legacy-
      inference deletion is the content-application cutover (double-render risk the
      §9 gate prevents) and would inject `TurnEnded` into the durable WAL — runtime
      +soak-gated, **held by design**, not by an open decision.
    - `10` reconnect — ✅ **decided ADR-0008 scope DONE** (re-attach failures
      surfaced via `spawn_attach_sessions`). The `Arc<Core>` swap-in-place is an
      explicit **ADR-0008 deferral** (HIGH risk, rare path, trigger not fired) —
      a recorded non-goal, not unfinished work.
    - `ChannelAttachState` faithful enum — still held (refactors the live
      reconnect path; stabilize that first).

- **CI gate** — `DONE` (2026-06-08). Minimal `build --bins + test` on push/PR
  (`.github/workflows/ci.yml`) landed; the `quality` job (`clippy -D warnings` +
  `fmt --all --check`) is now enabled too — the whole tree is clippy-clean and
  fmt-clean. Turns the human from the only oracle into the fallback.

- **Verification harness** — `PARTIAL`. The original premise ("agents can't
  drive the GPUI app") is **stale**: `verify_harness.rs` (~40 `#[gpui::test]`s)
  drives the real view headlessly — constructs it, presses real keys, streams
  events through the real reducer, asserts state. The scripted-input driver is
  done. Three gaps remain, in leverage order: (1) **full GUI↔server↔agent loop
  in one process** — wire the GUI's real `SessionServerClient` to an in-process
  fake server+agent (server-side fakes already exist); retires the most
  `NEEDS-RUNTIME` flags. (2) **golden render output** — snapshot the element
  tree / layout bounds from `run_until_parked` for the pixels/geometry class.
  (3) **wall-clock perf gate** — `--release` criterion bench at realistic
  transcript size (render-count proxy is already in CI). See
  `docs/dev-system.md` § Verification harness. `NEEDS-RUNTIME` items below now
  mean "owes a pixels/timing eyeball," not "untestable."

## State (2026-06-02)

`master` fast-forwarded `f282130` → `8036ccf` (= `integration`): base ACP + rail
+ perf + workspaces are now on `master`. Rail is **runtime-confirmed by the
user**; the rest is `NEEDS-RUNTIME`. Follow-ups below are off `integration`,
**not yet folded**.

## Follow-ups (branches off `integration`)

- **`ff-buffer-pool`** — `DONE` (folded into 5c, 2026-06-08). The buffer pool is
  wired into the live app: `open_and_retain` dedups by canonical path and
  `gc_buffers` (strong-count liveness) backs every file-backed view, so docs are
  shared by reference across views (Doc/Edit/splits of one file share a rope +
  unified undo). See ADR-0005 / ADR-0007 and spec §6 step 5c. ⚠️ cross-tile paint
  owes a GPUI runtime eyeball.
- **`ff-ui-threading`** — `DONE` (`c7b138f`). Move `open_agent`/`attach`/`close`
  socket round-trips off the paint thread (tachyon S4); open is now instant.
  Removes the last ~30s freeze path. Behavior-changing → **runtime review before fold**.
- **`ff-editor-perf`** — `DONE` (`42b4507`). Delta-based undo (refactor #4) + O(1)
  LLM insertion-point cache (#9), +10 tests. Behavior-preserving → foldable after build check.
- **`ff-server-perf`** — `DONE` (`7a352ea`). `Arc` event_log snapshots (#6).
  Behavior-preserving → foldable. `#7` (lock sharding) deferred below.

## Ready

- **Fold the perf/cleanup follow-ups into `integration`** after build-check:
  `ff-server-perf` (done), `ff-editor-perf` (when done). Hold the behavior-
  changing ones (`ff-buffer-pool`, `ff-ui-threading`) for runtime review.
- **Retarget `/refactor` to yalda** — `READY`. Its `workflow.js` PHILOSOPHY
  preamble is Fulcrum-specific (Python/PyO3/pytest/EARS). Replace with a Rust /
  GPUI philosophy (Result-typed errors, newtypes for invariants, `#[test]` /
  `debug_assert!` as enforcement hooks, no migration framing).

## Deferred (with reason)

- **`/refactor` net-new findings not yet taken** — see
  `docs/research/refactor-review-perf-hot-path.md`. `#4`/`#9` are being done in
  `ff-editor-perf`; `#6` in `ff-server-perf`. Remaining: nothing critical.
- **Server lock sharding / forwarder-consumes-broadcast (refactor #7)** —
  `DEFERRED` (needs-human). The "event_log is the single source of truth,
  broadcast is only a wake signal" design is load-bearing (fixes `Lagged`
  merge artifacts). Changing it risks ordering/dup regressions. `#6` already
  removed the dominant cost (whole-log clone). Revisit only if profiling shows
  the per-event global lock is a real bottleneck under many sessions.
- **event_log compaction/capping** — `DEFERRED`. Interacts with the resumable-
  tail `sent`-index replay protocol; risky. `Arc` snapshots (`#6`) bought the
  cheap win without it.
- **Tachyon R1/R2 (speculative pre-tokenize, frame-budgeted replay)** —
  `DEFERRED`. Marginal after the memoization (S1) landed; measure before building.
- **tool_calls deep-clone per frame → `Rc<HashMap>`** — `DEFERRED`. Touches
  ~5 mutation sites with `Rc::make_mut`; outside the memoized boundary, so
  orthogonal. Low risk but not yet worth the churn.

## Worksheet frozen-block model (branch `main`, this session)

- **Worksheet insert/render fixes** — `NEEDS-RUNTIME`. Five fixes from a
  4-personality subagent sweep (`docs/projects/worksheet-frozen-blocks/`):
  (1) atomic structural blocks (code/table) can no longer be split by an insert —
  the "butchers Claude text" bug, guarded in `can_insert_char_at` via a new
  `EditorCore::atomic_blocks` seeded from the render-time block detector;
  (2) blank lines are no longer frozen as empty "You" turns on submit;
  (3) the phantom "You" header scan is bounded to the current editable run;
  (4) each frozen prose line is its own nav stop (insert between any two);
  (5) `snap_nav_stop` no longer strands the caret on a block-interior line.
  Builds + 217 gpui tests + full suite green; needs human runtime check (GPUI
  can't run headless).
- **Worksheet deep bugs (deferred)** — `READY`. `001-ticket-deferred-deep-bugs`:
  streaming cursor-drift (cursor not shifted on `programmatic_insert`),
  floor-only-EOF (`agent_tail_floor_char` misses mid-transcript drafts), undo
  wipes `TurnId` metadata, `view_model_fingerprint` excludes cursor/content.
  Real, higher-scope, NOT the reported repro — each needs runtime repro + a
  separately-tested fix.

## Needs decision (you)

- **Workspaces multi-membership for agents** — `NEEDS-DECISION`. Needs the
  multi-subscriber session core/view split (see `spec-workspaces-tagging.md`).
  Bigger lift; confirm it's wanted before building.
- **Merge order to `master`/`main`** — `NEEDS-DECISION`. `integration` is the
  combined buildable branch; none of it is runtime-verified yet.

## Needs runtime verification

All 2026-06-02 branches: `rail-fixes` (placement/contrast/chords), `perf` /
`perf-tachyon` (feels-fast + tokens/tool-expand/thinking-indicator correct),
`workspaces` (Ctrl-W m/M chords, dot, focus after move), `integration` (all of
the above together).
