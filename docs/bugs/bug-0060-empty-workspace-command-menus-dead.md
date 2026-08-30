# bug-0060: empty-workspace-command-menus-dead

**Status:** FIXED
**First seen:** 2026-08-29
**Component:** `docs/components/workspace.md`, `docs/components/common/menu.md`

## Symptom

After the sole visible tile is closed and the valid “Empty workspace” surface is
visible, pressing either command leader—`space` or `.`—does nothing. The shell
menu should remain reachable; with no focused App, `space` should fall back to
the same shell menu instead of becoming a dead key.

## Context / root cause

`chrome.rs::render_focused_window` special-cases `Layout::Empty` with a bare
`div` carrying only Ctrl-W action listeners. Unlike every tile screen root, that
surface neither owns the shared focus handle nor calls a raw `on_key_down`
handler, so production keystrokes never reach `leader_intercept`. In addition,
the menu overlay records a mandatory focused `WindowId`, preventing the shell
menu from representing the legitimate no-tile origin.

This is the stale edge documented in `UXI-Workspace-1`, and it violates the
two-leader reachability contract in `UXI-Menu-6`.

## Solution

Make the empty-state surface the focused shell input root when no overlay owns
focus, route raw keys through universal leader interception, allow a menu origin
with no tile, and make `space` fall back to the shell menu only when no focused
App exists. Guard the exact close-sole-tile → paint → real-keystroke path and
observe it RED before the fix.

## Approaches already tried (do NOT repeat)

- Running changed-line mutation testing inside the default sandbox fails while
  compiling Metal because Clang cannot write its module cache. Use the approved
  escalated `cargo mutants` invocation.
- cargo-mutants copies the source without `.git`, so the unrelated
  `agent_stats_restores_durable_observations_before_fresh_collection` baseline
  guard fails there. Pass Cargo's test separator through the tool and skip only
  that repository-observation guard.
- Repository-wide `cargo fmt --all -- --check` is already red from unrelated
  formatting drift. Do not retain a bulk formatting rewrite; keep the scoped
  patch clean under `git diff --check`.

---

## Log

### 2026-08-29 16:50 PDT — localized and reproduced

The production empty-layout render branch was found to omit both focus ownership
and raw leader-key dispatch. A headless GPUI guard was added against the actual
close-sole-tile and painted-input path; implementation and verification evidence
will be appended after the required negative control.

### 2026-08-29 17:05 PDT — negative control observed

`empty_workspace_keeps_both_command_leaders_live` drove the production close
command, painted the `Layout::Empty` branch, and sent a real `space` keystroke.
Before the implementation change it failed at “space opens the shell menu on an
empty workspace,” proving that direct handler tests would have missed the bug.

### 2026-08-29 17:25 PDT — fixed and verified

The empty surface now tracks the shared focus handle, routes raw keys through
`leader_intercept`, and carries the same shell/global action listeners as normal
screen roots. `MenuOverlay.opened_from` now represents an absent tile honestly,
and the local-menu opener falls back to the shell menu for `space` only in that
state. The targeted guard passes for both leaders and verifies their distinct
trails and `None` origin.

Verification passed: the GUI suite (758 passed, 2 ignored), library suite (213
passed, 2 ignored), debug builds for both runtime binaries, and `git diff
--check`. Changed-line mutation testing evaluated six mutants: five were caught
and one was unviable. Its one environment-dependent baseline guard was excluded
because cargo-mutants' temporary source copy intentionally has no `.git` data.
