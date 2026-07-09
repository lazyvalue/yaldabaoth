# 002 — Fold `EditOps` / `SharedEditor` into one `Editor`

**Unification target 2 (do first — safe, mechanical, unblocks nothing else).**

`trait EditOps` (`main.rs:825`) exists only because `Editor` (owned `EditorCore`)
and `SharedEditor` (`Rc<RefCell<EditorCore>>`, `main.rs:741`) are different types.
The two impls (`main.rs:874` + `main.rs:1008`) are ~280 lines of pure delegation
that must stay in lockstep with `EditorView` — three parallel method lists, and a
"add a motion in three places" tax on every editing change.

## Goal

Make `Editor` hold a shared core (or be generic over `C: BorrowMut<EditorCore>`),
in `src/editor.rs`. Compose + transcript construct a single-owner shared core;
buffers use the pooled one. `SharedEditor` and `EditOps` disappear; dispatch takes
`&mut Editor`.

## Subtasks

- [ ] Add a shared-core constructor to `Editor` (or the generic param).
- [ ] Port the compose + transcript `Editor` construction.
- [ ] Port the buffer `SharedEditor` sites to the unified type.
- [ ] Change `dispatch_*_core` signatures from `<E: EditOps>` to `&mut Editor`.
- [ ] Delete `trait EditOps` + both impls (~350 LOC).
- [ ] RefCell borrow discipline: mirror the existing `SharedEditor` /
      `DocState::refresh_blocks` (`main.rs:598`) pattern.

## Verification

The full existing dispatch test coverage (verify_harness + editor.rs unit tests)
exercises the ported path. Land incrementally: add the shared-core constructor
first, port callers one at a time, delete `EditOps` last. Payoff: ~350 LOC gone;
one undo path; unlocks pooling the compose/transcript buffers.
</content>
