# Multiple Claude Agent Sessions

**Status:** SHIPPED.

**Last updated:** 2026-05-14

## Builds On

- **ACP channel** (`src/acp_channel.rs`): Provides `AcpChannelClient`, which owns a subprocess, prompt/reply channels, turn counter, and permission mode. This spec relies on the fact that multiple `AcpChannelClient` instances can coexist — each owns isolated state and an independent subprocess. The multi-session model creates one `AcpChannelClient` per session.
- **ClaudeState** (`src/bin/sketch-gpui.rs`): The current single-session state holder (editor, channel, tool calls, compose box, turn timer). This spec factors `ClaudeState` into a per-session object and adds a session-list layer above it.
- **Session persistence** (`save_persisted_acp_session` / `load_persisted_acp_session`): Currently stores one session ID per cwd. This spec extends persistence to a list of session IDs keyed by cwd, each tagged with a label.
- **Compose textbox** (`spec-textbox-compose.md`): The compose box lives inside `ClaudeState`. Multi-session inherits this — each session has its own compose box state. Only the active session's compose box is rendered.

## Overview

Sketch currently supports one Claude agent session at a time. Opening a second session requires clearing the first (`claude-clear`) or rebooting (`claude-reboot`). This limits workflows where the user wants to run parallel agents — one for a refactor, another for tests, a third for research — without losing conversational context.

This spec describes how multiple concurrent ACP sessions coexist in the GPUI frontend, how the user creates, switches between, and manages them, and how the screen and rendering adapt.

The feature introduces four named artifacts:

1. **SessionSlot** — a named wrapper around `ClaudeState` that adds a user-facing label and a slot index.
2. **SessionRing** — an ordered collection of `SessionSlot`s with one active slot, providing ring-style next/prev navigation.
3. **Session sidebar** — a vertical panel on the left edge of the Claude screen listing all sessions, with the active one highlighted.
4. **Session commands** — menu entries and keybindings for creating, switching, closing, and renaming sessions.

## Behaviors

### Session lifecycle

1. **Create.** [SHIPPED] A new session is created via the `claude-new` command (menu: `Space c n`). This spawns a fresh `AcpChannelClient` with `session/new` semantics (no resume), creates a new `ClaudeState`, wraps it in a `SessionSlot` with the label `"claude-{N}"` (where N is a monotonic counter), and appends it to the `SessionRing`. The new session becomes active immediately. If the user has never opened Claude, `open-claude` (the existing command) creates the first session as today.

2. **Switch.** [SHIPPED] The user cycles between sessions via `claude-next` / `claude-prev` (menu: `Space c ]` / `Space c [`), direct `Ctrl-]` / `Ctrl-[` keybindings (intercepted in `handle_claude_key` before mode dispatch), or by clicking a session label in the sidebar. Switching is instant — no subprocess work, just swapping which `SessionSlot` the renderer reads from. The inactive session's pump task continues running in the background; replies accumulate in its editor buffer.

3. **Close.** [SHIPPED] `claude-close` (menu: `Space c x`) closes the active session. The `AcpChannelClient` is dropped (subprocess killed via `kill_on_drop`), the `SessionSlot` is removed from the ring, and the next session in the ring becomes active. If the last session is closed, the screen returns to the underlying doc/browser screen (same as `back_to_doc` today). Closing does not persist the session — it is gone.

4. **Detach.** [SHIPPED] `claude-detach` on a session drops its `AcpChannelClient` (killing the subprocess) but keeps the `SessionSlot` alive with `channel: None`. The session's chat history remains scrollable. Any in-flight attach is cancelled (`attach_pending` cleared). `awaiting_reply` and `turn_started` are reset so the footer doesn't show a stale "…" indicator. The user can re-attach later with `claude-attach`, which spawns a new `AcpChannelClient` with `session/new` (fresh context) and clears `resume_id` so persistence captures the new id once it binds. Re-attaching to a detached session does NOT resume the previous conversation — the subprocess is gone. For true resume, the session must never have been detached (the persisted session ID is used on sketch restart).

5. **Rename.** [SHIPPED] `claude-rename` opens a centered single-line input overlay pre-filled with the current label. Enter commits, Esc cancels. The overlay targets the slot by its monotonic `SessionSlot::index`, so a concurrent `claude-close` on another slot doesn't rename the wrong one. An empty/whitespace-only input cancels (acts like Esc) so the user can't accidentally erase the label by hammering Enter. Labels are cosmetic — they affect only the sidebar display.

