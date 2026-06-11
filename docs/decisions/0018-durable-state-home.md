# ADR-0018: Durable state lives in `~/.yalda`, not the OS cache dir

**Status:** Accepted
**Date:** 2026-06-08
**Related:** ADR-0009 (durable session log / WAL), ADR-0016 (ringbuffer compaction),
ADR-0017 (WAL version-discard migration), `src/paths.rs`

## Context

All of yalda's persisted state lived under `dirs::cache_dir().join("yalda")` —
`~/Library/Caches/yalda` on macOS, `~/.cache/yalda` on Linux. That includes
**durable, hard-to-regenerate** data:

- `wal/` — per-session write-ahead logs (the agent conversation history; ADR-0009)
- `session_server.json` — persisted session list the server replays on restart
- `acp_sessions.json`, `workspace.json`, `preferences.json` — GUI persisted state
- `client_id` — the stable per-install id the lease protocol keys on
- plus disposable logs (`session-server.log`, `debug.log`)

The OS cache directory is **purgeable by design**: macOS evicts `~/Library/Caches`
under disk-space pressure, "Manage Storage" and third-party cleaners wipe it, and
nothing guarantees it survives. Storing agent-session history there means a cache
purge silently destroys conversations — a durability bug masquerading as a path
choice. (It is also why `dev-all.sh` historically *claimed* to "drop sessions":
that comment predated the WAL and was never about the cache dir at all — the WAL
is never deleted by the dev scripts; only a `WAL_VERSION` bump discards it.)

## Decision

Put all persisted yalda state under a single durable home: **`~/.yalda`**.
One helper, `paths::yalda_home()` (`dirs::home_dir().join(".yalda")`), is the
sole source of that base path; every persist site calls it. A one-time,
idempotent, best-effort `paths::migrate_legacy_cache_dir()` runs at the top of
both binaries' `main()` and relocates any pre-existing `<cache_dir>/yalda/*`
into `~/.yalda` (never clobbering a name the new home already owns), so the move
loses nothing.

Unchanged:
- **Config** stays at `~/.config/yalda/config.kdl` (XDG config dir) — it's
  user-authored config, not runtime state, and `~/.config` is already durable.
- **Sockets / pid** stay in `/tmp` (`session_proto::socket_path`) — runtime-only,
  correctly disposable, and `/tmp` is the right place for IPC endpoints.
- The `YALDA_SESSION_SOCKET` override branches (isolated/blue-green instances)
  still derive WAL/state paths from the socket path, so test and alternate
  instances never share durable state.

## Rationale

Durability is a property of *where* the bytes live, and the cache dir is the one
location the OS is explicitly allowed to delete. `~/.yalda` is a conventional,
user-visible dotfolder under `$HOME`, on the same volume as the cache dir (so the
migration is a cheap `rename`), and is not subject to cache eviction. A single
`yalda_home()` chokepoint means the location can't drift across the ~9 sites
that previously open-coded `cache_dir().join("yalda")`.

Rejected: `~/Library/Application Support/yalda` (macOS-canonical for app data) —
correct on macOS but platform-specific and less discoverable; the user asked for
a single cross-platform `~/.yalda`, which is simpler and equally durable.

## Consequences

- Agent-session history survives OS cache purges, "Manage Storage", and reboots.
- Logs move too (consolidated home); the TUI debug-overlay path is now
  `~/.yalda/debug.log` (CLAUDE.md updated).
- Existing users' state is migrated automatically on first run of the new build.
  A `rename` failure across mounts (EXDEV, rare) leaves the file in the legacy dir
  with a log line rather than losing it — that file simply starts fresh.
- Dev scripts (`dev-all.sh`, `scripts/rebuild-server.sh`) point at `~/.yalda`;
  the stale "drops all sessions" note in `dev-all.sh` is corrected (sessions
  survive — only a `WAL_VERSION` bump discards them).
