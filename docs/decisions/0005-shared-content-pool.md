# ADR-0005: Panels are views onto a refcounted shared Core (buffer pool wiring deferred)

**Status:** Accepted
**Date:** 2026-06-02
**Related:** ADR-0002, spec-workspaces-tagging.md, memory: project-buffer-pool-unwired

## Context

Workspaces "also-show" (and any same-content-in-two-places feature) needs the
underlying content shared by reference. `workspace.rs` already has a refcounted
buffer pool (`open_buffer` / `buffer_retain` / `FileBufferId` / `EditorView` /
`EditorCore`) — but it is **dead code, never wired into the live app**:
`DocState`/`EditState` own content directly, and splits duplicate by re-reading
from disk. An early framing said "only file buffers can be multi-home"; the user
correctly pushed back that *all* panel types are panels.

## Decision

The unifying model: **a panel is a view onto a refcounted shared Core.** What
varies per type is only what the Core is and what's per-view:

| Panel | Shared Core (pooled, refcounted) | Per-view |
|---|---|---|
| Doc/Edit | `EditorCore` / buffer (by `FileBufferId`) | cursor, scroll, selection |
| Agent | the server-side session (by `server_session_id`) | transcript scroll, focused subagent, chatbox draft |
| Browser | cwd / dir cursor | scroll |

Multi-membership = multiple view-leaves across workspaces, each pointing at the
same Core by id, each with its own per-view state. Uniform across types.

For v1 we **deferred wiring the pool**: also-show re-reads from disk (no shared
unsaved edits), matching existing split behavior. Wiring it is the unblocker.

## Rationale

The Core/View split is what every successful editor converged on, and ACP is
*well*-positioned, not excluded — the session server is already a shared-Core
pool (Owner + N Observers, replay-on-attach). The only per-type cost is
separating the fused state into Core + View.

## Consequences

- **Backlog: wire the buffer pool into the live app** (`ff-buffer-pool`) →
  unblocks real shared-text/undo also-show for docs.
- **Agent also-show** additionally needs: refcount the per-session attach
  (attach once per connection), and fan event delivery out to *all* views
  (the `for_each_server_session_slot` helper already exists) — reversing the
  ADR-0003 dedup. `NEEDS-DECISION` before building.
- Saved as a durable gotcha in agent memory so future agents don't assume
  `buffer_retain` already works.
