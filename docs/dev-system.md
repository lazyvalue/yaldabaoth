# Yalda dev system

How we build yalda with agents. This is the operating manual: the artifacts,
the lifecycle that connects them, the definition of done, and how parallel
agent work converges. Read this before starting substantial work.

## The lifecycle

Work flows through stages; the artifacts are the handoffs between them.

```
spec ──▶ decision ──▶ scaffold ──▶ implement ──▶ verify ──▶ integrate ──▶ log
(what)   (why)        (worktree)   (agents)      (gate)     (merge)       (worklog+backlog)
```

- **spec** — `docs/specs/spec-<topic>.md`. The design: what we're building, the
  shared vocabulary, the constraints. Use `/spec` to draft/revise. A spec
  describes the *target*, not the path taken.
- **decision** — `docs/decisions/NNNN-<slug>.md` (ADR). The *path*: options
  considered, what we chose, why, and what we gave up. Use `/decision`. Specs
  say "what"; ADRs say "we chose Y over X because Z." Without this the *why*
  evaporates between sessions and gets relitigated.
- **scaffold** — a git worktree under `.claude/worktrees/<slug>` on its own
  branch (see ADR-0001). Substantial / multi-file / agent-run work gets one.
- **implement** — directly or via subagents. See "Parallel work" below.
- **verify** — the gate (see "Definition of done"). The GUI **is** drivable
  headlessly now (`#[gpui::test]` + `TestAppContext`: construct the real view,
  press real keys, stream events, assert state — see "Verification harness").
  What's still manual is narrower: painted pixels/geometry, the full
  GUI↔server↔agent loop in one process, and wall-clock perf. Closing those is
  the highest-leverage remaining investment.
- **integrate** — merge feature branches into one buildable branch in
  dependency order, resolving conflicts. Use `/integrate`. Behavior-changing
  branches are flagged for human review before folding, not auto-merged.
- **log** — `docs/worklog/` entry + `docs/backlog.md` update. Use `/worklog`.

## Artifacts: where things live

| Artifact | Path | Tense | Written with |
|---|---|---|---|
| Specs | `docs/specs/spec-*.md` | future (design) | `/spec` |
| Decisions (ADR) | `docs/decisions/NNNN-*.md` | past (rationale) | `/decision` |
| Worklog | `docs/worklog/YYYY-MM-DD-*.md` | past (what happened) | `/worklog` |
| Backlog | `docs/backlog.md` | future (what's open) | `/worklog`, manually |
| Research/review | `docs/research/*.md` | analysis snapshots | `/refactor`, etc. |
| Durable gotchas | agent memory (`~/.claude/.../memory/`) | invariants | as discovered |

## Definition of done (the gate)

A branch is **done** when:

1. **Builds** — `cargo build --bin yalda-gpui --bin yalda-session-server`, no new errors.
2. **Tests pass** — `cargo test --bin yalda-gpui` and `cargo test --lib`, green, with new tests for new behavior.
3. **Evidence pasted** — the agent shows the actual command output, not a claim. Claims get independently re-verified (agents are confidently wrong sometimes).
4. **Runtime-checked OR explicitly flagged** — either exercised against the running app (a headless `#[gpui::test]` driving the real view counts), or the report states exactly what a human must run. `NEEDS-RUNTIME` means a human must confirm *pixels / timing / OS-behavior* — not "no test was possible"; state-level behavior is testable headlessly.
5. **Artifacts updated** — spec/decision touched if design changed; worklog + backlog updated at session end.

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
assert state" is **done**. The scripted-input driver below already exists.

What exists to build on:
- The headless GPUI harness (`verify_harness.rs`, `cargo test --bin yalda-gpui`).
- Server-side fakes (`FakeTransport`/`FakeAgentSpawner`, phase 6) +
  `tests/session_resilience_test.rs` driving the **real** server binary.
- Render-count instrumentation (`record_render`, read via `perf_render_count`)
  as an O(changed) proxy, gated in CI.
- `YALDA_PERF=1` / `YALDA_HL_CACHE` → render/pump timing + the `perf_report` bench.

The three remaining gaps (what `NEEDS-RUNTIME` actually means now):
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
3. **Wall-clock perf as a gate.** Render *count* is in CI, but it's a proxy and
   debug masks wins — real latency is still a human `sample --release`. Close it
   with a `--release` criterion bench over a realistic transcript as a threshold
   gate (not small-N).

Recommended order: (2) in-process loop first (retires the most flags, the seam
already exists server-side) → (1) element-tree snapshots → (3) perf gate.

## Skills

| Skill | Use |
|---|---|
| `/spec` | draft/revise a design spec |
| `/decision` | record a design decision as an ADR |
| `/worklog` | end-of-session: write the worklog entry + update backlog |
| `/integrate` | merge feature branches into one buildable branch |
| `/refactor` | multi-lens design review (⚠ retarget its Fulcrum preamble — see backlog) |
| `/responsiveness-audit` | symptom-agnostic whole-surface sweep for UI-responsiveness invariant violations ("the tachyon reviewer") |
| `/verify`, `/code-review`, `/simplify` | built-ins |

## See also

- `CLAUDE.md` — code-level conventions, GUI layout, key bindings, worktree rule.
- `docs/decisions/` — why the system is the way it is.
- `docs/backlog.md` — what's open right now.
