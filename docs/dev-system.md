# Yalda dev system

How we build yalda with agents. This is the operating manual: the artifacts,
the lifecycle that connects them, the definition of done, and how parallel
agent work converges. Read this before starting substantial work.

## The lifecycle

Work flows through stages; the artifacts are the handoffs between them.

```
spec ──▶ decision ──▶ scaffold ──▶ implement ──▶ verify ──▶ integrate ──▶ log
(what)   (why)        (worktree)   (Cog graph)   (gate)     (merge)       (worklog)
```

- **spec** — `docs/specs/spec-<topic>.md`. The design: what we're building, the
  shared vocabulary, the constraints. Use `/spec`. A spec is a design proposal;
  once behavior ships, the **component spec** (`docs/components/`, `UXI-<Component>-N`)
  is the standing truth and the spec is a linked reference. Every change touching a
  tile / view / editor / scroll / caret / input surface MUST NOT violate a universal
  pattern (`docs/ux-patterns/`, `UXP-N`) or the owning component's `UXI`s; new UX
  goes through `/new-ux`. A PreToolUse hook reminds on UX-bearing files.
- **decision** — `docs/decisions/NNNN-<slug>.md` (ADR). The *path*: options
  considered, what we chose, why, and what we gave up. Use `/decision`. Specs
  say "what"; ADRs say "we chose Y over X because Z." Without this the *why*
  evaporates between sessions and gets relitigated.
- **scaffold** — a git worktree under `.claude/worktrees/<slug>` on its own
  branch (see ADR-0001). Substantial / multi-file / agent-run work gets one.
- **implement** — driven by a Cog graph (`/cog-plan` → `/cog-execute`): claim a
  node, do it, close it with output. Directly or via subagents; see "Parallel work".
- **verify** — the gate (see "Definition of done"). The GUI **is** drivable
  headlessly now (`#[gpui::test]` + `TestAppContext`: construct the real view,
  press real keys, stream events, assert state — see "Verification harness").
  What's still manual is narrower: painted pixels/geometry, the full
  GUI↔server↔agent loop in one process, and wall-clock perf. Closing those is
  the highest-leverage remaining investment.
- **integrate** — merge feature branches into one buildable branch in
  dependency order, resolving conflicts. Use `/integrate`. Behavior-changing
  branches are flagged for human review before folding, not auto-merged.
- **log** — a short `docs/worklog/` entry pointing at the complete graph. Use
  `/worklog`. `docs/backlog.md` gets an entry only if Scott must act.

## Artifacts: where things live

| Artifact | Path | Holds | Written with |
|---|---|---|---|
| Execution plan + record | Cog graph (`cog graph …`) | nodes, outputs, notes, deviations | `/cog-plan`, `/cog-execute` |
| Deferred / unscheduled ideas | Cog bulletin at `yaldabaoth/<area>::<leaf>` | "recorded so it isn't re-derived" | `cog bulletin` |
| Specs | `docs/specs/spec-*.md` | design proposals (what) | `/spec` |
| **Component specs** | **`docs/components/`** | **per-component behavior, `UXI-<Component>-N`** | **`/new-ux`** |
| **UX patterns** | **`docs/ux-patterns/`** | **universal laws, `UXP-N`** | **`/new-ux`** |
| Decisions (ADR) | `docs/decisions/NNNN-*.md` | rationale (why) | `/decision` |
| Bugs | `docs/bugs/bug-NNNN-*.md` + `bug-manifest.md` | every fix attempt, timestamped | `/bug` |
| Worklog | `docs/worklog/YYYY-MM-DD-*.md` | shipped / caveats / graph id | `/worklog` |
| Backlog | `docs/backlog.md` | only `NEEDS-RUNTIME` / `NEEDS-DECISION` for Scott | `/worklog` |
| Reference | `docs/reference/` | durable technical facts (e.g. `gpui-render-model.md`) | manually |
| Research/review | `docs/research/*.md` | analysis snapshots | `/refactor`, etc. |
| Durable gotchas | agent memory (`~/.claude/.../memory/`) | non-obvious lessons | as discovered |

## Definition of done (the gate)

A branch is **done** when:

1. **Builds** — `cargo build --bin yalda-gpui --bin yalda-session-server`, no new errors.
2. **Tests pass** — `cargo test --bin yalda-gpui` and `cargo test --lib`, green, with new tests for new behavior.
3. **Evidence pasted** — the agent shows the actual command output, not a claim. Claims get independently re-verified (agents are confidently wrong sometimes).
4. **Runtime-checked OR explicitly flagged** — either exercised against the running app (a headless `#[gpui::test]` driving the real view counts), or the report states exactly what a human must run. `NEEDS-RUNTIME` means a human must confirm *pixels / timing / OS-behavior* — not "no test was possible"; state-level behavior is testable headlessly.
5. **Artifacts updated** — component spec / ADR touched if behavior or design changed; Cog graph `complete`; worklog written.

