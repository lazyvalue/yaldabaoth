# Worklog: headless enqueue-prompt + GUI stale-session + cursor reconnect

**Date:** 2026-06-07
**Branches (all merged to `master`, then deleted):**
- `headless-prompt` → `f3585b0`
- `gui-stale-session` → `b0f1eb2`
- `cursor-reconnect` → `a3650a4`

"Do the next 3" from the backlog, via scout→implement→adversarial-review
workflows. Server-side work verified headlessly; GUI work compile-verified +
flagged NEEDS-RUNTIME.

## Built (with status)

- **Headless enqueue-prompt verb + CLI (ADR-0015)** — `DONE` (`f3585b0`). A
  non-GUI caller can enqueue a prompt to an existing session it does NOT own and
  the agent runs the turn to completion with no GUI attached. `do_prompt` split
  into an owner-gate check + a shared owner-gate-free `enqueue_prompt` core;
  `Command::AdminPrompt` (ungated), `Request::AdminPrompt` wire verb,
  `SessionServerClient::{admin_prompt,connect_existing}`, and the
  `yalda-session-server prompt <sid> <text>` CLI subcommand. A headless prompt
  takes no lease — it enqueues onto the WAL-durable input queue (same fsync-at-
  boundary durability as the owner path) and runs under the session's stored
  permission mode. ✅ `admin_prompt_drives_turn_without_owner`; both reviews APPROVE.
- **GUI stale-session robustness** — `DONE` (`b0f1eb2`) / NEEDS-RUNTIME. When the
  GUI's persisted ACP-session list outlives the server's (server restarted with a
  fresh WAL), attach returns `no such session: <id>`; the GUI now DROPS the dead
  slot (via the existing `reconcile_session_closed` invariant-preserving removal)
  and scrubs the id from the persisted file by id across all cwd keys
  (`forget_persisted_acp_session_ids`) — so it neither shows a broken slot nor
  recurs next launch. Transient errors keep the old recoverable status. Review
  caught + fixed a persistence-leak major (single-slot-empty-ring / non-active-tab
  cases) and a doc nit. Compile-verified; GPUI not headless-drivable.
- **Cursor-based incremental reconnect (phase 5, additive)** — `DONE` (`a3650a4`).
  `Request::Attach` gains `#[serde(default)] cursor: Option<(generation,index)>`;
  when generation matches `channel_generation` and index ≤ log_len, the forwarder
  streams only the transcript tail `[index..]` instead of full replay from 0.
  Fully additive + behavior-preserving (no cursor → full replay; force-restart
  epoch bump / compaction-past-cursor → safe full-replay fallback; server-restart
  WAL recovery keeps a faithful gen-0 prefix so a gen-0 cursor correctly tails).
  `attach_with_cursor` added; `attach()` delegates with None; GUI untouched.
  ✅ `cursor_reconnect_streams_only_tail` (tail / full-replay control / mismatch);
  both reviews APPROVE.

Final master: resilience 8 · transcript 8 · server-unit 2 — all green.

## Open / unresolved (deferred next steps)

- **Phase-4 lease ownership** — `READY` but deferred. Replace `owner: conn_id`
  with `Lease{client_id, expires_at}` + heartbeat; the `OwnerChanged→LeaseChanged`
  rename is a BREAKING wire+WAL change requiring lockstep GUI updates, so it must
  not land unattended. Independent of the cursor work (cursor = replay efficiency,
  lease = ownership). Do supervised, riding the spec-event-stream §12 migration.
- **GUI cursor-wiring** — `NEEDS-RUNTIME`. Have the GUI track its last-seen
  (generation, index) per session and pass it via `attach_with_cursor` on
  reconnect, so reconnects stream only the tail. The transcript reconciler
  (agent_transcript.rs) currently assumes a full-replay rebuild — must be checked
  under a tail-only stream. GPUI not headless-drivable.
- **GUI stale-session + permission-badge-on-reconnect + in-app rebuild** — the
  GUI changes from this + prior sessions all need a human runtime pass.

## Verification status

Server-side: headless harness green throughout (incl. the race-prone reconnect/
crash/slow-subscriber tests). GUI-side (stale-session drop): compile-verified,
NEEDS-RUNTIME.

## Next

Supervised: GUI cursor-wiring (then measure tail-only reconnect), then phase-4
lease ownership (breaking migration). Then WAL compaction (now that cursor resume
has a defined epoch fallback).
