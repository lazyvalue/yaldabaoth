# bug-0061: new-cog-solo-noop

**Status:** FIXED
**First seen:** 2026-08-27
**Component:** Cog / Workspace

## Symptom

With a detached or hidden tile presented directly, `.` → new → cog closes the
menu but appears to do nothing. The same command works from a normal workspace.

## Context / root cause

The `new-cog-tile` dispatcher only called `Frame::split_focused`. A solo-presented
tile lives outside the visible workspace layout, so there is no focused layout
leaf to split and `split_focused` returns `None`. The command silently exited.

This violated the contextual tile-creation ownership behavior already used by
New Agent, `UXI-Workspace-24`'s independent presentation/attachment model, and
the Cog opening contract in `UXI-Cog-13`.

## Planned solution

When a solo presentation is active, create a stable detached `App::Cog` tile,
present it directly, and start its production load by stable tile id. Preserve
the existing split-and-replace path for normal workspaces.

## Approaches already tried (do NOT repeat)

- Treating this as a missing Linux `cog` executable or unreachable `cogd`: the
  command failed before any Cog subprocess load was relevant.

---

## Log

### 2026-08-27 13:47 — contextual New Cog ownership fixed

Added
`new_cog_tile_from_solo_presentation_creates_and_focuses_detached_cog`, which
drives the real `dispatch_menu_command("new-cog-tile")` path in both an attached
workspace and a solo-presented detached tile. The pre-fix negative control failed
because the presented tile remained the original id. The fixed dispatcher creates
and focuses a distinct detached Cog tile, preserves the original Linear tile, and
keeps ordinary workspace splitting intact.
