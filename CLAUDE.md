# Yaldabaoth

Agentic operating system for Scott's life. Yaldabaoth is the Demiurge, the blind
craftsman who spins up a whole hierarchy of archons to run the world beneath him
while remaining serenely unaware there's a higher pleroma he's not party to.

Built in Rust. The surface is a GPUI desktop GUI (`yalda-gpui`), backed by
supporting binaries (`yalda-channel`, `yalda-session-server`). It began life as
a markdown editor; that's now just one App among many.

## Tiles and Apps

The workspace is a tree of **Tiles** (tabs + n-ary splits; see
`docs/specs/spec-tabs-and-splits.md`). Each Tile holds exactly one **App**
(`docs/specs/spec-tiles-and-apps.md`, ADR-0019) — the Demiurge arranges the Apps;
the work happens inside them:

- **`App::Buffer`** — a view onto the shared file-buffer pool, always in exactly
  one `BufferMode`: `Picking` (file/buffer browser), `Viewing` (rendered
  markdown), or `Editing` (raw source). `Viewing ⇄ Editing` toggle over the same
  pooled `SharedCore`; `Picking` is reachable via Cmd+O (Buffer-scoped).
- **`App::Agent`** — an `AgentTile`: a **viewport** bound to (at most) one ACP
  session. `App::Agent` is just the enum tag; the real split is `AgentTile` =
  the viewport/UX (in the layout tree, holds `bound: Option<SessionId>`) vs
  `AgentSession` = the conversation (transcript, channel, tools), owned by the
  `AgentSessions` store on the view (see `spec-agent-session-ownership.md`). The
  store enforces strict **1:1** — a session is bound by at most one tile; a
  session no tile binds is **free** and re-bindable. An unbound tile
  (`bound: None`) renders the **selector** (free sessions + "create new"); close
  / unbind / rebind keep the tile `App::Agent` showing the selector — it never
  vanishes and never silently becomes a Buffer (Agent and Buffer are orthogonal;
  there is no nested `underlying` buffer, and no "leave agent" gesture — an
  agent tile stays an agent tile; you close it or open a Buffer tile normally).
  Agent commands (space / tile menu): select session · stop · send message · switch
  Worksheet⇄Message Box. (Two leaders — ADR-0032: space → verbs on the focused
  App; `.` → verbs on the shell. `?` is retired.)

## Dev system (read this for how we work)

`docs/dev-system.md` is the operating manual: the spec → decision → scaffold →
implement → verify → integrate → log lifecycle, the definition of done, parallel-
work discipline, and the verification-harness plan. Key artifacts:

- `docs/specs/` — design (what). Skill: `/spec`.
- **`docs/components/` — per-component specs: each component (`Workspace`, `Tile`,
  `AgentTile`, `TextEditing`, …) in one place — Description + References + its UX
  invariants `UXI-<Component>-N`.** This is the home for new UX behavior. A new
  behavioral requirement goes here via **`/new-ux`** (capture → interrogate to zero
  ambiguity → check code+specs for prior art → spec as a `UXI` at `target` →
  implement + guard test → reconcile status/deviations). Big components decompose
  into a `<component>/` subdir; shared behavior lives in `docs/components/common/`.
  See `docs/components/README.md`. **Every code change touching a tile / view /
  editor / scroll / caret / input surface MUST be checked against the owning
  component's `UXI` list and MUST NOT violate one** — if it seems to require
  violating one, stop and reconcile the spec first.
- **`docs/ux-invariants.md` — the LEGACY flat `INV-UX-N` contract, being migrated
  into `docs/components/` incrementally.** Still authoritative for any `INV-UX-N`
  not yet migrated (a migrated one carries a `→ migrated to UXI-…` pointer). Don't
  add new invariants here — add them as `UXI-<Component>-N` in the component spec.
- `docs/bugs/` — one file per bug (`bug-<NNNN>-<slug>.md`) with a timestamped log
  of every attempt, indexed by `bug-manifest.md`. Fix bugs via **`/bug`**: check the
  manifest first so we don't repeat a failed approach, then append the actual fix.
- `docs/decisions/` — ADRs (why a path was chosen). Skill: `/decision`.
- `docs/worklog/` + `docs/backlog.md` — what happened / what's open. Skill: `/worklog`.
- `docs/projects/` — **multi-session project tickets** (see below).
- `/integrate` — converge parallel branches into one buildable branch.

### Project planning (`docs/projects/`) — skill: `/plan`

