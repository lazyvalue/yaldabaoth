# Worklog: session-server hardening (phase-7 security + observability)

**Date:** 2026-06-07
**Branches touched:** `session-hardening` (off `master` @ `f0adeb2`) — not merged, not pushed.
- `bd796d4` — safe-default permission mode + owner-only socket (ADR-0014)
- `1e2c881` — structured tracing + admin_status verb (phase-7)
- (this entry) — worklog + backlog

Picks up the `handover2.md` "Remaining work" list, items #1 (security hardening)
and the testable half of #3 (phase-7 hardening), linearized for an unattended
session. Driven via parallel scout → implement → adversarial-review workflows;
all server-side, all verified headlessly.

## Built (with status)

- **Safe-default permission mode (ADR-0014)** — `bd796d4`. New sessions no longer
  default to `Yolo` (auto-approve every gated tool incl. shell). Single source of
  truth `DEFAULT_PERMISSION_MODE = AskEachTime` in `acp_channel.rs`, used at both
  creation defaults (server `create_session` + `AcpChannelClient`). Escalation to
  `Yolo` stays an explicit, **owner-gated** `SetPermissionMode` action. ✅ tests:
  `new_session_defaults_to_safe_permission_mode`,
  `non_owner_cannot_change_permission_mode`, plus `acp_channel` unit tests
  (default denies Execute/Delete; Yolo allows Execute).
- **Owner-only socket, no TOCTOU** — `bd796d4`. Server socket forced to `0600`
  via `umask(0o177)` clamped *around* `bind()` (not chmod-after-bind), closing the
  window where the inode is briefly group/other-connectable while `connect()`s
  already queue. Belt-and-suspenders `set_permissions(0o600)` follows. Added
  `libc` dep for `umask`. ✅ test: `server_socket_is_owner_only` (asserts mode bits).
- **Structured tracing (server binary)** — `1e2c881`. `tracing` +
  `tracing-subscriber` (env-filter); fmt subscriber at startup → **stderr**, ANSI
  off, default `info` filter. All 19 server-binary `eprintln!` → leveled tracing
  with structured fields. Scoped to the binary (left `session_wal`/`session_client`
  on `eprintln` — they run in GUI/TUI with no subscriber). The four harness-grepped
  substrings preserved verbatim. ✅ regression guard: full `session_transcript_test`
  (greps the server log) passes.
- **`admin_status` verb** — `1e2c881`. Additive `Request::AdminStatus` /
  `ResponseData::AdminStatus` with `AdminSnapshot { session_count, sessions:
  [AdminSessionInfo] }` (connected, has_owner, owner_conn_id, turns, event_log_len,
  subscriber_count via broadcast `receiver_count`, channel_generation,
  permission_mode). `SessionManager::admin_status` + `SessionServerClient::admin_status`.
  Replaces eprintln-grepping for live-state diagnosis. ✅ test:
  `admin_status_reports_live_sessions`.

Build: `cargo build --bin yalda-session-server --bin yalda-acp-stub --lib` clean
(only pre-existing warnings). Tests: `session_resilience_test` (8) +
`session_transcript_test` (5) + `acp_channel`/`session_proto` lib tests all green,
`--test-threads=1`.

## Open / unresolved

- **GUI permission-UX runtime review (NEEDS-RUNTIME)** — the permission default is
  an *intended* behavior change to the GUI happy path: with no inline-approval UI
  yet, `AskEachTime` declines gated tools, so a fresh session is inert until the
  user cycles the mode (`<space> k m`). Verify it doesn't read as "broken"; consider
  a chrome hint ("session starts in ask-each; <space> k m to escalate"). One-constant
  revert if the ergonomics are unwanted (flip `DEFAULT_PERMISSION_MODE`).
- **Slow-subscriber disconnect — DEFERRED (refined).** See backlog. The shared-log
  forwarder design already largely defuses the original "slow subscriber pins
  unbounded growth" concern (subscribers re-derive their tail from the shared
  `event_log`, no per-subscriber queue; `Lagged` is graceful). Residual is a minor
  liveness issue (a forwarder parked on a permanently-dead socket), not a
  correctness/security hole. A bounded write-timeout reaper is the low-risk version,
  but it touches the load-bearing forwarder and its GUI reconnect-after-forced-
  disconnect behavior can't be runtime-verified headlessly — left for a supervised
  session.
- **Actor extraction (phase 3) — not started.** Scoped (21 lock sites, 8 mutated
  `ManagedSession` fields, the per-session pump sync↔async bridge is the crux); a
  1–2 week mechanical-but-pervasive refactor. Too large/risky to land unattended;
  the two changes above are independent of it.
- **Merge to master — NEEDS-DECISION.** All work is on `session-hardening`,
  unmerged. The permission change is `NEEDS-RUNTIME`; tracing + admin_status are
  behavior-preserving server-internal and could fold independently if the branch is
  split, but they ride the same branch today.

## Decisions

- **ADR-0014: Sessions start in a safe permission mode, not Yolo.** Chose
  `AskEachTime` over `ReadOnly`/`AutoEdit`/caller-specified — it is the only option
  whose semantics already match "ask the user / escalate on explicit action" and
  auto-upgrades to real prompting when the approval UI lands, no future default
  change. Capability token still rejected as theater for the single-uid model
  (per spec); permission mode + `0600` socket are the real controls.

## Verification status

- Fully headless-verified server-side (real `yalda-session-server` binary on a
  private socket; stub agent for the transcript path). No GPUI runtime check
  performed — the permission-UX item above is the one thing that needs human eyes.
- launchd `install`/GUI reconnect seam from the prior arc remain `NEEDS-RUNTIME`
  (unchanged by this work).

## Next

- Human runtime check of the new permission default in the GUI (and decide on a
  chrome hint vs reverting the constant).
- If keeping: consider splitting the branch so tracing + admin_status fold to
  master ahead of the permission change's runtime sign-off.
- When supervised: the slow-subscriber write-timeout reaper, then actor extraction
  (phase 3) as the next big code-quality lift (kills the shared-mutex race class
  and the poison-tolerant lock).
