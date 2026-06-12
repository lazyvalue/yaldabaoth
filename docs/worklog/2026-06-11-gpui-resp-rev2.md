# Worklog — 2026-06-11 — gpui-responsiveness REV-2 (capstone verification)

Branch: `gpui-resp-rev2` (worktree `.claude/worktrees/gpui-resp-rev2`). NOT merged
to `main`. This entry is the capstone verification of the rev-2 invalidation
refactor (tickets 024 → 025 → 021). See
`docs/projects/gpui-responsiveness/project.md` for the load-bearing model (the six
gpui 0.2.2 facts + the component model).

## What landed

Rev-2 replaces rev-1's fingerprint-polled `CachedPanel` (the per-frame poll from
inside `render()` that violated the timing law, fact 4) with the framework-native
model: **state in entities, invalidation by observation, `cached()` as a thin
render-skip wrapper.**

- **024 — `cached_panel` rework** (`70852cc`). Deleted the fingerprint layer
  (`FingerprintedPanel` / `notify_if_changed` / parent-held `last_fp`). What
  survives is a thin `cached_child` embed helper that bakes in the `size_full`
  sizing guard (fact 2 — a cached slot is sized from its style or it collapses)
  and the `YALDA_PERF` render/cache counters (hit/miss with miss reason).
  Headless timing-law tests added in `verify_harness.rs`.
  Files: `cached_panel.rs`, `verify_harness.rs`.

- **025 — `Entity<AgentSession>` hoist** (`eeb1497`). Per-session domain state
  moved into `Entity<AgentSession>`; `SessionStore<P>` made payload-generic so
  the 1:1 sid-binding invariant is untouched by the payload swap. Mutators now run
  `session.update(cx, |s, cx| { …; cx.notify() })` — notify at the mutation site
  (timing-correct, fact 4), per-entity granularity for free. No behavior change.
  Files: `agent.rs`, `agent_ui.rs`, `chrome.rs`, `main.rs`, `screens.rs`,
  `verify_harness.rs`.

- **021 — `TranscriptView` entity** (`8963e41`, hardened by `1ddcdba`). The
  flagship. The transcript is now its own view entity embedded in `render_agent`
  via `cached_child`. It holds a `session` handle + `cx.observe(&session)`; the
  observe callback compares the slice version watermark (the seqs its render
  reads: `edit_seq`, frozen-gen, tools-gen, compose mode, anim tick) against
  `last_rendered` and self-notifies ONLY when its slice moved, logging the reason
  for the notify counter. Its `render()` NEVER calls `cx.notify()`: cache
  mutations it performs (S1 view-model, highlight) go through `session.update`
  WITHOUT an inner notify (a plain mutation, not an invalidation). The
  adversarial-review follow-up (`1ddcdba`) widened the watermark to cover compose
  `mode` + anim-tick seqs so a mode flip / cursor-blink can't strand a stale
  cached transcript. Result: a compose-box keystroke leaves every observed seq
  stable ⇒ no self-notify ⇒ transcript render-skip.
  Files: `agent.rs`, `agent_ui.rs`, `main.rs`, `screens.rs` (765 lines of render
  body moved out), `tests.rs`, `transcript_view.rs` (new), `verify_harness.rs`.

## Test evidence

Commands run in the worktree (`.claude/worktrees/gpui-resp-rev2`):

- `cargo build --bin yalda-gpui` → **GREEN** (6 warnings, all pre-existing
  dead-code in `workspace.rs`; nothing from the rev-2 work).
- `cargo test` (full workspace) → **522 passed, 0 failed** (2 + 1 ignored).
  Aggregated across every suite (lib 134, yalda-gpui bin 185, session-server 2,
  plus the integration suites: editor 52, cursor 15, keys 23, config 18,
  session_transcript 14, session_resilience 9, render 14, file_browser 12,
  menu 10, keybind 10, document 9, yalda_channel 5, tree 4, snapshot 3, parse 3,
  channel/server unit 0).

Flake note (not a rev-2 regression): on one full-suite run `yalda_channel_test`
reported `4 passed; 1 failed`. It passes in isolation (`cargo test --test
yalda_channel_test` → 5/5) and on the immediately following full run (0 failures).
This is the known timing-sensitive subprocess integration test, unrelated to the
render/cache work. Re-runs were clean (522/0).

## Timing-law audit (fact 4 — the rev-1 bug)

Grepped the whole `src/bin/yalda-gpui` crate for `.notify()` inside the
brace-balanced body of every `render*()` function (a script, not eyeballing).
Five textual hits, all benign:

- `screens.rs:45` — a comment, not a call.
- `main.rs:6016`, `main.rs:6023` (`render_splash`) — inside `cx.listener` key-down
  / mouse-down event-handler closures (run at event time, phase == None).
- `main.rs:6210` (root `render`) — inside a `capture_key_down(cx.listener(…))`
  closure (event time).
- `verify_harness.rs:2840` — the **deliberate** `SelfNotifier` test fixture: an
  intentionally-illegal notify-from-render that the timing-law pin test uses to
  assert gpui still parks a mid-draw notify (the loud signal if a future gpui
  upgrade changes fact 4). Not production code.

`transcript_view.rs` render body: **zero notify**. Its only notifies are line 152
(inside the `cx.observe` callback — the canonical outside-draw cache-busting path,
fact 5) and line 642 (inside an event-handler closure). Confirmed: **ZERO
production `cx.notify()` issued during a draw.** The timing law holds.

## HUMAN RUNTIME CHECKLIST (still gates integration — cannot self-verify headless)

The GPUI app cannot be driven headlessly; these must be checked on the live
process before this branch folds to `main`:

1. **No per-keystroke transcript layout.** `sample` the live process while typing
   in a large transcript (open an agent tile with a long conversation, hold a key
   in the compose box). The transcript subtree must NOT re-layout per keystroke —
   confirm no `TranscriptView::render` / list-layout frames in the sample while
   typing (the cached child should render-skip).
2. **Streaming-tail freshness (the rev-1 stale-tail hazard).** Stream a long agent
   reply and confirm the FINAL append of the turn appears WITHOUT a mouse wiggle /
   unrelated event. This is the exact failure rev-1's fingerprint poll could
   strand indefinitely; rev-2 must not.
3. **Cursor blink** — caret still blinks in the compose box and edit view (timer
   notifies its owning view; must not be starved by the cached transcript).
4. **Selection parity** — text selection in the transcript and edit view behaves
   as before the entity hoist (listeners captured in the cached prepaint act on
   current state).
5. **Resize** — resizing the window / tiles re-lays-out the transcript correctly
   (cached slot bounds change ⇒ cache miss ⇒ fresh render).
6. **Multi-tile splits** — typing / streaming in one tile does not re-lay-out an
   unrelated tile's transcript (the core mega-entity symptom this refactor targets).
7. **Follow-tail** — auto-scroll-to-bottom while streaming still tracks the tail,
   and disengages on manual scroll-up as before.
8. **Session close / rebind** — closing a tile, unbinding, and rebinding a free
   session to a tile keeps the transcript correct (no stale view bound to a closed
   session; the `Entity<AgentSession>` handle resolves to the right conversation).

Also still owed from Phase 0/1: the runtime copy/paste check (clipboard in-process,
ticket 011).
