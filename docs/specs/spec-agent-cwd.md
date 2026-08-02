# Per-Session Agent CWD

**Status:** DRAFT.

**Last updated:** 2026-06-18

> **Workspace cwd is now a required, typed field (ADR-0023).** The directory a
> new agent session inherits (`agent_base_cwd`) is the active workspace's cwd,
> held as `Tab.cwd: WorkspaceCwd` — a required, private field, not a stringly
> `kv["cwd"]`. A workspace cannot be constructed without one (build via
> `Tab::with_layout`), so "no cwd → silently use the process dir" is
> unrepresentable; ephemeral virtual workspaces (ADR-0021) inherit the spawning
> workspace's cwd. This closed the cwd-inheritance regression where a new
> `Tab` literal omitted the cwd key.

## Builds On

- **`spec-agent-window.md`** — Defines `AgentWindow`, `AgentRing`, `AgentSlot`, and `AgentState`. Its revision history parks "per-window cwd" as a sibling spec; this is that spec. The `AgentSlot` data model gains one field; the Status Strip layout (§30) gains one field; the persisted slot JSON shape (§35) gains one field. Nothing about the worksheet/chatbox contract, sidebars, or input modes is affected.
- **`spec-multi-session.md`** — Defines the session lifecycle (`session/new`, `session/load`, detach, attach). This spec relies on detach + fresh-attach as the mechanism for changing a live session's cwd, since a running ACP subprocess cannot move cwd. The "re-attach starts fresh" contract from §4 of that spec carries straight over.
- **`spec-workspaces-and-splits.md`** — Path canonicalization rule (Constraint §11) is reused: per-slot cwds canonicalize with `std::fs::canonicalize` when the directory exists, falling back to `cwd.join(path)` with `.`/`..` collapsed when it doesn't. Persistence of the workspace tree (Behaviors 23–24) is unaffected — `workspace.json` keeps storing only session ids in its `Claude` leaves; per-slot cwds live in `acp_sessions.json` next to the session id.
- **`src/acp_channel.rs`** — `AcpChannelClient::spawn_with_resume_in` already takes `cwd: Option<PathBuf>`, but today that value is **only** forwarded to the agent over the wire as `NewSessionRequest::new(cwd)` (`acp_channel.rs:1098, 1117`); it is **not** applied to the spawned OS process. `tokio::process::Command::new(&parts[0])` at `acp_channel.rs:733` never calls `.current_dir(...)`, so the agent subprocess inherits yalda's process cwd regardless of the argument. This spec **fixes that**: the worker must call `cmd.current_dir(&cwd)` on the `tokio::process::Command` builder before spawn so the OS-level cwd matches the protocol-level cwd. Without that, any agent tool call that reads the OS cwd (Bash `pwd` / `ls .` / a subprocess spawned with a relative path) keeps resolving against yalda's process cwd, which would make `:claude-new <path>` and `:claude-cd <path>` misleading half-features.

## Overview

Today every agent session yalda spawns runs at the process's `cwd` — whatever directory the user launched yalda from. Sessions that were created before the user ran `cd` (or that resume from a persisted state where the project layout has shifted) still inherit today's process cwd. There is no per-session control.

This spec gives each `AgentSlot` its own `cwd: PathBuf`. The slot's `cwd` is the directory the subprocess runs in and the directory the agent's tool calls resolve relative to. The user picks it when creating a session (`:claude-new <path>`) and can change it later (`:claude-cd <path>`, which detaches and respawns). Defaults preserve today's behavior — `:claude-new` with no argument uses the process cwd.

The spec introduces no new artifacts. It extends three existing ones and fixes one channel-layer bug:

- **`AgentSlot`** gains one field: `cwd: PathBuf`.
- **Status Strip** gains one field: shortened slot cwd.
- **`acp_sessions.json` persisted slot record** gains one field: `cwd` (string, absolute).
- **`acp_channel.rs::run_worker`** gains one line: `cmd.current_dir(&cwd)` on the `tokio::process::Command` builder before spawn (the missing piece that today silently ignores the `cwd` argument at the OS level).

