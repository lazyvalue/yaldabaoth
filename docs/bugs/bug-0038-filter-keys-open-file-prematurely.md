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

**Real fix (typeability restored):** the guards alone SWALLOW bound letters
(`l h j k s q w r -`) during search — search became unusable, since most
filenames contain them. The proper fix is to stop the bindings from matching at
all while filtering: render the browser under a different **key context**
(`BrowserFilter`, and `RailFilter` for the rail) that has NO bare-letter/arrow
bindings. GPUI resolves a key against the dispatch path's contexts; with no
`BrowserView`/`RailView` binding matched, the event flows to the capture handler,
which types the key into the query. Global `cmd-*` bindings (`None` context)
still work. `dispatch_key_event` redraws before each keystroke when dirty, so the
context is always fresh — no one-frame skew. The action-handler guards stay as
defense-in-depth (they never fire during filter now, because no binding matches).

## Approaches already tried (do NOT repeat)

- **Catch-all `cx.stop_propagation()` in the capture handler's `_` arm.** Does
  NOT work: GPUI dispatches the action before the capture listener, so
  propagation is already spent — the file still opened. Verified: the guard test
  stayed RED with only this change. The listener cannot cancel an action.
- **Action-handler guards ALONE (no context switch).** Stops the premature open
  but SWALLOWS bound letters during search (the action consumes the event before
  the capture handler can append it) — search becomes unusable. Must be paired
  with the key-context switch so the bindings never match while filtering.

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

Full GUI suite green (564 passed, 1 ignored live test). Fix + guard on `main`
(commit `e0c2def`).

### 2026-08-16 (later) — Key-context switch restores typeability; rail fixed too

The guard-only fix stopped the premature open but SWALLOWED bound letters
(`l h j k s q w r -`) during search — the reporter flagged that search was then
useless (most filenames contain those letters). Root of the swallow: GPUI
dispatches a matched action *and consumes the event* before the capture handler,
so a bound key never reached the query.

**Real fix:** render the browser under a distinct **key context** while capturing
text so the bindings don't match and every key flows to the capture handler:
- `src/bin/yalda-gpui/screens.rs::render_browser` — `key_context` is
  `"BrowserFilter"` when `filter_mode || rename.is_some()`, else `"BrowserView"`.
- `src/bin/yalda-gpui/chrome.rs::render_rail` — `key_context` is `"RailFilter"`
  when the rail's file browser is filtering, else `"RailView"`.
`BrowserFilter`/`RailFilter` have no bindings in `DEFAULT_BINDINGS`, so bound
letters/arrows fall through to `handle_browser_filter_key` /
`handle_rail_filter_key` and are typed into the query. The action-handler guards
from the first entry stay as defense-in-depth (unreached during filter now).

**Verified (real keymap, `simulate_keystrokes`):**
- `browser_filter_arrow_key_does_not_open_file` — extended: after `right` (no
  open) it types the bound letters `-`/`l` and asserts the query becomes
  `"target-file"` (letters TYPED, not swallowed).
- `rail_filter_bound_keys_type_into_query` — Cmd-B opens a focused rail, `/`
  enters filter, then `s`/`w`/`-` are typed → asserts query `"sw-"`, no worktree
  mode, dir unchanged.

**Negative controls (both RED, restored):**
- Force `BrowserFilter`→`BrowserView` → bound letters swallowed, query
  `"tagetfie"` ≠ `"target-file"`.
- Force `RailFilter`→`RailView` → `w` opens worktree mode; assert fails.

Full GUI suite green (565 passed, 1 ignored). On `main`.