Work that spans multiple sessions (a refactor done in stages, a feature with a
tail of follow-ups) gets a **durable project record** so it survives context
loss. The in-session task list (TaskCreate) is the live mirror; these files are
the record that outlives it. Scaffold and extend them with `/plan`.

```
docs/projects/<project-slug>/
  project.md             # standing context: problem/why, goals, scope, the model, tickets table
  NNN-ticket-<slug>.md   # one actionable task: goal, subtask `- [ ]` checkboxes, verification, links
```

`project.md` is **context, not a task** — the shared understanding every ticket
assumes (root cause, the model, links, a tickets status table). A ticket is one
coherent deliverable with subtasks as checkboxes. Litmus: writing "why / the
model" → `project.md`; writing "do X, then Y" → a ticket. Tick boxes as subtasks
land and keep the session task list in sync; new threads get a new ticket
(`NNN+1`), not scope creep. Live on `main`. Example:
`docs/projects/agent-model-refactor/`.

### Mandatory Cog orchestration

Use Cog as the mandatory execution source of truth for every non-trivial request
to specify, plan, implement, or change product/process behavior. Small, genuinely
single-step edits do not need a graph. The user's request authorizes creation of
the planning graph; ask again only when scope is ambiguous or the plan expands it.

Before editing tracked files:

1. Confirm `cogd` is reachable with `cog graph list`.
2. Create or import a graph and use actor `claude-code` for every mutation.
3. Show the graph id and `cog graph render <id> --frontiers` to the user.

Then claim each ready node before doing its work and close it only after its
acceptance criteria are verified, attaching meaningful JSON output. Heartbeat
long claims. Record cross-cutting decisions and deviations as graph notes, and
node-local facts as node notes. Update the graph before doing newly discovered
work. Re-read graph status, ready nodes, inputs, logs, and notes instead of
relying on conversation memory.

- `/cog-plan <goal>` decomposes approved work into a dependency graph. It is
  distinct from `/plan`, which maintains Yaldabaoth's durable project/ticket
  record under `docs/projects/`; use both when work needs both forms of record.
- `/cog-execute <graph-id>` resumes and drives a graph to completion.
- Claude's task list or prose plan may supplement Cog but cannot replace it.
- An empty ready set is not completion. Claim and close omega, confirm
  `cog graph status <id>` is `complete`, and capture the final frontier render.
- Finish non-trivial work with `/worklog`, validate it using
  `scripts/check-cog-worklog.sh <worklog>`, and include the graph id, final
  status, and render in the handoff.

Never reconstruct a graph after implementation to simulate compliance. If Cog
is unavailable, stop before tracked-file edits and surface the prerequisite. The
user may explicitly opt out of Cog for a particular request.

**Definition of done:** builds + tests + pasted evidence + runtime-checked-or-
flagged + artifacts updated. "Compiles" is not done, and neither is "a green test"
— the test must exercise the REAL path the user's action runs and be observed RED
without the fix (see **"The anti-circling rules"** under Verification harness below;
they are mandatory for every bugfix). Most GUI behavior IS headlessly testable via
`verify_harness.rs` — flag `NEEDS-RUNTIME` only for one of the documented genuine
gaps, naming which.

## Worktree workflow (default)

**Do substantial work in a git worktree, not the main checkout.** Each task /
feature / agent gets its own worktree + branch so the main working dir stays
clean and parallel work can't collide. Place worktrees under
`./.claude/worktrees/` (NOT as siblings of the repo in `~/ws/` — that clutters
the workspace dir). The harness already uses `./.claude/worktrees/` for agent
isolation; task worktrees live there too. `./.claude/worktrees/` is gitignored.

```
git worktree add .claude/worktrees/<task-slug> -b <task-slug>
```

Trivial one-file edits and conversational answers don't need a worktree; new
features, multi-file changes, and anything you'd run agents on do.

**Commit freely — do not ask.** When work is verified (builds + tests + an
observed-RED negative control for any bugfix), commit it, and merge finished
branches to `main`, without asking first. Asking to commit is friction that has
stranded verified work and caused errors — this overrides any default "commit
only when asked" guidance. The quality gate still holds: never commit an
unverified guess (guard RED, or you couldn't localize on the real path — report
instead). **Push** to a remote is the one step that still needs an explicit ask.

## The GUI

`yalda-gpui` is the user-facing surface; all new UX work targets it. The
shared document/editor/render crates live under `src/` (see "Shared crates"
below) and the GUI binary lives under `src/bin/yalda-gpui/`.