## Behaviors

### Lifecycle

1. **Creation default. [DRAFT]** When a slot is created without an explicit cwd — bootstrap (the first slot when yalda opens Claude), bare `:claude-new`, restoring a persisted slot that has no `cwd` field — the slot's `cwd` is `std::env::current_dir()` resolved at the moment of creation. If `current_dir()` itself fails (rare; pwd gone), the fallback is `PathBuf::from("/")` — the same fallback `AcpChannelClient::try_spawn` already uses (`src/acp_channel.rs:411`).

2. **Creation with explicit cwd. [DRAFT]** `:claude-new <path>` parses `<path>` and uses it as the new slot's cwd. The path may be absolute or relative; relative paths resolve against the process cwd (not against any other slot's cwd). Resolution sequence:
    1. **Tilde expansion.** A leading `~` or `~/` is expanded to `$HOME` (read from the environment). A literal `~` followed by a username (`~alice/...`) is **not** supported and is left unchanged — yalda is single-user and the userdir-lookup path would add a dependency for one degenerate case.
    2. **Canonicalization** per `spec-workspaces-and-splits.md` Constraint §11: `std::fs::canonicalize` when the path exists, fall back to `process_cwd.join(path)` with `.`/`..` collapsed when it doesn't.
    3. **Validation:** the resolved path must exist and be a directory; otherwise the command no-ops with footer hint `not a directory: <path>` and no slot is created. A nonexistent argument is a typo, not a feature.

3. **Subprocess spawn. [DRAFT]** `create_agent_session` is parametrized on the slot's cwd. The thread that calls `AcpChannelClient::spawn_with_resume_in` passes `Some(slot_cwd)` instead of `std::env::current_dir().ok()`. Downstream in `acp_channel.rs` the worker is amended per "Builds On" — `cmd.current_dir(&cwd)` is set on the `tokio::process::Command` builder before spawn so the OS-level subprocess cwd matches the `NewSessionRequest` cwd. The two were silently divergent before this spec.

4. **Changing a live slot's cwd. [DRAFT]** `:claude-cd <path>` operates on the active slot. Steps:
    1. Resolve `<path>` per §2's rules; if invalid, footer hint and no-op.
    2. Drop the slot's `channel: AcpChannelClient` (kills the subprocess via `kill_on_drop`). Clear `attach_pending`, `awaiting_reply`, `turn_started`.
    3. Set `slot.cwd = resolved_path`.
    4. Set `slot.resume_id = None`. (The agent's session-side state was tied to the old cwd; resuming it under a new cwd would mislead the agent. The respawn uses `session/new`.)
    5. Spawn a new `AcpChannelClient` with the new cwd via the same attach-thread / pump machinery used by today's `:claude-attach`. The pump's `wake_rx` is re-taken from the new channel on the next idle wait (the existing `wake_rx.is_none()` re-acquisition path at `main.rs:6125-6149` covers this naturally — no new pump code).
    6. Save the ring (§5).

    The transcript editor is **not** wiped. Prior frozen lines remain visible as the session's history; the new subprocess does not know about them. The footer logs `claude-1: cwd → /Users/scott/ws/foo, fresh session` (the explicit "fresh session" wording is how the user reconciles the visible history with the agent's amnesia in v1). A future refinement may insert a visible `── new session at <path> ──` divider line in the transcript — that requires a new `TurnId::Divider` variant and Worksheet-gutter rendering work, deliberately deferred to keep this spec focused on the cwd plumbing.

5. **Persistence. [DRAFT]** Per-slot cwd is part of every ring snapshot. Whenever the ring saves (per `spec-multi-session.md` §15: on every ring mutation), the per-slot record includes `cwd` as an absolute string. Loader reads `cwd`; absence defaults per §1.

