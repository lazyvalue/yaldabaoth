# 003 — Inbound → injection

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §6
**Status:** Not started · **Depends on:** 001, 002

## Goal

A plain-text message in a session's topic drives a turn on that session, over the
existing WAL-durable, ungated `enqueue_prompt` core (the ADR-0015 `admin_prompt`
path) — no new prompt-durability code, no ownership/lease fight with an attached
GUI.

## Approach

- `InboundMsg { chat_id, thread_id, from_user, text }` → `TopicRouter` resolves
  `thread_id → session_id` → call the Manager's `enqueue_prompt(session_id, text)`
  in-process.
- A message in a thread with no mapped session (General topic, non-command) ⇒
  gentle nudge to use `/new` or a session topic (no injection).
- Suppress echo of a prompt the bridge itself injected (short-lived pending
  marker + text match), mirroring GUI echo-suppression.

## Subtasks

- [x] Route `InboundMsg` (non-command) by `thread_id → session` to `enqueue_prompt`
- [x] Unmapped-thread handling (nudge, no injection)
- [ ] Self-injected-prompt echo suppression marker — DEFERRED to T-004 (needed once
      the outbound fold echoes user prompts back into the topic)
- [x] Headless test: simulated inbound routes to the driver's `admin_prompt`
      (handler-level with `FakeDriver`; the real-`event_log` E2E is the T-006 live test)

## Verification

- Headless real path: assert the injected prompt appears in the session's durable
  `event_log`. Negative control: break the router lookup, assert red.
