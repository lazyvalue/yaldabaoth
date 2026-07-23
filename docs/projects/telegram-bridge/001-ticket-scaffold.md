# 001 — Scaffold: bridge module + ChatTransport trait

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §6, §8
**Status:** Not started · **Depends on:** none

## Goal

Stand up the empty-but-wired bridge inside `yalda-session-server`: a module that
loads config, defines the topic-aware `ChatTransport` trait, ships a
`FakeTransport` for tests, and spawns a supervised bridge task **iff** config is
present. No Telegram, no routing yet — just the skeleton everything else hangs
off.

## Approach

- New module `src/bin/yalda-session-server/bridge/` (or `bridge.rs`).
- Config load from `~/.yalda/bridge.toml` + env (`YALDA_TELEGRAM_TOKEN`); absent
  config ⇒ task never spawned (zero cost).
- `ChatTransport` trait per spec §6 (topic-aware: `poll_inbound`, `send`, `edit`,
  `open/close/reopen/rename_thread`, `send_buttons`; `ThreadId` newtype).
- `FakeTransport`: captures outbound ops (incl. topic ops) into a log, injects
  scripted inbound with `thread_id`.
- Spawn as a supervised child task (panic-catch + backoff restart) from the
  Manager startup; honor `YALDA_SESSION_SOCKET` test isolation (no `~/.yalda`
  writes under `cfg(test)`).

## Subtasks

- [x] Create `bridge` module + wire into server startup (spawn-iff-configured)
- [x] `BridgeConfig` load from `bridge.toml` + env; `None` ⇒ no spawn; test-isolation seam
- [x] `ChatTransport` trait (topic-aware) + `ThreadId`/`InboundMsg`/`MessageId` types
- [x] `FakeTransport` capturing outbound + injecting inbound
- [ ] Supervised-task wrapper (panic-catch + backoff) — DEFERRED: currently a plain `tokio::spawn`; a bridge panic dies with the task and the server survives (transport errors are already caught + logged), but there is no auto-restart-with-backoff yet
- [x] Headless test: configured ⇒ task spawns; unconfigured ⇒ no spawn, no `~/.yalda` write

## Verification

- Headless: assert spawn-iff-configured both ways (negative control: unconfigured
  must NOT spawn and must NOT touch `~/.yalda`).
- `cargo check` + `cargo test --bin yalda-session-server` (or crate tests) green.