### Display

6. **Status Strip location row. [IMPLEMENTED]** The Status Strip
   (`spec-agent-window.md` §30) ends with a dedicated location row. A linked Git
   worktree renders as `WORKTREE <directory-name>`; any other location renders as
   `CWD <shortened-path>`. The `CWD` prefix is bold.

    ```
     CWD ~/ws/yalda
    ```

    The displayed string is the shortened form of `slot.cwd`:

    - If the cwd starts with `$HOME`, the prefix is replaced with `~`.
    - If the resulting string is longer than 32 chars, the middle is elided with `…` so the leading two and trailing two path segments remain (e.g., `~/ws/some-very-long-project-name/…/src/components`).
    - The location row is always visible, including when the cwd matches the process cwd.

    Hovering the cwd field shows the full absolute path. Clicking the cwd field has no effect in v1 (`:claude-cd` is the only path to change it).

7. **Sidebar disclosure. [DRAFT]** The session sidebar (`spec-multi-session.md` §9) gains a hover tooltip on each label showing the full cwd. The label text itself does not change shape — keeping the sidebar visually compact remains a goal. If two slots have identical labels and different cwds, the user disambiguates via the tooltip or the active-slot Status Strip.

### Restore

8. **cwd restored from persistence. [DRAFT]** On launch, `load_persisted_acp_sessions` populates each slot's cwd from the JSON `cwd` field. If the field is absent (old-format save, or a save written before this spec lands), the slot defaults per §1.

9. **cwd missing at restore. [DRAFT]** If the persisted cwd no longer exists at restore time (directory deleted, drive unmounted, project moved), the spawn itself fails: with `cmd.current_dir(&cwd)` set per §3, POSIX raises `ENOENT` at fork time. That error flows through the existing failed-attach path — `try_spawn` returns `Err`, the attach-thread sends it through the `attach_pending` channel, and the pump translates it to `slot.channel = None` with footer hint `claude-1: cwd no longer exists: <path>` (using the same status-line mechanism that today surfaces "no ACP agent on PATH"). The slot remains in the ring with the transcript visible and `channel: None`; the user can `:claude-cd <other-path>` to recover. The persisted `cwd` field is **not** rewritten on disk — the user may have temporarily detached the drive; preserving the original path means the next restore in the right environment works.

10. **Downgrade compatibility. [DRAFT]** Older yalda binaries reading a newly-written `acp_sessions.json` deserialize each slot record with serde's "ignore unknown fields" behavior. `cwd` is silently dropped; the downgraded slot spawns at process cwd. No persisted session is lost — same downgrade shape as `spec-agent-window.md` §35.

## Data Model

### AgentSlot extension

```rust
struct AgentSlot {
    label: String,
    index: usize,
    state: AgentState,
    has_unseen_activity: bool,
    resume_id: Option<String>,
    cwd: PathBuf,  // NEW — absolute path; defaults to process cwd at slot creation.
}
```

`PathBuf` (not `Option<PathBuf>`) because a slot always has a cwd — even bootstrapped slots have a definite cwd (`std::env::current_dir()` at creation). Optionality only exists in the JSON wire format.

### Persisted shape

```json
{
  "/Users/scott/ws/yalda": [
    {
      "id": "ses_abc123",
      "label": "claude-1",
      "active": true,
      "mode": "worksheet",
      "tasklist_open": true,
      "subagents_open": false,
      "cwd": "/Users/scott/ws/yalda"
    },
    {
      "id": "ses_def456",
      "label": "refactor-foo",
      "mode": "chatbox",
      "cwd": "/Users/scott/ws/foo"
    }
  ]
}
```

Top-level shape (cwd-keyed object → list of slot records) is unchanged. Each slot record gains the optional `cwd` field. Loader treats absence per §1.

## Interfaces

### Commands

