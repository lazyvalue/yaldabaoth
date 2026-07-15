# Bug Manifest

The index of every bug we've worked on. **Read this first** (via `/bug`) before
attacking a bug — if it's here, the linked file holds the history of what was already
tried, so we don't attempt the same failed fix twice (that's the definition of
insanity). Higher fidelity than git: each bug file carries context, the planned fix,
and a **timestamped log** of every actual attempt.

## How this works

- Each bug is `docs/bugs/bug-<NNNN>-<slug>.md` (zero-padded, sequential). Template:
  `docs/bugs/_template.md`.
- A bug addressed **more than once** does NOT get a new file — it gets a new
  timestamped entry appended to the bottom of its existing file. One bug, one file,
  a growing log.
- Status: `OPEN` (known, not fixed) · `IN-PROGRESS` · `FIXED` (fix landed +
  verified) · `RECURRED` (came back after a FIXED — see its latest log entry) ·
  `WONTFIX`.

## Bugs

| Id | Slug | Status | First seen | Times addressed | One-line |
|----|------|--------|------------|-----------------|----------|
| bug-0001 | created-session-not-persisted | RECURRED→FIXED | 2026-07-14 | 3 | created server-managed sessions weren't persisted (resume_id/channel both None) → picker on restart; fixed by resolving the sid via the store's `sid_of` |
| bug-0002 | restore-drops-replayed-history | FIXED | 2026-07-15 | 1 | on restore, replayed history vanished when a respawn (gen bump) happened while the §9 gate was closed — the deferred rebaseline's `reset_for_replay` wiped legacy-rendered replay the gated reducer had skipped; fixed by applying the rebaseline eagerly at `ChannelOpened` |

<!-- Example row once populated:
| bug-0001 | chatbox-caret-offscreen | RECURRED | 2026-05-02 | 16 | caret + text scroll out of the visible chatbox; no single owner of caret-in-viewport |
-->
