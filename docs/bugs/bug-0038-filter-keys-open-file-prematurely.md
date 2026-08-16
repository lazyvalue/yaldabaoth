# bug-0038: filter-keys-open-file-prematurely

**Status:** FIXED
**First seen:** 2026-08-16
**Component:** Buffer file picker (`App::Buffer(BufferApp::Picking)` / `BrowserView`)

## Symptom

In a Buffer tile's file picker, after pressing `/` to search, certain keys open
the selected file (or navigate away) instead of editing the search query. The
arrow keys are the clearest: pressing `right` mid-search opens the highlighted
result; `left` jumps to the parent directory. Bound letters behave the same way
(`l` opens, `h`/`-` go up, `r` starts a rename, `s` cycles sort, `q` closes).

## Context / root cause

`/` search text is accumulated by a **capture-phase** key listener,
`handle_browser_filter_key` (`browser_ui.rs`), attached to the `BrowserView`
element alongside a full set of `.on_action(...)` bindings (`keymap_registry.rs`
binds `right`/`l`→BrowserEnter, `left`/`h`/`-`→BrowserParent, `r`→BrowserRename,
`s`→BrowserCycleSort, `q`/esc→BrowserClose, `j`/`k`→BrowserDown/Up).

The design assumed the capture listener's `cx.stop_propagation()` would suppress
those bindings while filtering. **It cannot.** GPUI 0.2.2 dispatches bound
actions *before* capture key listeners:
`window.rs::dispatch_key_event` runs the `match_result.bindings` loop
(`dispatch_action_on_node`) first, then only calls `finish_dispatch_key_event`
(which runs the capture/bubble key listeners) if propagation still stands. Worse,
a matched action **consumes** the event by default (propagation is not
re-enabled), so for any bound key the capture listener never runs at all — the
key fires its action and is never seen as filter text.

So during `/` search every *bound* key fired its `BrowserView` action:
`right`/`l` opened the selected file (the reported bug), `left`/`h`/`-`
navigated up, `r` began a rename, etc. Only *unbound* characters (most vowels,
`t`, `n`, digits, …) ever reached the query. The earlier belief that "letters are
safe, appended to the query" was wrong.

## Planned solution

The capture listener runs too late to prevent an action, so the fix is in the
**action handlers**: while the browser is capturing text (`filter_mode` on, or an
inline `rename` open) the nav/open/mutate `BrowserView` actions must no-op and
let the capture handler own input. Added `browser_text_captured()` and an early
`return` guard to `browser_down`, `browser_up`, `browser_enter`,
`browser_parent`, `browser_worktrees`, `browser_toggle_hidden`,
`browser_cycle_sort`, `browser_close`, `browser_rename`. `browser_filter` (`/`)
guards on rename only, preserving the "`/` toggles search off" gesture.

**Known limitation (follow-up):** because a matched action consumes the event
before the capture listener can append it, bound keys (`l h j k s q w r -`) are
now *swallowed* during search rather than typed into the query. Search still
works via fuzzy matching on the remaining characters, but those letters can't be
entered literally. A full fix (letters typeable in search) needs the browser to
stop binding bare letters and route all keys through one handler, or to register
filter capture as a pre-binding keystroke interceptor — larger than this bug.

## Approaches already tried (do NOT repeat)

- **Catch-all `cx.stop_propagation()` in the capture handler's `_` arm.** Does
  NOT work: GPUI dispatches the action before the capture listener, so
  propagation is already spent — the file still opened. Verified: the guard test
  stayed RED with only this change. The listener cannot cancel an action; the fix
  must live in the action handlers.

---

## Log

### 2026-08-16 — Guarded the BrowserView action handlers on text-capture

**Root cause** (see above): GPUI 0.2.2 runs bound actions before capture key
listeners and consumes the event, so the `/`-filter capture listener could never
suppress the `BrowserView` bindings — every bound key fired its action mid-search
(`right`/`l` → BrowserEnter → `open_file`).

**Fix:** `src/bin/yalda-gpui/browser_ui.rs` — new `browser_text_captured()`
(`filter_mode || rename.is_some()`) plus an early-return guard in every
nav/open/mutate `BrowserView` action handler (`browser_down/up/enter/parent/
worktrees/toggle_hidden/cycle_sort/close/rename`); `browser_filter` guards on
rename only. In nav mode the guard is false → behavior unchanged.

**Verified:** `verify_harness.rs::browser_filter_arrow_key_does_not_open_file`
drives the REAL keymap (`register_keymap` + `simulate_keystrokes`): `/`, a query
that selects a real file, then the actual `right` keystroke → asserts the tile is
still the `Picking` browser (file not opened); then `h` → asserts the dir did not
change (BrowserParent didn't leak).

**Negative controls (both observed RED, then restored):**
- Remove the `browser_enter` guard → `right` fires BrowserEnter → tile flips to
  `Viewing`; test fails "the tile flipped away from the picker".
- Remove the `browser_parent` guard → `h` navigates to parent (clears filter);
  test fails "still filtering after a bound letter".

Full GUI suite green (564 passed, 1 ignored live test). Fix + guard on `main`.