| Command | Argument | Effect |
|---|---|---|
| `:claude-new` | `[path]` | Create a new slot. With `path`, validate per §2 and use that cwd. Without, default per §1. Menu chord unchanged (`Space c n`); the menu form has no argument, so the menu always uses the default cwd. |
| `:claude-cd` | `<path>` | Change the active slot's cwd per §4. No menu chord in v1 — this is a deliberate command, not a casual gesture. |

The `cwd`-argument form of `:claude-new` is parsed by the existing command parser; the path token is everything after the command name (already supported by the shell-words split applied to command lines).

### AgentRing API

`AgentRing::push` gains a fourth argument: `cwd: PathBuf`. The new signature is `push(&mut self, label: String, state: AgentState, resume_id: Option<String>, cwd: PathBuf) -> usize`. The `cwd` argument lands directly in the constructed `AgentSlot`'s new `cwd` field (`main.rs:3133-3145`). Call sites: `open_agent_inner` (bootstrap; passes the resolved cwd it used when spawning each session), `new_agent_session` (with no path: process cwd at the moment of the call; with path: the resolved per §2), restore (per-slot `cwd` from persistence with §1's fallback).

A new helper on `AgentRing` for the `claude-cd` path:

```rust
/// Replace the active slot's channel with a fresh one rooted at `new_cwd`.
/// The transcript editor stays. Returns the previous cwd for the footer hint.
fn change_active_cwd(
    &mut self,
    new_cwd: PathBuf,
    channel: AcpChannelClient,
    attach_pending: Option<...>,
) -> PathBuf;
```

Caller responsibilities: build the new channel (or attach-thread) before invoking, since channel construction blocks on the ACP handshake and must happen off the GPUI foreground.

### Persistence functions

- `save_persisted_acp_sessions(cwd, ring)` — extended to emit each slot's `cwd` field. Cwd is serialized as a string via `PathBuf::display().to_string()` (lossy on non-UTF8, matching the top-level key's serialization).
- `load_persisted_acp_sessions(cwd) -> Vec<PersistedSlot>` — extended to read `cwd` and store it on `PersistedSlot`. Missing field deserializes to `None`; the loader resolves it to process cwd per §1 when building the live `AgentSlot`.

```rust
struct PersistedSlot {
    id: String,
    label: String,
    active: bool,
    mode: InputMode,
    tasklist_open: bool,
    subagents_open: bool,
    cwd: Option<PathBuf>,  // NEW — None = default to process cwd at restore.
}
```

### Status Strip rendering

The cwd field renders per the field-order rule in §6 (after any sub-agent breadcrumb, before the model id). The render function takes the slot's `cwd` and the process cwd as inputs; if they match, it produces an empty fragment. If they differ, it produces the shortened display string and registers a hover tooltip with the absolute path.

## Constraints

1. **One cwd per slot, fixed for the slot's lifetime by default.** `:claude-cd` is the only way to change it, and it costs the conversation (fresh `session/new`). No mid-conversation cwd drift. This matches the subprocess model — the agent process literally has one cwd.