`cargo run --bin yalda-gpui [path]` launches it.

### GUI layout

`src/bin/yalda-gpui/` is a module-per-concern split (modules glob-import the
root via `use super::*;` and the root re-exports them with `pub(crate) use`,
so items stay crate-visible regardless of file):

- `main.rs` (~6.5k) — `YaldaGpuiView` struct, the `Render` impl, app/tab/
  split/doc methods, marks/layout-modes/tags, menus + overlays + pickers,
  key bindings + `main()`. A Tile (`Window<App>`) holds one `App`
  (`spec-tiles-and-apps.md`, ADR-0019): `App::Buffer(BufferApp)` —
  `BufferApp::{Picking(file browser), Viewing(rendered doc), Editing(raw)}`
  — or `App::Agent(AgentTile)` (a viewport bound to one session in the
  `AgentSessions` store; see `spec-agent-session-ownership.md`). The render path
  branches on that, each screen with its own `key_context` (`YaldaView`,
  `EditView`, `BrowserView`, `AgentView`) and its own `on_action` wiring.
- `screens.rs` — the screen render bodies: `render_doc`, `render_edit`
  (Code + WP), `render_agent`, `render_browser`.
- `agent.rs` — agent-tile data layer: tool-call model, `FlatItem` view model
  + S1 cache + `rebuild_agent_view_model`, `TurnPhase`, `AgentState`,
  `AgentSession`, `AgentTile`.
- `agent_sessions.rs` — the `SessionStore`/`AgentSessions` owner: the private
  `SessionId → AgentSession` registry that enforces the 1:1 binding invariant
  (`open_or_focus`, `bind_sid`, `locate`, `close`).
- `agent_ui.rs` — agent/session methods on the view: open/attach/create/
  close flows, server pump + reducers (`apply_server_batch`
  / `apply_reply_events` / `apply_agent_event`), submit paths, Claude key
  handler.
- `chrome.rs` — focused-window/layout render, tab strip, tag bar, rails.
- `edit_ui.rs` / `browser_ui.rs` — per-screen methods (edit entry/exit + key
  dispatch; browser nav + rail).
- `render_blocks.rs` — free render helpers for the markdown doc/transcript
  path: colors/fonts, styled-line/block/table elements, wiki links.
- `linear.rs` / `linear_ui.rs` / `linear_view.rs` — `App::Linear`: the Linear
  GraphQL client + data model, the view-layer methods, and the cached body
  component (built on **yux**).
- `diff.rs` / `diff_ui.rs` / `diff_view.rs` + `diff_model.rs` / `diff_git.rs` /
  `review_state.rs` — `App::Diff`: the read-only diff-review tile
  (`docs/specs/spec-diff-review.md`, `docs/components/diff.md`). `diff.rs` = tile
  data model + pure helpers (`merge_gate_decision`, `zed_open_arg`,
  `build_hunk_comment_prompt`, `DiffProjections`); `diff_ui.rs` = view methods
  (bind/refresh/apply derive pipeline, review marks, comment→steering, merge
  gate, open); `diff_view.rs` = the yux cached body (`DiffView`, root-observed).
  `diff_model.rs` = the pure unified-diff parser + `hunk_hash` (the shared
  normalization; also drives the hidden `--hash-diff` subcommand, C6);
  `diff_git.rs` = the async `git` subprocess boundary; `review_state.rs` =
  per-branch reviewed-hash persistence in the git common dir. Merge hook:
  `scripts/yalda-pre-merge-hook`.
- `yux/` — the reusable UX component layer (cached-view infra + view
  primitives). See **"yux" below** and `yux/CLAUDE.md`.
- `persist.rs` — paths, preferences, workspace + ACP-session persistence,
  server launch helpers.
- `workspace.rs` — tab strip + n-ary split tree (`Workspace<C>`,
  `FocusedWindow`, etc.). See `docs/specs/spec-tabs-and-splits.md`.
- `tests.rs` / `verify_harness.rs` — unit tests + headless render harness.

Keep the split honest: new agent-tile logic goes in `agent.rs`/`agent_ui.rs`,
markdown-block render helpers in `render_blocks.rs`, **all reusable UX in
`yux/`** — don't let `main.rs` re-accrete.

### yux — the UX component layer (all UI work goes here)

**`src/bin/yalda-gpui/yux/` is the home for every reusable UX building block,
and all new UX work is built from it.** Read `yux/CLAUDE.md` before touching any
view. It owns two things:

