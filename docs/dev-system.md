# Sketch dev system

How we build sketch with agents. This is the operating manual: the artifacts,
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
- **verify** — the gate (see "Definition of done"). The weak link today: the
  GUI can't be driven headlessly, so most verification is still manual. See
  "Verification harness" — closing this is the highest-leverage investment.
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

1. **Builds** — `cargo build --bin sketch-gpui --bin sketch-session-server`, no new errors.
2. **Tests pass** — `cargo test --bin sketch-gpui` and `cargo test --lib`, green, with new tests for new behavior.
3. **Evidence pasted** — the agent shows the actual command output, not a claim. Claims get independently re-verified (agents are confidently wrong sometimes).
4. **Runtime-checked OR explicitly flagged** — either exercised against the running app, or the report states exactly what a human must run (see harness gap).
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

## Verification harness (the top gap)

The binding constraint on throughput is that **agents can't confirm runtime
behavior** — so the human is the verification oracle for every change, and
parallelism just defers work to review. Closing this compounds everything else.

What exists to build on:
- Snapshot tests (`tests/snapshots/`, `cargo test --lib`).
- `SKETCH_DEBUG=1` → per-frame ground-truth JSON log (TUI; see CLAUDE.md).
- `SKETCH_PERF=1` / `SKETCH_HL_CACHE` → render/pump timing + the `perf_report` bench.

What to build (backlog item #1):
- A headless/scripted render mode + golden screenshots for the GPUI surface.
- A perf benchmark over a realistic transcript size, run as a gate (not small-N).
- A scripted-input driver so "Cmd-B opens the rail" / "tokens stream in order"
  become tests, not manual checks.

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
