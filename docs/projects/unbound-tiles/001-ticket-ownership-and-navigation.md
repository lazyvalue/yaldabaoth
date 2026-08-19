# 001 — Optional workspace ownership and navigation

**Status:** complete
**Depends on:** ADR-0033

## Subtasks

- [x] Add tile project + tags and the Frame unbound collection/direct-focus
      pointer.
- [x] Add invariant-preserving bind, unbind, lookup, focus, and workspace-close
      operations.
- [x] Persist unbound tiles and migrate legacy session/buffer tags.
- [x] Replace ephemeral direct session views with real unbound Agent tiles.
- [x] Put unbound tiles in Cmd-P and make bound results focus their workspace.
- [x] Render workspaces as independently collapsible folders of bound tiles;
      render only unbound tiles in the tag-grouped Unbound list.
- [x] Add production-path tests, observed-RED evidence, mutation testing, and
      full build/test evidence.

## Non-goals

- The period (`.`) shell menu is not another tile picker.
- Cross-project binding is not introduced.
- Killing an Agent session is not coupled to tile membership.
- Layouts are not normalized into a central id-only arena in this pass.
