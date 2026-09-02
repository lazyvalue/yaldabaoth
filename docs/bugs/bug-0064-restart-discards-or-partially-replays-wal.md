# bug-0064: restart-discards-or-partially-replays-wal

**Status:** RECURRED→FIXED
**First seen:** 2026-08-30
**Component:** `docs/components/agent-tile/session-binding.md` (`UXI-AgentTile-44`)

## Symptom

After a Yalda/session-server restart, some sessions disappear and some surviving
sessions replay only a suffix of their durable transcript. Manual repair backups
exist for earlier session-identity damage, but production recovery does not
discover them and must not guess how to merge divergent histories.

A concurrent-launch race also allowed several servers to unlink and bind the
same socket path in succession. Each recovered and resumed the same durable
sessions, producing multiple live agents with one ACP identity and multiple
processes holding the same WALs open for append.

## Context / root cause

Two independent production policies violated the durability contract:

- `session_wal::recover_one` returned `Ok(None)` for every header version other
  than the current v3, intentionally dropping the complete session on upgrade.
- `restore_seed_from_disk` called `event_log_from_recovery`, which applied the
  live in-memory cap before any client could attach. A real session with 159,874
  WAL lines was published with 37,501 events while the server logged
  `recovered_dropped=122296`.

Additional destructive risks found in the same audit are fresh creation using
`truncate(true)` on an existing identity and recovery silently skipping malformed
interior records. The v1→v2 control rename never affected WAL-appended records;
the v2→v3 event transition was additive and the current reader retains the legacy
variants, so discarding those known versions had no technical necessity.

The existing single-instance check used socket existence plus a trial connect
as a lock. That check and the later unlink/bind were not atomic. A transient
connect failure or two simultaneous launchers could both decide the pathname
was stale, unlink a live server's socket, recover the same roster, and open the
same WALs. No OS lease protected either server ownership or individual writers.

## Planned solution

Make restart recovery exact and read-only: replay every valid record from known
v1/v2/v3 WALs, remove pre-attach compaction, refuse create collisions, distinguish
only a torn final line from interior corruption, retain unsupported/corrupt files,
and prove the contract through real recovery/server-start paths plus mutation
controls. Never touch the live GUI, server, agents, or user WAL files while fixing.

Acquire a non-blocking OS lifetime lease before inspecting or unlinking the
socket and before WAL recovery or agent spawn. Keep the lease inode persistent
across shutdown. Independently lock every open WAL writer so even a future
server-ownership regression cannot create concurrent appenders.

## Approaches already tried (do NOT repeat)

- Replacing a durable prefix with `CompactedSummary` is not complete recovery;
  keeping bytes on disk while hiding them from the session is still data loss.
- Provider `session/load` is not a substitute for the exact Yalda WAL.
- Automatically selecting or merging manual backups is unsafe when active and
  backup histories may diverge.
- A socket pathname and PID text are observations, not mutual exclusion. Never
  use connect-then-unlink as the single-instance primitive.

---

## Log

### 2026-08-30 — loss paths localized; adversarial repair in progress

- Read-only live evidence confirmed 122,296 valid events omitted from one
  restart replay; no live process or WAL was modified.
- Historical-version preservation guard observed RED because v1 returned
  `Ok(None)`.
- Complete active/archived replay guard observed RED with 14 of 20 fixture
  events dropped by startup compaction, then passed with compaction removed.
- Remaining implementation and the expanded durability proof matrix are tracked
  in Cog graph `xtt`.

### 2026-08-30 — split-brain reproduced and structurally blocked

- Read-only process evidence found three Claude processes sharing ACP identity
  `9be662f6-9b47-41b8-b927-47599cb58148` and three session servers holding the
  same Fable WAL open concurrently.
- At explicit operator direction, older server processes were terminated; the
  current GUI-connected server was not restarted.
- The server now acquires an OS lifetime lease before socket/WAL/agent access.
- Every `SessionWal` create/reopen takes an exclusive writer lock.
- A real two-process regression proves the losing server cannot replace the
  socket inode and the owner continues serving; the full WAL unit matrix proves
  the second writer is refused without changing bytes.

### 2026-09-01 — post-merge follow-up: torn-tail poison sequence and headerless artifacts fixed

- Post-merge review of the strict reader landed in `8eef3b1` found two ways a
  routine crash artifact escalates into the strict reader's fail-closed path
  and bricks the **entire** server startup (every session offline until
  manual file surgery), both reproduced before fixing:
  - **Torn-tail poisoning:** crash mid-append leaves a torn unterminated
    final line (tolerated by recovery). `SessionWal::reopen` appended the
    next record directly after the torn bytes, producing one
    newline-terminated malformed line. The FOLLOWING restart's
    `recover_one` then hard-errored on it (`malformed WAL record at line
    3`), `try_recover_each` failed, and the server refused to start.
  - **Headerless crash artifacts:** `try_recover_each` turned a `recover_one`
    `Ok(None)` (zero-byte file from a crash between `create_new` and the
    header write, or a file torn on its very first line) into a fatal "WAL
    is empty or headerless" error, failing the whole roster over a file with
    no recoverable data.
- Fix: `reopen` now repairs the tail before resuming appends, under the
  writer flock — a parseable unterminated final record is newline-terminated
  (recovery already keeps it), an unparseable one is truncated to the last
  newline (bytes the durability contract already declares lost), then
  fsynced. `try_recover_each` now skips a headerless `Ok(None)` file with a
  visible `eprintln!` warning instead of failing the roster;
  newline-terminated garbage stays fatal
  (`strict_recovery_refuses_an_incomplete_roster` unchanged, still green).