- **Render-skip infrastructure** (`yux/cached.rs`) — `cached_child`, the
  `record_render`/`record_notify` perf counters, `MissReason`. The one lever
  that keeps typing latency O(changed), not O(whole tree).
- **Reusable view primitives** (`yux/detail.rs`) — `DetailStyle` + domain-free
  blocks (`multiline_text`, `kv_row`, `section_heading`, `note_block`,
  `fmt_iso_datetime`) that any read-only detail panel composes from.

The rules (enforced, each maps to a shipped bug): never `cx.notify()` in a
render path; an expensive/stable surface is its own cached view entity embedded
via `cached_child`, self-invalidating at its mutation site; globals (theme/zoom)
are pushed via a `notify_*_views` walk; **state is encapsulated** — a component
owns its UI state, reads (not owns) the global chrome off the root, and is the
only thing that notifies for its state; and **every new cached surface ships a
render-count test**. Reference components: `transcript_view.rs` (`TranscriptView`)
and `linear_view.rs` (`LinearView`). When you build UX, compose from existing
primitives and promote anything reused twice into `yux/detail.rs` — the goal is
reuse and DRY.

### GUI screens

- **Doc view (`YaldaView`)** — rendered markdown, block-by-block. Cursor
  is a left orange bar on the focused block; j/k or arrows move block focus.
  Built from `RenderedBlock`s via `block_element` / `block_inner` /
  `styled_line_element`.
- **Edit view (`EditView`)** — raw markdown editing, two sub-views toggleable
  with Ctrl-W:
  - `EditView::Code` (RAW): monospace, line-number gutter, `md_highlight`
    source colors.
  - `EditView::WordProcessor` (WP): proportional font, per-line typographic
    classification (`classify_wp_line`) for headings/lists/blockquote/code.
- **Browser view (`BrowserView`)** — file picker for `Cmd+O`.
- **Claude session (`ClaudeView`)** — ACP chat panel for the active session.

### GUI key conventions

Per-screen vim-style bindings live with `Some("YaldaView")` etc. contexts.
Global Cmd shortcuts (Quit, OpenBrowser, OpenClaude, tab/split management,
zoom) are registered with `None` context and **must** have a matching
`on_action(Self::handler)` on every screen's root so the dispatch lands.

### Document text zoom

`Cmd-=` / `Cmd-+` zoom in, `Cmd--` zooms out, `Cmd-0` resets. Implementation
is a `text_scale: f32` on `YaldaGpuiView` (clamped `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`,
step `TEXT_SCALE_STEP = 1.1`) that multiplies the body `text_size(px(14.0))`
and every heading size. Threaded into `RenderCtx::text_scale` for block
rendering — for the buffer doc/edit views AND the **agent transcript**
(conversation prose + markdown blocks scale; INV-UX-13). **Chrome stays fixed** —
status bars, tab strip, browser rows, the agent gutter/labels + bottom panels,
and the pixel-pinned compose input all render at their native sizes. To extend
the zoom to a new surface, multiply that surface's base `text_size` by
`self.text_scale` and add `on_action(Self::zoom_in/out/reset)` to its root; for a
cached surface (the transcript) read `text_scale` off the root and invalidate via
`notify_transcript_views`.

### GUI responsiveness invariants (read before touching agent/transcript UI)

The agent transcript is a **cached child entity** (`transcript_view.rs`,
`TranscriptView`), not inline render. This is load-bearing for typing latency —
see `docs/projects/gpui-responsiveness/` (`project.md` has the 6 verified GPUI
0.2.2 facts + the component model). The rules that keep it fast and correct:

- **Never call `cx.notify()` inside a `render()`/`build_body` path.** A notify
  issued mid-draw is *parked* (no effect that frame, no scheduled redraw) — the
  rev-1 stale-tail bug. Notify from event handlers, `cx.observe` callbacks,
  timers, or `cx.defer` only. Pinned by `cached_notify_from_render_is_parked`.
- **Every input `TranscriptView::render` reads must have a monotonic seq in
  `TranscriptSeqs`** (the `cx.observe` slice filter). Add a render input
  without a covering seq ⇒ stale UI (the caret-glyph / stall-clock class of
  bugs). Global inputs (theme, zoom) are pushed via `notify_transcript_views`
  from their action handlers, not via a seq. Add an input → add its seq → add a
  `transcript_021_*` regression test in `verify_harness.rs`.
- **Embed cached children via `cached_child(view)`** (carries `size_full`);
  never hand-roll the `.cached()` call (a sizeless style collapses the panel).
