# 004 — Outbound event fold

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §5, §6
**Status:** Not started · **Depends on:** 001, 002

## Goal

Fold a session's `AgentEvent` stream into coalesced, live-edited Telegram
messages **in that session's topic** — prose streamed via throttled edits, tool
calls as compact status lines, finalized at the turn boundary.

## Approach

- `EventFolder`: **pure fn** `Vec<AgentEvent> → Vec<ChatOp>` (`Send`/`Edit`/
  `Finalize`, each carrying a `ThreadId`). Keep it pure so it's fully unit-testable.
- Tap the per-session event bus in-process (same signal `push_event` raises); do
  NOT register a forwarder (`subscriber_count` stays = GUI only).
- Coalescing rules (spec §5): prose buffer + throttle (≥~1.5s or sentence/para
  boundary or N chars); `ToolCallStarted/Updated` → one-line status/result;
  `Notice`/errors/`SessionDetached`/`PromptRejected` → ⚠️ line; `TurnEnded` →
  finalize.
- Key each running message by `session_id → ThreadId` via `TopicRouter`.

## Subtasks

- [x] `EventFolder` pure fn + `ChatOp` type
- [x] Coalescing + tool-line + finalize rules (prose buffer, `🔧` tool lines, Post/Edit/Finalize).
      NOTE: no time-based throttle yet — every content event emits an Edit; a Telegram
      edit-rate throttle is deferred (fine for the fake, matters live)
- [ ] Echo-suppression for bridge-injected user prompts — DEFERRED (needs the fold to
      also surface `UserMessage`/`UserPrompt`; currently only assistant `Chunk`s render)
- [x] Tap event bus; drive transport `send`/`edit` per `ChatOp`, keyed to topic
- [x] Headless pure tests: synthetic streams (interleaved chunks+tools+TurnEnded)
- [ ] Global outbound scheduler for many-topic rate budget — NOT DONE (deferred, §13)

## Verification

- Headless pure: assert coalescing, tool rendering, finalize-on-turn-end, correct
  `ThreadId` targeting. Negative control per changed rule.