- Guards: `reopen_truncates_unparseable_torn_tail_so_later_recovery_survives`,
  `reopen_terminates_parseable_unterminated_tail_and_keeps_it`,
  `strict_recovery_skips_headerless_create_crash_artifacts_instead_of_failing_the_roster`,
  `strict_recovery_skips_torn_unterminated_header_only_artifact`.
- Negative controls observed RED: torn-tail guard twice (implementing agent
  run + supervisor re-verification), both times
  `InvalidData "malformed WAL record at line 3: expected `,` or `}`"`;
  headerless-skip guard once, the fatal "WAL is empty or headerless" error.
- Cleanups in the same pass: `lock_writer` now preserves the source errno's
  `ErrorKind` instead of flattening every flock failure to `WouldBlock`; the
  stale `WalRecord::Rename` graceful-downgrade comment is rewritten to state
  the real policy (additive record variants require a `WAL_VERSION` bump
  under the strict reader); `event_log_from_recovery` collapsed to an honest
  one-arg signature, removing the always-0 `recovered_dropped` plumbing (it
  fed only a tracing field, no wire format); `write!`→`writeln!` nit for the
  pid lease.
- Suites green on branch `wal-crash-artifact-repair`: `cargo test --lib` 223
  passed; `cargo test --bin yalda-session-server` 68 passed;
  `cargo test --test session_resilience_test` 11 passed.
- Commit `6339ca5`. Cog graph `hs0` (follow-up to `xtt`): implementation nodes
  (`repair-torn-tail`, `skip-headerless-artifacts`, `wal-strictness-cleanups`,
  `collapse-dead-recovery-params`) closed done; `verify-integrate` in
  progress at time of writing (merge to `main` pending). No yalda process was
  restarted; activation is build-only and left to Scott.
- Status stays FIXED.

### 2026-09-01 — RECURRED in production: the fix had never run. Lifecycle redesign (graph f5x)

- **Recurrence.** ~19:29, seconds after a hypridle screen lock froze the GUI:
  the GUI's `connect_or_launch` reconnect path auto-launched a second
  `yalda-session-server` from `~/.local/bin` — an **Aug 27 binary predating
  `8eef3b1`** (no lifetime lease, no writer locks, old connect-then-unlink
  socket check). It stole the socket pathname from the resident server (pid
  897160, also pre-fix), recovered the same 44-session roster, and resumed
  **twin `claude --resume` workers** for ~12 ACP identities. Session
  `66dd9fad` was published with `recovered_dropped=122296`. The GUI froze and
  restarted into the new server; every old session read as frozen. With
  explicit operator approval the stale server tree (897160 + ~40 descendants)
  was SIGTERMed (clean "shutting down (WALs are durable)"); the GUI-connected
  server was untouched.
- **Why the fix didn't protect us:** it existed only in git. The Aug 30/Sep 1
  work verified in a worktree `target/`; neither `target/release` in the main
  checkout nor `~/.local/bin` was ever rebuilt (`deploy-server.sh` existed,
  never ran). Both servers in the incident were pre-fix binaries — the lease
  code had never executed on this machine. Compounding it, the lease can only
  exclude servers that *take* it: a pre-lease binary is invisible to it, and
  the GUI retained the authority to launch such a binary at any transient
  connect failure.
- **Fix (Scott's directive, ADR-0037, Cog graph `f5x`):**
  1. GUI launch authority DELETED — `connect_or_launch`/`find_server_binary`
     removed; connect/reconnect are connect-only; serverless boot restores
     tiles as reconnectable placeholders and the pump retries until the server
     appears, then re-attaches.
  2. systemd user service owns the lifecycle; `dev-server.sh` delegates to
     `deploy-server.sh` (build → atomic install to `~/.local/bin` →
     `systemctl --user restart`); the in-app Rebuild&Restart does the same
     install+restart instead of pkill+GUI-respawn — "merged but never
     installed" can no longer be silent.
  3. **Highlander guard**: the server scans `/proc` for ANY other
     `yalda-session-server` process (` (deleted)` exe suffix included — the
     stale-reinstalled case) before touching lease/socket/WAL/agents, and
     refuses to boot with a clean exit(0) (`Restart=on-failure` cannot loop
     it). `--force` for deliberate side-by-side runs (tests).
  4. Splash instruction when no server is reachable (UXI-Workspace-29).
- **Guards (all observed RED with the fix reverted, then green restored):**
  `highlander_refuses_boot_while_another_server_process_exists` (RED = the
  guard-disabled server boots alongside a live one),
  `highlander_force_boots_side_by_side`,
  `client_connect_never_launches_a_server` (RED = reintroduced spawn makes the
  socket appear), `splash_paints_start_server_instruction_when_server_missing`
  (RED = probe miss with `server_missing()` hard-wired false). Existing
  lease-layer tests keep their coverage via explicit `--force`.
- Suites on the branch: lib 223, server bin 68, resilience 14 (incl. 3 new),
  transcript 14, GUI 765 passed / 1 failed —
  `archived_waiting_session_is_removed_from_the_painted_waiting_tab`, which
  fails identically on unmodified `main` (pre-existing, live-server-state
  dependent; tracked separately). Mutation gate left to CI `--in-diff`.
- **Activation (NOT done by the agent — no process touched):** merge + release
  build + install to `~/.local/bin`, then Scott: stop the unmanaged server
  (`kill <pid>`), `./install-service.sh` once, service starts under systemd.
  The Highlander guard makes starting the new server while the old one runs a
  loud no-op instead of a split-brain.
