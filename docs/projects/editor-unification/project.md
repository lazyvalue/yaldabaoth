# Editor unification + editing bug sweep

**Status:** in progress (2026-07-08). First batch of shared-root fixes landed on
`main`; the larger structural unifications and the remaining bug backlog are
tracked as tickets below.

## Problem / why

There are three editable text surfaces — the **buffer** `EditView` (Code + WP),
the **agent compose** (worksheet / message box), and the **agent transcript**
nav. The user's goal: *"unify as much of the codebase between agent tiles and
buffers… they should largely be the same code for editing / rendering text,"*
and fix the pile of small editing usability bugs that keep surfacing.

A Fable multi-agent comb (2026-07-08) established the ground truth and a ranked
roadmap. This project.md is the standing context; do not re-derive it.

## The model (what is ALREADY shared vs forked)

**Text EDITING is already one engine.** All three surfaces are `Editor` instances
over `src/editor.rs` + `src/document.rs` + `src/cursor.rs`, and every keystroke
funnels through the shared `dispatch_insert_core` / `dispatch_normal_core`
(`edit_ui.rs`), called by both the buffer path and `handle_claude_key`. Insert /
delete / motions / undo are not duplicated. **Fixing a bug in the shared dispatch
fixes both surfaces at once** — this is the lever the first batch used.

**Where they genuinely FORK:**

1. **Two wrapped-line-with-caret renderers.** `build_wrapped_line` (`agent.rs`,
   GPUI flex-wrap; used by buffer Code/WP + transcript) vs `build_chatbox_*`
   (`agent.rs`, hand-computed monospace column wrap; used by compose + inline
   You-block). The chatbox renderer re-implements caret splitting, selection
   painting (`emit_chunk`), and wrap — it exists only to make the caret's visual
   row computable for the compose caret-containment window. This is the largest
   bug family in the repo (the recurring "cursor off-screen in the chatbox").

2. **`EditOps` trait + two delegating impls.** `Editor` (owned `EditorCore`) vs
   `SharedEditor` (`Rc<RefCell<EditorCore>>`) are different types, so `EditOps`
   (`main.rs`) + ~280 lines of pure delegation bridge them — three parallel
   method lists that must stay in lockstep.

3. **Scroll-follows-caret implemented 4–5×.** Buffer Code, buffer WP (now merged
   → `EditState::reconcile_and_reveal`), doc view, transcript, compose window.

Recommended execution order (Fable): **2 → 3 → 1** — target 2 is safe and
mechanical; targets 1+3 together delete the compose's parallel text stack, after
which the compose *is* a small buffer (the user's stated end-state).

## Tickets

| # | Ticket | Status |
|---|--------|--------|
| — | Batch 1: shared-dispatch bug fixes (arrows/Home/End/Delete, Cmd chords, block-caret off-by-one) | ✅ landed `main` (9d3ca50, 6f83f43) |
| — | reconcile_and_reveal dedup (scroll target 3, buffer half) | ✅ landed `main` (4f7663a) |
| 001 | Remaining editing bug backlog (undo lifecycle, tab caret-align, count prefixes, `e`/`x` at EOL, stale desired_col) | open |
| 002 | Unification target 2 — fold `EditOps`/`SharedEditor` into one `Editor` over a shared core | open |
| 003 | Unification target 1 — one wrapped-line renderer; delete `build_chatbox_*` + compose grid-window | open |

## Links

- UX contract: `docs/ux-invariants.md` (INV-UX-1 caret visible/tracks text,
  INV-UX-2 compose wraps, INV-UX-9 compose focus model).
- Chatbox off-screen saga: `docs/projects/chatbox-offscreen-recurring` /
  memory `project_chatbox_offscreen_recurring` (target 1 is the structural cure).
- Shared dispatch: `edit_ui.rs::dispatch_insert_core` / `dispatch_normal_core`.
</content>
</invoke>
