# 006 — TelegramTransport (real transport)

**Project:** `project.md` · **Spec:** `spec-external-chat-bridge.md` §2, §4a, §6
**Status:** Not started · **Depends on:** 001–005

## Goal

Implement the real `ChatTransport` against the Telegram Bot API so the whole
bridge works end-to-end against a live forum group.

## Approach

- Long-poll `getUpdates` with a persisted `update_id` offset (spec §8) — no
  public webhook needed.
- Outbound: `sendMessage` / `editMessageText` with `message_thread_id`.
- Topics: `createForumTopic` / `closeForumTopic` / `reopenForumTopic` /
  `editForumTopic` (bot must be admin with `can_manage_topics`, §4a).
- `send_buttons` via `InlineKeyboardMarkup` (seam for v2 §9; wire the send path,
  callback handling deferred to 007).
- Markdown parse mode for prose; graceful handling of edit-rate-limit 429s.

## Subtasks

- [ ] HTTP client + token handling (never logged)
- [ ] `getUpdates` long-poll loop + persisted offset
- [ ] `sendMessage`/`editMessageText` with `message_thread_id`
- [ ] Forum topic ops (`create`/`close`/`reopen`/`edit`)
- [ ] 429 / rate-limit backoff
- [ ] `#[ignore]` live integration test: scratch bot + forum group, round-trip a turn

## Verification

- Headless: everything below the network edge already covered by 001–005 via
  `FakeTransport`.
- Live (`#[ignore]`, run with `--ignored`): scratch bot token + forum group;
  `/new` opens a topic, a message drives a turn, reply streams back into the
  topic. Documented as genuine-gap runtime check (spec §11).