6. **Implicit first session.** [SHIPPED] `open-claude` behaves as today if no Claude screen is active — it creates the first `SessionSlot` and a `SessionRing` around it. If the Claude screen is already active and the user runs `open-claude` again, it adds a new session to the ring (delegates to `claude-new`).

### Background pumping

7. **All sessions pump concurrently.** [SHIPPED] Each `SessionSlot` has its own GPUI `Task` running the 16ms pump loop. Replies from inactive sessions accumulate in their respective `ClaudeState.editor` buffers. Tool call state updates. Turn counters advance. The only thing that doesn't happen is rendering — only the active session is painted.

8. **Wake coalescing.** [SHIPPED] Each session's pump task independently calls `cx.notify()` when events arrive. Since only one session is rendered at a time, wakes from inactive sessions trigger a no-op repaint (the renderer reads the active session, which hasn't changed). This is acceptable at 60Hz with a handful of sessions. If profiling shows excessive repaints, inactive sessions can suppress `cx.notify()` and batch a single wake when the user switches to them.

### Rendering

9. **Session sidebar.** [SHIPPED] When more than one session exists, a vertical sidebar renders on the left edge of the Claude screen. Each entry is a single row showing the session label and a status indicator:
   - Active session: highlighted background (accent color), bold label.
   - Inactive session with activity (new content since last viewed): label with a dot prefix (`● refactor`).
   - Inactive session idle: dim label.
   - Detached session: label with `[detached]` suffix.

   The sidebar has a fixed width (20 chars, including 1-char left padding). Labels longer than the available width are truncated with `…`. The sidebar is vertically scrollable if sessions exceed the viewport height — the active session is always scrolled into view.

   With one session, the sidebar is hidden (no visual change from today's single-session UI). A `+` button at the bottom of the sidebar creates a new session (equivalent to `claude-new`).

   Layout:

   ```
   +----------+-------------------------------+
   | header (full width)                      |
   +----------+-------------------------------+
   | sidebar  | chat body                     |
   | ● claude | (scrollable list, compose,    |
   |   refact |  etc. — all from active       |
   |   tests  |  session)                     |
   |          |                               |
   |   [+]    |                               |
   +----------+-------------------------------+
   | footer (full width)                      |
   +----------+-------------------------------+
   ```

   The sidebar occupies a left column between header and footer. The chat body (list, compose box) fills the remaining horizontal space. Header and footer span the full window width as today.

10. **Chat body.** [SHIPPED] Renders as today, but reads from `ring.active_mut().state` instead of a bare `ClaudeState`. The list state, flat items, tool call store, compose box — all belong to the active session. The chat body's width is reduced by the sidebar width when the sidebar is visible.

11. **Header.** [SHIPPED] The header shows the active session's attach status and turn timer. The active session's label is shown alongside the attach label (e.g., `sketch-gpui [claude: refactor] — ACP: …`).

12. **Footer.** [SHIPPED] Unchanged. Shows mode, cursor position, and hints for the active session.

### Key dispatch

13. **Session-level keys.** [SHIPPED] These are processed before the active session's key handler in `handle_claude_key`:
    - `Ctrl-]` — next session (wraps around).
    - `Ctrl-[` — prev session (wraps around).
    - All other keys are forwarded to the active session's handler unchanged.

14. **Menu commands.** [SHIPPED] Entries in the claude submenu:
    - `n` — "new session" → `claude-new`
    - `x` — "close session" → `claude-close`
    - `]` — "next session" → `claude-next`
    - `[` — "prev session" → `claude-prev`
    - `c` — "clear → fresh session" → `claude-clear`
    - `r` — "reboot → resume claude" → `claude-reboot`
    - `R` — "rename session" → `claude-rename`
    - `d` — "detach session" → `claude-detach`
    - `a` — "attach session" → `claude-attach`

    Existing entries (`o`, `s`, `m`, `t`) operate on the active session, unchanged.

### Session persistence

15. **Multi-session persistence.** [SHIPPED] The persistence format changed from `{cwd: session_id}` (a string) to `{cwd: [{id, label, active}]}` (an ordered list). Each entry records the agent-side session id, the user-facing label, and a single `active: true` marker on the slot that was active when the ring was last written. The list preserves the ring's slot order so reboot restores the same `[claude-1, refactor, tests]` arrangement. An empty list, a missing `cwd` key, and an unparseable file are all equivalent and trigger the no-saved-state path (open a fresh `claude-1`).

    **Persisted id stability (NOT the channel's current id).** Each `SessionSlot` carries a `resume_id: Option<String>` set when the slot was created from persistence. The slot's persisted id is `resume_id.unwrap_or_else(|| channel.session_id())` — i.e., the slot remembers what it was *trying* to resume, not what it ended up with. When `session/load(resume_id)` fails and the slot falls back to `session/new`, `resume_id` is **unchanged**: the persisted entry continues to point at the original id, so subsequent reboots retry the load. If the agent has truly GC'd the session, repeated reboots produce repeated transient fallbacks; the user closes the slot to discard a session whose agent-side state is gone. This rule prevents a transient `session/load` failure (agent restart, file lock, momentary network) from silently nuking the only resumable id on disk.

    Save trigger: **every time the ring changes** — whenever a session id is first assigned (current single-session save point), whenever a slot is added/removed/renamed, and whenever the active index changes. Writes remain best-effort: failures are silent. Per-slot writes always write the **whole ring snapshot**, so a stale pump from a slot that was just removed contributes nothing (its slot isn't in the snapshot).

    Restore: on launch with `SKETCH_OPEN_CLAUDE=1` (or on the first `open-claude` after launch), `load_persisted_acp_sessions(cwd)` returns the saved list. Sketch builds a `SessionRing`, pushes one `SessionSlot` per entry with its saved label and `resume_id`, and each slot spawns an `AcpChannelClient` with `session/load` using its saved id. The slot marked `active: true` becomes the active slot (or the first slot if none is marked). Slots whose `session/load` fails fall back to `session/new` (per `acp_channel.rs` resume logic) and remain in the ring with a fresh subprocess; their `resume_id` stays set so the next reboot retries the original load. After restore, `next_index` is set to `slots.len()` so the next `claude-new` produces a label that doesn't collide with restored slot labels.

    Pending-attach slots (channel not yet resolved, no id available) are not persisted. A reboot invoked in the same tick as a fresh `claude-new` will drop the new slot — acceptable because the conversation has zero content.

    **Concurrent sketch instances on the same `cwd`: last-writer-wins.** Each save is a read-modify-write of the file (read JSON, replace the `cwd` entry, write back). Other `cwd` entries are preserved. There is no file locking. Two sketch processes on the same project will overwrite each other's rings as they save; the single-user-one-instance-per-cwd scenario (the realistic case) is unaffected.

    Old-format migration: if the loader sees a bare string for the `cwd` entry instead of a list, it treats it as a single-element list `[{id, label: "claude-1", active: true}]`. The next save rewrites the file in the new format.

16. **Reboot.** [SHIPPED] `claude-reboot` spawns a child process with `SKETCH_OPEN_CLAUDE=1` and quits. The child restores the full ring as it was at the moment reboot was invoked (eager writes mean no extra save step in `reboot_into_claude` is needed).

17. **Clear.** [SHIPPED] `claude-clear` is the "nuclear reset." It removes the entire `cwd` entry from `acp_sessions.json` via `forget_persisted_acp_sessions(cwd)`, then drops the current claude screen and re-opens with a single fresh session. Per-session clear (clear only the active session, keep the rest) is out of scope; the user can `claude-close` the unwanted session and `claude-new` a replacement.

### Resource limits

18. **Soft cap.** [SHIPPED] Sketch does not enforce a hard limit on session count, but `claude-new` writes an advisory footer status when the ring reaches 6+ sessions: "N sessions active — each uses ~100MB." The user can ignore this. The cap is advisory because the user may have legitimate reasons for many sessions (e.g., testing prompt variations). The status is one-shot — cleared on the next non-shortcut keystroke (same lifetime as other transient claude statuses).

## Data Model

### SessionSlot

[SHIPPED] all fields (`src/bin/sketch-gpui/main.rs:2392-2407`):

```rust
struct SessionSlot {
    label: String,
    index: usize,               // monotonic, not reused after close
    state: ClaudeState,
    has_unseen_activity: bool,
    resume_id: Option<String>,  // id the slot was created from on persistence restore;
                                // preserved across session/load fallback (and across detach)
}
```

### SessionRing [SHIPPED] (`src/bin/sketch-gpui/main.rs:2411-2514`)

```rust
struct SessionRing {
    slots: Vec<SessionSlot>,
    active: usize,            // index into slots
    next_index: usize,        // monotonic counter for SessionSlot::index
    underlying: Option<Box<WindowContent>>,  // content to restore on back_to_doc
}
```

Methods (all SHIPPED): `active`, `active_mut`, `next`, `prev`, `push`, `close_active`, `slot_by_index`, `slot_by_index_mut`, `len`, `is_empty`, `iter`.

### Relation to existing state [SHIPPED]

`WindowContent::Claude(SessionRing)` replaces the old `WindowContent::Claude(ClaudeState)`. Accessors `claude_mut() / claude_ring() / claude_ring_mut()` provide the common borrow paths. `ClaudeState` itself is unchanged.

### Persistence format [SHIPPED]

```json
{
  "/Users/scott/ws/sketch": [
    { "id": "ses_abc123", "label": "claude-1", "active": true },
    { "id": "ses_def456", "label": "refactor" }
  ]
}
```

- Path: `~/.cache/sketch/acp_sessions.json` (unchanged).
- Order: list order is the ring slot order; preserved across reboot.
- `active` is a single optional flag on one entry. If no entry has `active: true`, the loader picks slot 0; if multiple entries have it, the loader picks the first one with the flag set (manual editing artifact — saver only ever writes one).
- Slots without a session id (e.g., detached, never-attached) are not written. Persistence captures resumable sessions only.
- Migration: a bare string value (`{cwd: "ses_xyz"}`) is read as a one-element list `[{id: "ses_xyz", label: "claude-1", active: true}]`. The next save rewrites in list form.

## Interfaces

Ring API (all SHIPPED, `src/bin/sketch-gpui/main.rs:2422-2514`):
- `SessionRing::new(underlying: Option<Box<WindowContent>>) -> Self`
- `SessionRing::push(&mut self, label: String, state: ClaudeState, resume_id: Option<String>) -> usize` — append and activate; returns the new slot's monotonic index.
- `SessionRing::close_active(&mut self) -> Option<ClaudeState>` — remove and return active state (drops subprocess via `kill_on_drop`).
- `SessionRing::next(&mut self)` / `prev(&mut self)` — cycle active.
- `SessionRing::active(&self)` / `active_mut(&mut self) -> &mut SessionSlot`
- `SessionRing::iter(&self) -> impl Iterator<Item = &SessionSlot>`
- `SessionRing::slot_by_index(&self, index: usize) -> Option<usize>` / `slot_by_index_mut`

App methods (all SHIPPED, `src/bin/sketch-gpui/main.rs`):
- `open_claude_inner()` (4605) — bootstraps the ring on first open; delegates to `new_claude_session` if already open. On restore, walks `load_persisted_acp_sessions()` and pushes one slot per entry, each with `session/load` against its saved id.
- `new_claude_session()` (4660) — push a fresh `session/new` slot. Surfaces the §18 advisory at 6+ slots.
- `close_active_claude_session()` (4701) — close active; `back_to_doc()` if ring empty (also `forget_persisted_acp_sessions(cwd)` so a stale entry doesn't resurrect on reboot).
- `switch_claude_session(direction: i32)` (4688)
- `clear_claude_session()` (5043) — see §17 for the multi-session contract.
- `detach_active_claude_session()` (5104) — see §4.
- `attach_active_claude_session()` (5131) — see §4.
- `open_rename_overlay()` (3868) — see §5.
- `reboot_into_claude()` (5177) — see §16.

Persistence functions (all SHIPPED, `src/bin/sketch-gpui/main.rs`):
- `save_persisted_acp_sessions(cwd, ring: &SessionRing)` (1240) — write all slots with session ids, preserving order, with the active slot flagged. Best-effort.
- `load_persisted_acp_sessions(cwd) -> Vec<PersistedSlot>` (1164) — read the list; migrate from old string format on the fly.
- `forget_persisted_acp_sessions(cwd)` (1210) — drop the whole `cwd` entry (used by `claude-clear` and the close-last-slot path).

The single-session names (`save_persisted_acp_session`, `load_persisted_acp_session`, `forget_persisted_acp_session`) were removed; all call sites use the plural versions.

## Constraints

1. **No shared state between sessions.** Each `ClaudeState` is fully independent — its own editor, channel, tool calls, compose box, and turn timer. Sessions do not communicate with each other. There is no "shared context" feature.

2. **Subprocess cost.** Each session spawns an `AcpChannelClient` subprocess (~100MB RSS for the Node.js + SDK process). With 5 sessions, that's ~500MB of agent overhead. The advisory warning at 6+ sessions is the only mitigation. A future optimization could share a single agent process with multiplexed sessions (requires ACP protocol changes) but is out of scope.

3. **Background pump budget.** Each session's pump task runs independently at 16ms cadence with a 64-event budget per tick. With N sessions, worst-case CPU is N × 64 events per 16ms — acceptable for N ≤ 10. If profiling shows contention, the budget can be reduced for inactive sessions.

4. **Sidebar width.** The sidebar is fixed at 20 chars wide. Labels are truncated with `…` if they exceed the available space. The sidebar scrolls vertically when sessions exceed the viewport height. Clicking a session label switches to it; `Ctrl-]`/`Ctrl-[` remain the keyboard-first navigation.

5. **Persistence migration.** The loader handles both old format (bare string) and new format (list of `{id, label, active?}`). The saver always writes the new format. Old sketch versions that read the new format will see a parse error and start a fresh session. With multi-session this means a downgrade silently abandons *all* persisted sessions for that cwd (not just one). Documented downgrade path: manually edit `~/.cache/sketch/acp_sessions.json` to replace the array with a single bare string id for the slot the user wants to keep.

6. **Restore concurrency.** Each restored slot spawns its `AcpChannelClient` on its own background thread (matches today's per-session attach behavior). With N restored slots, restore produces N concurrent agent subprocess spawns — ~100MB RSS per slot during the restore window. No serialization; the agent SDK handles independent concurrent `session/load` calls already because each subprocess has its own stdio pipe.

7. **Compose-box drafts not persisted.** Unsent text in each session's compose box is lost on reboot. Persisting per-keystroke draft state is out of scope for this spec.

8. **Scope boundary.** This spec covers GPUI only. The TUI frontend (`src/app/`) is not modified. If the TUI gains multi-session support later, it can reuse `SessionRing` (it is not GPUI-specific) but will need its own rendering and key dispatch.

## Revision History

- 2026-05-14 — Status now SHIPPED across the board. Detach/attach (§4), rename (§5), and the §18 soft cap landed. Persistence (§15-17) and Ctrl-]/Ctrl-[ (§13) were already shipped — spec markers updated to reflect that. App-method line refs migrated from the old monolithic `src/bin/sketch-gpui.rs` to the new `src/bin/sketch-gpui/main.rs` after the directory split. Removed the standalone "DRAFT persistence fields" block on `SessionSlot` since `resume_id` is now a permanent field; same edit clarified that `resume_id` is preserved across detach (not just `session/load` fallback). Per-slot `[detached]` indicator surfaces as ` [d]` suffix in the sidebar (was already wired before detach landed).
- 2026-05-11 (2) — Adversarial review pass. §15 now specifies `resume_id` stability across `session/load` fallback (transient failures no longer overwrite the persisted id); explicit last-writer-wins contract for concurrent sketch instances on the same cwd; whole-ring snapshot writes (no stale-pump corruption); pending-attach slots dropped on reboot; `next_index = slots.len()` post-restore. New Constraints §6 (restore concurrency) and §7 (compose-box drafts out of scope). Constraint §5 updated with downgrade blast radius and manual-edit recovery path.
- 2026-05-11 — Marked ring, sidebar, lifecycle (create/switch/close), background pumping, header/footer/chat-body rendering, and the single-cwd save+load+forget functions as SHIPPED. Refined §15–17 persistence design: list-of-slots format with an `active` flag, eager writes on every ring change, per-slot fallback to `session/new` when load fails. Renamed planned persistence functions to plural (`save_persisted_acp_sessions` etc.). Detach/attach (§4), rename (§5), direct `Ctrl-]`/`Ctrl-[` bindings (§13), and the soft cap (§18) remain DRAFT.
- Initial draft — single document covering the full multi-session feature.