- **Tests must never touch `~/.yalda`.** `acp_session_persist_path` /
  `preferences_path` / `workspace_persist_path` return `None` (or a tempdir
  override) under `cfg(test)`. A test that triggers `set_theme`,
  `set_text_scale`, or `save_workspace_state` must NOT write the user's real
  state — if you add a new persisted path, give it the same `*_PATH_OVERRIDE`
  seam.

### Verification harness — the testing protocol (DEFAULT to it; do NOT call things "runtime-only")

`verify_harness.rs` (`cargo test --bin yalda-gpui`) drives the **real**
`YaldaGpuiView` headlessly via `#[gpui::test]` + `TestAppContext`. It is far more
capable than "state asserts" — **before flagging any GUI behavior as
human-runtime-only, check it against this list; most things are headless.** Every
behavior/UX change ships a headless guard here (and an `INV-UX-N` if it's a UX
invariant). Background + the 3 genuine gaps: `docs/dev-system.md` § Verification harness.

**What the harness CAN verify headlessly — reach for these seams:**

- **Real view + real actions.** `boot_with_transcript` / `install_agent_slot` /
  `boot_browser` construct the production view with a bound agent session;
  `run_until_parked()` runs real layout/paint. Read state back via
  `read_session(id, cx, |c| …)` / `agent_read`; mutate via `with_session`.
- **Real keystrokes + bindings.** `cx.update(register_keymap)` then
  `vcx.simulate_keystrokes("escape")` exercises the *actual* keymap + on_key_down
  dispatch (e.g. `esc_interrupts_in_flight_turn`, `cmd_b_toggles_file_browser_rail`).
- **Synthetic agent stream through the REAL reducer.** Build
  `session_proto::Notification::ReplyEvent { session_id, event: ReplyEvent::… }`
  batches and apply with `v.apply_server_batch(batch, cx)` (or `apply_reply_events`).
  This covers transcript **ordering, dedup/echo-suppression, turn accounting,
  tool-call rendering** — NO live agent needed (e.g. `steering_midturn_ordering_and_dedup`).
- **PAINTED geometry.** Wrap an element in `probe_bounds("tag", el)`, then in a test:
  `layout_probe_begin()` → force a frame → `layout_probe_get("tag")` returns the
  painted `(x,y,w,h)` → `layout_probe_end()`. Assert real placement/visibility
  (e.g. `subagent_panes_paint_above_the_compose`,
  `compose_caret_row_painted_inside_box_when_wrapped`). Caret-below-fold,
  panel-collapse, element-order bugs are all catchable here.
- **Render-count perf proxy.** `perf_reset/perf_render_count` assert O(changed)
  (typing on surface A leaves cached surface B's render count flat). Every new
  cached surface ships one (`transcript_021_*`).
- **State-machine fuzzer + invariant oracle** (property-based; the strongest net
  for interaction-sequence regressions). `agent_tile_statemachine_fuzz_holds_invariants`
  drives the real view through deterministic-random op streams (type, toggle
  worksheet, submit, stream events, stop, spawn subagent, …) and after EVERY op
  runs `assert_agent_invariants` — one oracle re-checking the whole contract
  (caret-in-range / INV-UX-1, append-only frozen transcript / INV-ORDER,
  `stop_requested⇒awaiting`, focus validity, no-panic). A seeded LCG (no
  wall-clock/RNG) makes any failure reproduce by `seed`/`step`. When you add a
  new agent-tile operation or invariant, add it to the op list / the oracle —
  this catches what example tests can't. (Validated with a negative control: a
  one-line injected caret corruption fires the oracle with the exact seed/step.)

**The 3 genuine gaps (the only legitimate `NEEDS-RUNTIME`):**

1. **Pixels/colors beyond bounds geometry** — the probe gives layout rects, not a
   rendered bitmap; exact glyphs/theme colors need a human eye.
2. **The live subprocess worker / full GUI↔server↔agent loop** — `apply_server_batch`
   feeds the reducer directly because `sent` can't be true with no daemon. To
   verify the real `AcpChannelClient` worker against the real agent, write an
   `#[ignore]` integration test in `tests/` (e.g. `tests/steering_midturn_live.rs`,
   run with `--ignored`) — needs the agent on PATH + auth.
3. **Wall-clock perf as a gate** — render *count* is a proxy; real timing is a
   human `sample` under `--release` (debug masks wins). `benches/render_bench.rs` exists.

