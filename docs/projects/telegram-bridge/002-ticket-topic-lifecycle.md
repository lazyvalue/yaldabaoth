# 002 — Topic lifecycle: TopicRouter + persisted map + reconcile

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §4, §6, §8
**Status:** Not started · **Depends on:** 001

## Goal

Make each session own a Telegram topic, driven entirely off the manager-wide
`SessionCreated / SessionClosed / SessionRenamed` broadcasts — so a session
created **anywhere** (GUI included) grows/updates/closes a topic with no
per-surface coordination. Persist the `session_id ⇄ message_thread_id` map and
reconcile it on startup.

## Approach

- `TopicRouter`: bidirectional `session_id ⇄ ThreadId` map; pure over
  `(map, event) → (transport actions, new map)`.
- Subscribe the bridge to the existing manager `broadcast::Sender<Notification>`
  (already sent to every connection) and translate:
  `SessionCreated → open_thread` (name from label), `SessionClosed →
  close_thread`, `SessionRenamed → rename_thread`.
- Persist the map alongside `session_server_persist_path()` (honors
  `YALDA_SESSION_SOCKET` seam). Replaces any focus pointer.
- **Startup reconcile:** live session with no mapped topic ⇒ `open_thread`;
  mapped topic whose session is gone ⇒ `close_thread`.

## Subtasks

- [x] `TopicRouter` type + bidirectional map + pure transition fn
- [x] Subscribe bridge to manager broadcast; map create/close/rename → transport ops
- [x] Persist `session_id ⇄ thread_id` map (state-file plumbing + test seam)
- [x] Startup reconciliation (session-without-topic, topic-without-session)
- [ ] Backfill: on topic open/reopen, render last-turn tail from `event_log` — NOT DONE (needs the T-004 event tap / event_log read)
- [x] Headless tests: broadcasts → correct `FakeTransport` topic ops; reconcile both directions

## Verification

- Headless (mocked transport): feed synthetic `SessionCreated/Closed/Renamed`,
  assert `open/close/rename_thread` called with right names + map updated.
- Negative control: revert the create→open wiring, assert the test goes red.
