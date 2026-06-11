# Runtime check: 5c Doc/Edit shared rope (pooled SharedCore)

**What landed:** `9e8f3ce` (master). ADR-0007 / spec §6 step 5c.
**Why a human:** the model/pool seam is proven headlessly
(`pool_dedups_by_path_so_two_views_share_one_core`,
`re_render_one_doc_sources_live_core_not_disk`), but GPUI can't be driven
headlessly, so the **cross-pane paint** (two panes visibly updating) needs eyes.

Launch: `cargo run --bin yalda-gpui <some.md>`

Keys (from `register_keymap`): `Ctrl-W s` split horizontal · `Ctrl-W v` split
vertical · `Ctrl-W Shift-M` also-show focused content in a new pane · `Ctrl-W h/j/k/l`
move focus · `Ctrl-E` enter Edit · `Ctrl-V` back to Doc · `Ctrl-S` save ·
`:theme <name>` cycle theme.

---

## 1. Live cross-pane reflection (the headline 5c behavior) — ⬜
1. Open a file (lands in a Doc pane).
2. `Ctrl-W v` to split — two panes, same file.
3. Focus one pane (`Ctrl-W l`), `Ctrl-E` to enter Edit mode there.
4. Type some text in the Edit pane.
- **PASS:** the *other* (Doc) pane re-renders the new text **live**, within a frame
  or two, without any save/toggle.
- **FAIL:** the Doc pane stays stale until you save / re-open / toggle.

## 2. Unified undo across views — ⬜
1. From state in check 1 (an edit made in the Edit pane).
2. In the Doc-side pane's editor (or after `Ctrl-V`/`Ctrl-E` round-trip), trigger undo.
- **PASS:** the edit made via the *other* view is undone — one history per file.
- **FAIL:** undo only affects the view that made the edit, or does nothing.

## 3. Theme switch preserves unsaved edits (the specific bug fixed this pass) — ⬜
1. Open a file, `Ctrl-E`, type an **unsaved** change (do NOT `Ctrl-S`).
2. `Ctrl-V` back to Doc (or split so a Doc pane shows it).
3. `:theme <a-different-theme>` to force a re-render.
- **PASS:** the Doc still shows your unsaved edit, re-themed.
- **FAIL (the old bug):** the Doc reverts to the on-disk content (your unsaved edit
  vanishes) and stays reverted.

## 4. also-show variant — ⬜
1. Open a file in a Doc pane.
2. `Ctrl-W Shift-M` (AlsoShowPane) to mirror it into a second pane.
3. Edit in one; confirm the other tracks live (same as check 1).
- **PASS:** both panes share the rope; edits reflect both ways.

## 5. No-regression sanity — ⬜
- String-backed Docs (help/welcome, or a path that doesn't exist yet) still render
  and don't panic on theme switch (the `source == None` disk-fallback branch).
- Closing one pane of a shared file leaves the other fully editable; reopening the
  file later still loads (gc didn't reap a referenced/dirty buffer).

---

When checks 1–4 pass: drop the `NEEDS-RUNTIME` flag on 5c in `docs/backlog.md`
(Top-priority → state-first overhaul → Phase B) and note the date here.

**Result log:**
- _(unfilled — run on next GPUI launch)_