"Compiles" is not done. "Tests pass" is not done if the change is a UX/perf change that only a runtime check can confirm — say so.

## Parallel work discipline

Fanning out N agents is not free — they have to converge.

- **Decompose by ownership boundary (file/module), not by concern**, when
  concerns overlap in code. Lesson from 2026-06-02: three perf agents split by
  *concern* (event-loop / threads / render) all landed in the same hot path and
  needed a manual synthesis pass. Had they been split by *file ownership* they'd
  have merged trivially.
- **When overlap is unavoidable, plan a synthesis/integration step** up front —
  don't discover it at merge time.
- **Verify every agent's "it's green" yourself** — cheap rebuild on the branch.
- **Behavior-changing branches are flagged, not auto-folded.** Behavior-
  preserving perf/cleanup can fold after a build check; anything that changes
  interaction or output waits for human runtime review.
- **Vary the SURFACE, not just the lens.** Lesson from 2026-06-02: a perf
  fan-out (workflow + /refactor + tachyon, ~dozens of agents) all missed a
  textbook O(document)-per-keystroke bug in the Edit view, because every prompt
  inherited the *reported symptom's* framing ("slows down once an agent session
  runs") and aimed every agent at the agent-transcript path. Diverse lenses over
  identical scope = one search run N times, with a shared blind spot. So:
  - At least one pass per audit must be **invariant-driven, not symptom-driven**
    — "audit EVERY <surface> for invariant <Y>", deliberately *not* anchored to
    the reported symptom (e.g. "every render/input path must be O(changed)").
  - The verification harness is the empirical backstop that doesn't care about
    framing — it catches what a misframed prompt can't.

## Verification harness (state-level: solved; three gaps remain)

The original framing — "agents can't confirm runtime behavior, so the human is
the oracle for every change" — is **no longer accurate**. `verify_harness.rs`
(~40 `#[gpui::test]`s on `TestAppContext`) drives the **real** `YaldaGpuiView`
headlessly: it constructs the production view, installs the production keymap,
simulates real keystrokes (`cmd_b_toggles_file_browser_rail`,
`edit_view_keystroke_is_o_changed`), streams synthetic agent events through the
real reducer, and asserts post-action state through entity handles —
`run_until_parked` runs a real layout/paint pass. "Drive the view, press keys,
assert state" is **done**. Before flagging any GUI behavior as human-only, check it
against these seams — most things are headless:

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
  (caret-in-range / UXI-TextEditing-1, append-only frozen transcript / INV-ORDER,
  `stop_requested⇒awaiting`, focus validity, no-panic). A seeded LCG (no
  wall-clock/RNG) makes any failure reproduce by `seed`/`step`. When you add a
  new agent-tile operation or invariant, add it to the op list / the oracle —
  this catches what example tests can't. (Validated with a negative control: a
  one-line injected caret corruption fires the oracle with the exact seed/step.)

**Protocol for a GUI change:** pick the seam (reducer for stream behavior, layout
probe for placement/visibility, `simulate_keystrokes` for bindings, render-count
for perf), add the guard, and write "human runtime check" only for one of the
genuine gaps below — naming which.

### The guard MUST fail without the fix (negative control) — non-negotiable

A guard test only proves something if it is **observed RED before the fix and
GREEN after**. A test that passes with *and* without the change guards nothing —
it manufactures false confidence, which is worse than no test. This is the exact
mechanism behind repeated "fixed it" claims that didn't fix it: the guard
asserted a state (`focus == Compose`, `you_block_open == true`) that was already
true on the passing path, or was reachable by a *different* input (e.g. a later
`i` keystroke re-opened the block the async replay had closed), so the test went
green while the reported symptom survived untouched.

Protocol for any bug fix (not just UX):
1. Write the guard. **Run it against `HEAD` (no fix) and watch it FAIL.** If it
   passes, the guard doesn't cover the bug — the reproduction is wrong, or a
   later step in the test masks the defect. Fix the guard until it's red for the
   *right reason*, then write the code fix.
2. Reproduce the symptom the user actually reports, on the path they actually
   drive. "The state predicate is wrong after operation X" is necessary but not
   sufficient — if the on-screen symptom is "typing doesn't repaint," the guard
   must exercise typing and assert the repaint (`perf_render_count` bump), not
   just the predicate. If pressing a key in the test masks the difference,
   that's a signal the predicate may be *latent* (real but not the cause).
3. If you cannot make a guard fail without the fix, you have **not localized the
   bug** — say so explicitly and keep digging or ask for the exact repro. Do NOT
   ship the change as "the fix." A plausible-looking state correction with a
   green-both-ways test is precisely the false-fix trap.