2. **Resolution at command time, not display time.** `:claude-new ./foo` resolves `./foo` against the process cwd **at the moment the command runs**. If the user later runs `cd` in their shell (which doesn't reach yalda anyway) or yalda's own process cwd changes, the slot's absolute cwd is unaffected. Yalda never reinterprets a stored cwd as relative.

3. **No cwd validation on restore.** A persisted cwd whose directory no longer exists is loaded as-is and passed to the subprocess. With `cmd.current_dir(&cwd)` set, the spawn fails at fork time with `ENOENT`; the failed-attach path surfaces a footer hint per §9. Yalda does not crash, does not rewrite the persisted file, and does not silently swap to process cwd. The user diagnoses and either fixes the directory or runs `:claude-cd`.

4. **`workspace.json` is not extended.** The workspace persistence file stores Claude leaves by `session_id` only (`spec-workspaces-and-splits.md` Behavior 23). The cwd lives next to the session id in `acp_sessions.json`, not in `workspace.json`. Two files-of-truth would risk drift. The loader resolves the cwd at `session/load` time by reading `acp_sessions.json`.

5. **Persistence is keyed by yalda's process cwd, not by the slot's cwd.** `save_persisted_acp_sessions(cwd, ring)` at `main.rs:1878` keys the JSON object by `std::env::current_dir()` at save time. A slot whose `cwd = /Users/scott/ws/foo` created while yalda ran from `/Users/scott/ws/yalda` is saved under the top-level `/Users/scott/ws/yalda` key. Launching yalda from `/Users/scott/ws/foo` does **not** restore that slot — the key is the workspace, the slot's `cwd` is its tool-execution root, and the two are independent. This matches the mental model "a yalda workspace is one project; its agents may operate on adjacent directories." Cross-workspace agent restore is out of scope.

6. **Browser-window cwd is unrelated.** `BrowserWindow.fb.current_dir()` is the file browser's working directory, not the agent's. They may diverge — a user can browse `/foo` in one tile while their active agent operates on `/bar`. There is no link between the two in v1; a future "spawn agent at browser dir" affordance can land later without changing this spec.

7. **No multi-agent shared cwd.** Two agent slots with the same cwd are perfectly fine and have no shared state (`spec-multi-session.md` Constraint §1). The cwd is just a directory path, not a coordination primitive.

8. **TUI is unchanged.** This spec covers the GPUI frontend only. The TUI's agent integration continues to spawn at process cwd. Per-session cwd in the TUI is a future spec if needed.

9. **No directory picker UI in v1.** Setting cwd requires typing a path into a command. A `Cmd+O`-style picker for `:claude-new` is parking-lot — adding it later is a UI-only change that doesn't touch this spec's data model.

10. **`:claude-cd` to the slot's existing cwd is still a respawn.** The command does not no-op when the new cwd equals the current cwd; it always tears down and respawns. This makes the command a useful "reset this slot" affordance even when the user just wants a clean subprocess at the same path. (Alternative: optimize to no-op when cwd matches. Rejected — the reset value is more useful than the optimization.)

11. **Non-UTF8 paths round-trip lossily through persistence.** Cwd is serialized via `PathBuf::display().to_string()` for symmetry with the top-level `acp_sessions.json` key, which is already lossy on non-UTF8 paths. On macOS this is rare to non-existent (HFS+/APFS enforce UTF8-encodable names); flagged because the spec is explicit about the choice and a future migration to `Path` byte-level serialization is the only fix.

## Revision History

- 2026-05-22 (2) — Adversarial review pass. Blocking fix: §"Builds On" and §3 corrected — today's `acp_channel.rs` worker never calls `cmd.current_dir(...)` on the `tokio::process::Command` builder, so the `cwd` argument was only flowing as a NewSessionRequest hint while the OS-level subprocess inherited yalda's process cwd; spec now mandates the missing line. §2 path resolution gained an explicit tilde-expansion step (input form matches the §6 display form). §4 (`:claude-cd`) gained a session-divider transcript marker so the user sees where old subprocess knowledge ends; also documented the pump's `wake_rx` re-acquisition path as no-new-code. §9 (cwd missing at restore) rewritten to flow through the spawn-error path (`ENOENT` at fork time) rather than vaguely through tool-call errors. §6 (Status Strip) field order pinned down relative to the sub-agent breadcrumb. `AgentRing::push` signature change spelled out (`push` gains a `cwd: PathBuf` argument). New Constraint §5 making process-cwd-keyed persistence explicit. New Constraint §11 noting `display().to_string()` lossiness on non-UTF8 paths.
- 2026-05-22 — Initial DRAFT. Sibling spec to `spec-agent-window.md`. One field on `AgentSlot`, one field in the persisted JSON, one field in the Status Strip, two commands (`:claude-new <path>` extension and new `:claude-cd <path>`).
