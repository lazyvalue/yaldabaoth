# 005 — Commands + allowlist

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §4, §7
**Status:** Not started · **Depends on:** 003, 004

## Goal

Slash-command control plane + the security boundary. Commands are locale-scoped:
`/new`, `/sessions` in the General topic; `/stop`, `/mode`, `/status` inside a
session's own topic (the topic is the addressing — no selector arg).

## Approach

- `CommandParser`: pure over `(InboundMsg) → Command`. Dispatch:
  - `/new <label> [cwd]` → `create_session` (cwd defaults to configured root),
    default permission mode **read-only** (§7 fail-safe). 002 opens its topic.
  - `/sessions` → list roster + topic mapping (recovery aid).
  - `/stop` (in-topic) → `cancel` that session's turn.
  - `/mode <read-only|auto-edit|yolo>` (in-topic) → `set_permission_mode`.
  - `/status` (in-topic) → label, mode, turn state, last activity.
- **Allowlist gate FIRST:** update from a non-allowlisted `from_user` is dropped
  silently (no reply). No allowlist configured ⇒ bridge refuses to start (enforce
  in 001 config load; assert here).

## Subtasks

- [x] `CommandParser` pure fn + `Command` enum
- [x] `/new` (default read-only mode) + `/sessions`
- [x] In-topic `/stop`, `/mode`, `/status`
- [x] Allowlist gate (silent drop) + refuse-start-without-allowlist assertion
- [x] Headless table tests: parsing, locale rules, allowlist rejection

## Verification

- Headless: table-test each command + rejection of non-allowlisted user.
  Negative control: disable the allowlist check, assert a stranger's `/mode yolo`
  would go through → test red.