**Protocol for a GUI change:** pick the seam (reducer for stream behavior, layout
probe for placement/visibility, `simulate_keystrokes` for bindings, render-count
for perf), add the guard, and only write "human runtime check" for one of the 3
gaps above — with which gap, explicitly. Tests must never touch `~/.yalda`
(`*_PATH_OVERRIDE` / `None` under `cfg(test)`).

### The anti-circling rules — READ BEFORE calling ANY bugfix "done"

Each of these cost a multi-round, user-enraging failure where green tests coexisted
with a broken app. A test that doesn't exercise the code the user's action actually
runs is WORSE than no test — it manufactures false confidence and sends us in circles.

1. **Drive the REAL entry point, not a hand-built proxy state.** If the bug is "after
   `/clear` I can't type," the test must call the method the user's action invokes
   (`clear_agent_session` / `apply_open_agent_resolution`) — NOT hand-call
   `settle_input_focus` and assert the state "looks right." FIVE "/clear typeable"
   fixes passed because each asserted a *simulated* post-clear state; the real async
   bind path was never run, so the real state (resting in nav) was never seen. Find
   the method the click/keystroke actually calls, and call THAT.
2. **Negative control is mandatory — observe the guard RED with the fix reverted.**
   Toggle the fix off, run the test, watch it fail *for the right reason*, restore. A
   test that passes both ways guards nothing. One `cargo test` run; skipping it has
   cost days. (Do it inline: comment out the fix line, `cargo test <name>`, restore.)
3. **Assert on PAINT/behavior, not just state.** "the char is in the compose buffer"
   ≠ "the user sees it." For visibility/render bugs use the layout probe
   (`probe_bounds` / `layout_probe_*`) and assert the caret/text painted INSIDE the
   viewport (and make it NON-vacuous: assert the content is bigger than the viewport
   so a fit isn't a false pass). A state assert cannot catch a repaint miss.
4. **Keystrokes: `simulate_keystrokes` is focus-accurate but NOT OS-accurate.** It
   fabricates the ideal `Keystroke`, so it CANNOT catch OS-mangled chords. macOS eats
   `Ctrl`+digit and `Ctrl-Tab` — a passing `simulate_keystrokes("ctrl-3")` proves
   nothing about the real key. This is a 4th genuine gap. Prefer `Cmd`-based bindings
   (the app's `cmd-*` are delivered reliably); treat `Ctrl`+digit / `Ctrl-Tab` as
   unreliable on macOS. Boot the screen the user is actually on (agent, not browser).
5. **The fix must be on `main` AND in the running binary.** Fixes stranded on feature
   branches (the mid-turn-`m` fix sat unmerged on `jump-pane-nav`) never reach the
   user, who runs `main` via `./dev-gui.sh` (release). "Tests pass on the branch" is
   not shipped. Not done until green ON `main` + the binary rebuilt + the user
   restarted. Check for stranded work before assuming a fix landed.
6. **Mutation-test the changed predicate** (`cargo mutants`, config `mutants.toml`;
   CI `mutation-gate` runs `--in-diff`). A surviving mutant = the test doesn't
   constrain the code.

If you cannot make a test fail without the fix ON THE REAL PATH, you have NOT
localized the bug. Say so; do NOT ship the guess. (Full post-mortem of the saga that
produced these rules: `docs/dev-system.md` § negative-control + the anti-circling set.)

## Naming Conventions

### Modes

Two top-level modes:

- **View Mode** — rendered markdown display (read-only navigation)
- **Edit Mode** — raw markdown source editing, with two submodes:
  - **Normal** — vim-style navigation and commands
  - **Insert** — text input

In code, `ViewMode::Rendered` corresponds to View Mode, and `ViewMode::Raw` corresponds to Edit Mode. `AppMode::Normal` and `AppMode::Insert` are the Edit Mode submodes.

## Shared crates

The document/editor/render layer under `src/` (consumed by `yalda-gpui` and
the supporting binaries):

- `document.rs` — text buffer backed by ropey rope
- `render.rs` — markdown-to-rendered-blocks conversion (pulldown-cmark)
- `editor.rs` — editing operations over the document
- `keybind.rs` — key binding definitions and sequence matching
- `keys.rs` / `style.rs` — frontend-neutral key + styling primitives
- `command.rs` — command registry (`:` commands)
- `md_highlight.rs` — syntax highlighting for edit mode
- `theme.rs` — color themes
- `blocks.rs` — rendered block types (Heading, Paragraph, Table, etc.)
