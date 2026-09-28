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

## Dev system

`docs/dev-system.md` is the operating manual — lifecycle, artifacts, definition of
done, parallel-work discipline, and the full verification-harness protocol. Where
things live:

- **Cog** — every plan and its execution record (graphs); deferred ideas (bulletins
  at `yaldabaoth/<area>::<leaf>`, never on a graph). See below.
- `docs/ux-patterns/` — **universal UX laws** (`UXP-N`) that bind every surface.
- `docs/components/` — **per-component behavior** (`UXI-<Component>-N`); shared
  parts in `common/`. New behavior goes here via **`/new-ux`**. **Every change
  touching a tile / view / editor / scroll / caret / input surface MUST NOT violate
  a `UXP` or the owning component's `UXI`s** — if it seems to require it, stop and
  reconcile the spec first.
- `docs/specs/` — design proposals (`/spec`); the component spec is the standing
  truth once shipped.
- `docs/decisions/` — ADRs (`/decision`).
- `docs/bugs/` — one file per bug + `bug-manifest.md`; fix via **`/bug`** (check the
  manifest first so a failed approach isn't repeated).
- `docs/worklog/` — short per-work entry pointing at the graph (`/worklog`).
- `docs/backlog.md` — only what's waiting on Scott (`NEEDS-RUNTIME` / `NEEDS-DECISION`).
- `/integrate` — converge parallel branches into one buildable branch.

## Mandatory Cog orchestration

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

- `/cog-plan <goal>` decomposes approved work into a dependency graph.
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

## Definition of done

Builds + tests + pasted evidence + runtime-checked-or-flagged + artifacts updated.
"Compiles" is not done, and neither is "a green test" — the test must exercise the
REAL path the user's action runs. Most GUI behavior IS headless-testable via
`verify_harness.rs` (`cargo test --bin yalda-gpui`; seams and protocol in
`docs/dev-system.md` § Verification harness). `NEEDS-RUNTIME` is legitimate only
for a named genuine gap: (1) pixels/colors beyond layout bounds, (2) the live
GUI↔server↔agent loop, (3) wall-clock perf, (4) OS-mangled key chords.

### The anti-circling rules — mandatory for every bugfix

Full text and history: `docs/dev-system.md` § Verification harness.

1. **Drive the REAL entry point** — call the method the user's click/keystroke
   invokes, not a hand-built proxy state.
2. **Negative control** — observe the guard RED with the fix reverted, for the
   right reason. Green-both-ways guards nothing.
3. **Assert on paint/behavior, not just state** — layout probe, non-vacuous.
4. **`simulate_keystrokes` is not OS-accurate** — prefer `Cmd` bindings; macOS eats
   `Ctrl`+digit / `Ctrl-Tab`.
5. **The fix must be on `main` and in the built binary** — check for stranded
   branches.
6. **Mutation-test the changed predicate** (`cargo mutants`, `mutants.toml`).

If you cannot make a test fail without the fix ON THE REAL PATH, you have NOT
localized the bug. Say so; do NOT ship the guess.

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

### NEVER restart Yalda or its session server without explicit permission

**Agents MUST NOT restart, quit, kill, relaunch, or replace either the running
`yalda-gpui` process or `yalda-session-server` unless Scott explicitly approves
that specific restart in the current conversation.** This includes direct shell
commands (`dev-gui.sh`, `dev-server.sh`, `pkill`, `kill`, `systemctl restart`),
the in-app Restart / Rebuild & Restart actions, and any indirect operation whose
activation stops or replaces a running process. A request to fix, build, test,
commit, merge, or "ship" a change is **not** permission to restart anything.

Before asking for restart permission, perform a read-only live-session check and
state exactly what will be restarted, how many sessions are attached or running,
that every attached session will disconnect, and that in-flight work may be lost.
Approval is one-shot and applies only to the named process(es). If live-session
state cannot be determined, treat sessions as live and do not restart.

The default activation boundary is **build only**. Build the required release
binary, report that the running process has not been touched, and leave activation
to Scott. Never treat WAL replay, provider `session/load`, supervision, or prior
successful recovery as a safety guarantee: persisted transcript recovery is not
the same as preserving a live process or its in-flight work, and recovery can be
partial. **No agent may restart the session server merely to pick up a change or
restart the GUI merely to demonstrate a fix.** This prohibition overrides any
instruction elsewhere in this repository that says to restart as part of the
definition of done; mark activation `NEEDS-RUNTIME` and hand the exact command to
Scott instead.

## The GUI

`yalda-gpui` (`src/bin/yalda-gpui/`) is the user-facing surface; all new UX work
targets it. `cargo run --bin yalda-gpui [path]` launches it (but see the restart
rule above). **`src/bin/yalda-gpui/CLAUDE.md`** holds the module layout, the
performance contract, key conventions, and test-isolation rules; **`yux/CLAUDE.md`**
the reusable UX component layer — read both before touching a view. Keep the split
honest: agent-tile logic in `agent.rs`/`agent_ui.rs`, markdown render helpers in
`render_blocks.rs`, **all reusable UX in `yux/`** — don't let `main.rs` re-accrete.

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
