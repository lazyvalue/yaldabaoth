# Project: External chat bridge — drive agent sessions from Telegram

- **Status:** PLANNED — spec approved, no code written.
- **Owner:** Scott
- **Spec:** `docs/specs/spec-external-chat-bridge.md` (authoritative design)

## Problem / Why

Agent sessions are only interactive from the desktop GUI today. You want to
read, reply to, start, and stop agents **from your phone** — and have the same
conversation live in both places at once. The session already lives in the
always-on `yalda-session-server` daemon, decoupled from any window showing it,
so a phone surface is just a second view onto the same server-side session.

## Goals

- A Telegram surface where **each agent session is a forum topic**; the thread
  you type in *is* the session (no focus juggling).
- **Bidirectional, one source of truth:** a phone message lands in the GUI
  transcript and vice-versa, because both read/write the same durable
  `event_log` + WAL. No sync protocol to invent.
- **Zero GUI changes** and **near-zero wire-protocol changes** — the bridge is
  an in-process task in the session-server.
- Transport abstracted behind a `ChatTransport` trait so WhatsApp/Signal are
  later drop-ins.

## Scope

**In:**
- An in-process `bridge` module in `yalda-session-server`, spawned iff configured.
- Topic-per-session lifecycle driven off existing `SessionCreated/Closed/Renamed`
  broadcasts.
- Inbound routing (`thread_id → session`) → the ungated `enqueue_prompt` core
  (ADR-0015 path); outbound `AgentEvent` fold → coalesced live-edited messages
  into the session's topic.
- Commands: `/new`, `/sessions` (General topic); `/stop`, `/mode`, `/status`
  (in-topic). Allowlist security.
- `TelegramTransport` over long-poll; `FakeTransport` for headless tests.

**Out (deferred):**
- Interactive per-tool permission approval over chat (v2 — depends on unbuilt
  server-side `request_permission` approval infra; today `AskEachTime` is a stub).
- `WhatsAppTransport` (webhook mode + 24h-window handling).
- Topic auto-cleanup/archival policy (default: close, not delete).

## Model / Key decisions

1. **Bridge lives INSIDE `yalda-session-server`**, not as a separate process.
   The server is the always-on launchd daemon that owns sessions + WAL + event
   bus — the bridge's lifecycle == the server's. It taps the per-session event
   bus in-process (no `attach`, no forwarder contention: the GUI keeps its 1:1
   slot). Rejected the separate-process socket-client form because concurrent
   GUI+bridge on one session needs the WAL trim-floor generalized to
   `min`-over-forwarders **either way**, and in-process gives one-source-of-truth
   for free. (Spec §3.)
2. **Topic-per-session**, not a single focused chat. Standard practice for
   bridge/relay bots. Routing is a `session_id ⇄ message_thread_id` map lookup,
   not stateful focus. Requires a forum-enabled supergroup + bot admin with
   Manage Topics. (Spec §4, §4a.)
3. **Telegram first** (BotFather token, long-poll, live message-edit). WhatsApp
   fights an always-speaking assistant with its 24h window; behind the trait for
   later. (Spec §2.)
4. **Security is a hard boundary** — this is a remote-code-execution surface.
   Allowlist by Telegram user id, refuse to start without one, chat-created
   sessions default to `read-only`, every prompt is WAL-audited. (Spec §7.)

## Links

- Spec: `docs/specs/spec-external-chat-bridge.md`
- ADR-0015 (headless start-work / `admin_prompt` — the ungated inject path)
- ADR-0014 (permission mode gates unattended agents)
- ADR-0009 (durable WAL), ADR-0013 (launchd host)
- `spec-session-server-actor.md` (single-subscriber model), `spec-event-stream.md`
- Session server: `src/bin/yalda-session-server/main.rs`; proto:
  `src/session_proto.rs`; ACP: `src/acp_channel.rs`; event: `src/agent_event.rs`

## Tickets

Branch: `telegram-bridge` (worktree `.claude/worktrees/telegram-bridge`), through
commit `9b6371b`. **47/47 session-server unit tests green, warning-free**; the fold,
allowlist, and command-dispatch guards are all negative-controlled. Live Telegram
test is `#[ignore]` (needs a real bot + forum group). Not yet merged to `main`; not
yet runtime-verified against live Telegram.

| # | Ticket | Status | Notes |
|---|---|---|---|
| 001 | Scaffold: `bridge` module + `ChatTransport` trait + `FakeTransport` + spawn-iff-configured | **Done** | `bridge/{mod,transport,router,telegram,tests}.rs`; config gate + `SessionDriver` seam |
| 002 | Topic lifecycle: `TopicRouter` + persisted map + open/close/rename off broadcasts + startup reconcile | **Done** | Pure router + `handle_event` + JSON persistence + reconcile |
| 003 | Inbound → injection: route `thread_id → session` to `admin_prompt` | **Done** | allowlist gate (neg-controlled); General-topic nudge |
| 004 | Outbound event fold: `EventFolder` pure fn + tap event bus + coalesced streaming | **Done** | `fold.rs` (Post/Edit/Finalize) + `push_event` tap (Option-gated, hot-path-safe) + `handle_transcript` |
| 005 | Commands + allowlist: `/new /sessions /stop /mode /status` | **Done** | `command.rs` pure parse + locale dispatch; `/new` defaults read-only |
| 006 | `TelegramTransport`: long-poll + sendMessage/edit + forum topic ops + `#[ignore]` live test | **Done (code)** | `telegram.rs` on `ureq`+`spawn_blocking`; `tests/telegram_bridge_live.rs` `#[ignore]` — live run pending user creds |
| 007 | (deferred) Permission-prompt-over-chat (v2) | Deferred | Blocked on server-side approval infra |
| 008 | (deferred) `WhatsAppTransport` | Deferred | Behind the trait |

**Still open (deferred, not blocking):** topic backfill on open, self-echo
suppression, supervised restart-with-backoff, and the live end-to-end run against a
real bot. See tickets 001/002/003 for the specific unchecked subtasks.
