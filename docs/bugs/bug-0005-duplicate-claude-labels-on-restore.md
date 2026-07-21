# bug-0005: duplicate-claude-labels-on-restore

**Status:** FIXED
**First seen:** 2026-07-16
**Component:** docs/components/agent-tile (session labeling / persistence)

## Symptom

After reopening the GUI, two agent sessions in the SAME cwd are both named `claude`
(bare, no number). Sessions should have unique labels (`claude-1`, `claude-2`, …).

## Context / root cause

New sessions are labeled by `next_agent_label` (agent_ui.rs:188), which dedupes:
`claude-1`, `claude-2`, … So a **bare `claude`** (no suffix) can only come from a
fallback default, of which there are three, none deduped against each other:

1. `persist.rs:1077` — a persisted slot with a MISSING `label` key loads as
   `.unwrap_or("claude")`.
2. `main.rs:1922` — restore's by-id side-channel MISS (leaf sid in the layout but not
   in `acp_sessions.json` for this cwd) fabricates a slot with `label: "claude"`.
3. `main.rs:2028` — legacy no-server path defaults to `claude-1`.

The restore loop (`restore_agent_leaves`, main.rs:1874) binds each leaf's session with
`slot.label` **as-is** — it never dedupes labels across the batch. So two slots that
both resolve to `claude` (two missing labels, or two side-channel misses) produce two
sessions named `claude` in one cwd.

## Planned solution

Dedupe labels so no two sessions in one cwd share a name:

1. In `load_persisted_acp_sessions` (persist.rs), after building the slot list, run a
   `dedupe_slot_labels` pass: a missing/empty or already-seen label is reassigned to
   the next free `claude-N`; valid distinct labels are untouched. Also change the
   missing-label default from `"claude"` → `""` so a missing label always becomes a
   numbered `claude-N`, never bare `claude`.
2. In `restore_agent_leaves`, seed a `used` label set from the (deduped) persisted
   labels + existing store sessions, and draw the fabricated-fallback / legacy labels
   from it so they can't collide either.

## Approaches already tried (do NOT repeat)

- <none — first attempt held>

---

## Log

### 2026-07-16 — dedupe labels at load + restore (FIXED)

**Status:** FIXED.

**What changed.**
- `persist.rs`: new `unique_label(desired, used)` (desired if free+non-empty, else
  smallest free `claude-N`) + `dedupe_slot_labels(slots)`; `load_persisted_acp_sessions`
  now dedupes the loaded slot list before returning, and a MISSING label loads as `""`
  (was bare `"claude"`) so it always becomes a numbered `claude-N`.
- `main.rs` `restore_agent_leaves`: both branches (server + legacy) keep a running
  `used_labels` set and draw each bound session's label through `unique_label`, so a
  by-id-MISS fabricated slot (now `""` instead of `"claude"`) or a cross-leaf collision
  can't produce two identical names.

**How verified.** Localized via code trace (three bare-`"claude"` fallbacks, no dedup
in the restore loop). Guard `restore_dedupes_duplicate_claude_labels` (tests.rs) drives
the REAL loader against a raw `acp_sessions.json` with two slots labeled `"claude"` +
one missing label, asserts all three restored labels are non-empty and DISTINCT. Unit
`unique_label_fills_gaps_and_replaces_empty_or_dup` pins the helper. **Negative control
(observed RED):** commenting out `dedupe_slot_labels(&mut slots)` in the loader → two
`"claude"`s + an empty survive → uniqueness/non-empty asserts fail; restored → green.
Full suite: 377 pass.

**Coverage / gap.** The loader dedup (the primary source: duplicate/missing labels in
the side-channel) is guarded on the real path. The restore-loop fabricated-fallback
dedup (server-managed `restore_agent_leaves`) shares `unique_label` but its wiring runs
only in the full GUI↔server restore loop — a genuine `NEEDS-RUNTIME` gap (#2). Runtime-
unverified end-to-end; the loader fix + helper are headless-green.