Toggling the fix off to confirm the guard flips red costs one `cargo test` run.
Skipping it has cost multiple false "done" reports. Always pay the one run.

### The anti-circling rules — read before calling ANY bugfix "done"

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
localized the bug. Say so; do NOT ship the guess. (Saga that produced these rules:
`docs/bugs/saga-clear-worksheet-invisible/`.)

**Mechanically enforced (you can't rely on remembering the above):**

1. **Mutation gate** — `mutants.toml` scopes cargo-mutants to the input-routing
   predicates (`agent.rs`/`agent_ui.rs`/`main.rs`); the `mutation-gate` CI job runs
   `cargo mutants --in-diff` on every PR, so any *changed* line those tests don't
   actually constrain (a surviving mutant) fails the build. Local full run:
   `cargo mutants --features test-support`. This is the machine version of the
   negative-control rule — it caught that `try_start_mark_chord` /
   `focused_in_insert_mode` / `inline_you_block_active` had ~zero real coverage.
2. **The feature must be ON in CI** — `scripts/ci.sh` runs `cargo test --features
   test-support`. The real-loop harness (`AcpChannelClient::test_connected`, the
   seam that reaches genuine mid-turn state) is behind `test-support`; without the
   flag those guards compile out and gate nothing.
3. **Fixes live on `main`, not a branch** — the recurring "you said you fixed it"
   root cause was fixes committed to feature branches (e.g. the mid-turn-`m` fix
   stranded on `jump-pane-nav`) and never merged; the user runs `main`. A fix isn't
   done until its guard is green *on main* (CI runs on push to main). Check for
   stranded work with `git for-each-ref --format='%(refname:short)' refs/heads |
   while read b; do echo "$b: $(git rev-list --count main..$b)"; done`.

What exists to build on:
- The headless GPUI harness (`verify_harness.rs`, `cargo test --bin yalda-gpui`).
- Server-side fakes (`FakeTransport`/`FakeAgentSpawner`, phase 6) +
  `tests/session_resilience_test.rs` driving the **real** server binary.
- Render-count instrumentation (`record_render`, read via `perf_render_count`)
  as an O(changed) proxy, gated in CI.
- `YALDA_PERF=1` / `YALDA_HL_CACHE` → render/pump timing + the `perf_report` bench.

The genuine gaps (the only legitimate `NEEDS-RUNTIME`; plus anti-circling rule 4 —
OS-mangled key chords):
1. **Pixels / geometry.** The harness asserts state, not what's painted —
   "spinner clears," "panel didn't collapse," "right color after theme switch"
   need a human eyeball. Close it with golden output: snapshot the element
   tree / computed layout bounds from `run_until_parked` (the high-leverage 80%),
   or offscreen-render + hash regions.
2. **The full GUI↔server↔agent loop in one process.** Seam tests note `sent`
   can never be true headlessly (no daemon, no channel), so they drive the
   dedup core directly + add a negative control. The server half already has
   in-process fakes; the missing wire is the GUI's real `SessionServerClient`
   against an in-process fake server+agent, so submit→stream→reduce→render runs
   for real. This retires the largest batch of `NEEDS-RUNTIME` flags.
3. **Wall-clock perf as a gate.** PARTIALLY CLOSED (2026-06-25). `benches/render_bench.rs`
   (criterion) measures the lib render + highlight hot paths over a realistic doc,
   optimized — `cargo bench --bench render_bench`. The measurement + criterion's
   regression report exist; wiring a CI `--save-baseline`/`--baseline` threshold to
   fail on regression is the remaining step.

Recommended order: (2) in-process loop first (retires the most flags, the seam
already exists server-side) → (1) element-tree snapshots → (3) perf gate.

## Skills

| Skill | Use |
|---|---|
| `/cog-plan`, `/cog-execute` | plan work as a Cog graph; drive it to `complete` |
| `/spec` | draft/revise a design spec |
| `/new-ux` | new behavioral requirement → `UXI` (or `UXP`) → implement → reconcile |
| `/bug` | fix a bug with memory of prior attempts |
| `/decision` | record a design decision as an ADR |
| `/worklog` | end-of-work: short worklog entry + backlog pruning |
| `/integrate` | merge feature branches into one buildable branch |
| `/refactor` | multi-lens design review |
| `/responsiveness-audit` | symptom-agnostic whole-surface sweep for UI-responsiveness violations |
| `/code-review`, `/simplify` | built-ins |

## See also

- `CLAUDE.md` — the hard rules (Cog, worktrees, restarts, done) and pointers.
- `src/bin/yalda-gpui/CLAUDE.md` — GUI module layout, perf contract, key conventions.
- `docs/components/README.md` — component-spec format + the index of every surface.
- `docs/ux-patterns/README.md` — the universal UX laws.
