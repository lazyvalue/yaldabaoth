# bug-0016: renamed-session-names-lost-on-restart

**Status:** FIXED
**First seen:** 2026-07-22 ("most of the names of the newly named sessions were lost … reverted to claude-4, claude-5")
**Component:** docs/components/agent-tile (session labeling / persistence); `persist.rs`

## Symptom

After exiting and relaunching the app, sessions the user had renamed came back
under auto-generated names (`claude-4`, `claude-5`, …) instead of their custom
names. "Most" of the renamed sessions were affected. "Never, ever do this."

## Context / root cause

**A test-isolation hole clobbered the user's real persistence file.** Renamed-
session LABELS are persisted in `~/.yalda/acp_sessions.json` (restore reads
`slot.label` there — `restore_agent_leaves`, main.rs). Two of the three
persistence-path accessors were correctly hardened to return `None` under
`cfg(test)` unless a test opts into a tempdir (`workspace_persist_path`,
`preferences_path`) — so a view-booting test that triggers a save no-ops instead
of overwriting real user state. **`acp_session_persist_path()` was the one that
was missed:** under `cfg(test)` it returned the override *if set*, else fell
through to `yalda::paths::yalda_home()` — the REAL `~/.yalda/acp_sessions.json`.
`yalda_home()` itself has no test guard.

`save_agent_ring` fires from `/clear`, restore, rename, and ~every session
mutation, and the general test harness (`boot_with_transcript`,
`install_agent_slot`, `hermetic_browser_view`, `clear_agent_session`, …) does NOT
set the override. So every run of the suite that booted the real view and touched
a session overwrote `~/.yalda/acp_sessions.json` with test fixtures. The on-disk
file was found holding `{"id":"S2","label":"claude-2", …}` — unambiguous test
data (real sids are UUIDs, as `preferences.json` still showed) — written by a
prior `cargo test` run. On the next launch restore read that (or a by-id MISS
against the user's real sids in `workspace.json`), fabricated empty-label slots,
and `dedupe_slot_labels`/`unique_label` renumbered them to `claude-N`.

The user develops this app and runs the suite constantly, so: app exit (writes
correct labels) → `cargo test` (clobbers the file) → app launch (reads junk) →
names gone. That is the "when I exited the app the names were lost" loop.

## Planned solution

1. **Close the hole (primary).** Make `acp_session_persist_path()` return `None`
   under `cfg(test)` unless `ACP_PERSIST_PATH_OVERRIDE` is set — identical to
   `workspace_persist_path` / `preferences_path`. No test can then write the real
   file; round-trip tests still opt in via `with_acp_persist_path`.
2. **Recover the already-lost names + harden going forward.** The server WAL is
   authoritative for a session's label (renames are persisted there via
   `SessionWal::append_rename`, recovered by `recover_one`). On a roster refresh
   (`refresh_roster` → `list_sessions`, which carries the WAL label), adopt the
   roster's label for any OPENED session whose local label is auto-generated
   (`claude-N`/empty) — `recover_labels_from_roster`, gated by
   `is_auto_claude_label` so a real custom name is NEVER overridden by a
   momentarily-stale roster. This restores names lost to the already-clobbered
   file AND makes any future acp_sessions.json corruption non-fatal.

## Approaches already tried (do NOT repeat)

- bug-0005 (dedupe duplicate/missing labels on restore) — orthogonal; it makes
  labels UNIQUE, it does not keep them from being LOST. Re-running it won't help.

---

## Log

### 2026-07-22 — closed the test-clobber hole + WAL-label recovery (FIXED)

**What changed.**
- `persist.rs::acp_session_persist_path` — now returns `None` under `cfg(test)`
  with no override (was falling through to `yalda_home()`), matching
  `workspace_persist_path` / `preferences_path`. The load-bearing fix: tests can
  no longer overwrite `~/.yalda/acp_sessions.json`.
- `agent_ui.rs::refresh_roster` — after `replace_all`, calls the new
  `recover_labels_from_roster`: adopts the roster's (WAL-backed) label for any
  opened session whose local label is auto (`is_auto_claude_label`) or empty,
  persisting via `save_agent_ring`. New free fn `is_auto_claude_label`.

**How verified.**
- `acp_persist_path_never_hits_real_home_in_tests` — asserts the path is `None`
  with no override, `Some(temp)` inside `with_acp_persist_path`, and that the
  override doesn't leak. **Negative control (RED):** restore the `yalda_home()`
  fall-through → the no-override assert returns `Some(/Users/scott/.yalda/acp_sessions.json)`.
- `renamed_session_label_round_trips_unchanged` — a custom label survives the
  real `save_persisted_acp_sessions` → `load_persisted_acp_sessions` verbatim,
  not renumbered.
- `opened_session_recovers_lost_label_from_roster` (real view) — an opened
  session carrying `claude-5` adopts the roster's real name; a session carrying a
  real custom name is left alone even when the roster differs; idempotent.
  **Negative controls (both RED):** drop the `is_auto_claude_label` gate → the
  "custom name overridden" assert fires (`stale-old-name` vs `my careful name`);
  skip the update → "a lost label was available to recover" fires.
- `is_auto_claude_label_matches_only_generated_names` — the gate predicate.
- Full suite: 399 bin + 157 lib green.

**Shipped.** Committed to `main` + release binary rebuilt (anti-circling rule 5).

**Caveat (unrecoverable data).** The names already destroyed in
`~/.yalda/acp_sessions.json` (now `{S2/claude-2}` test junk) cannot be read back
from that file. They ARE recovered on the next launch for any session still live
on the server (its WAL keeps the rename) via `recover_labels_from_roster`. A
session whose server WAL is also gone is genuinely lost and comes back `claude-N`.

**Runtime-unverified.** The end-to-end GUI↔server restore + roster round-trip is
gap #2 (no live daemon headlessly); the reducer/store/persist pieces are all
headless-green and the path guard is negative-controlled.
