# Spec: External chat bridge — drive agent sessions from Telegram (and later WhatsApp)

- **Status:** DRAFT — proposal for review. No code written.
- **Date:** 2026-07-22
- **Related:** `spec-session-server-actor.md` (single-subscriber model, admin
  surface), `spec-event-stream.md` (canonical `AgentEvent` stream + durable
  `event_log`), `spec-agent-session-ownership.md` (1:1 tile↔session), ADR-0015
  (headless start-work / `admin_prompt`), ADR-0014 (permission mode gates what an
  unattended agent may do), ADR-0009 (durable WAL), ADR-0013 (launchd — the
  always-present host).

## 1. Goal

Make a yalda agent session **interactive from two surfaces at once**: the yalda
GUI tile *and* an external chat app. From your phone you can read what an agent
is doing, reply to it, start a new agent, stop a turn — and the same
conversation is live in the desktop GUI, because both surfaces are peers over
**one session with one source of truth** (the server's durable `event_log`).

The design target is **Telegram**; the transport is abstracted so **WhatsApp**
is a later drop-in. §2 explains why Telegram ships first.

## 2. Decision: Telegram first, WhatsApp behind the same trait

| | Telegram Bot API | WhatsApp Cloud API |
|---|---|---|
| Auth to stand up | `@BotFather` → token. Minutes. | Meta Business acct + phone-number registration + app review. Days. |
| Inbound delivery | Long-poll `getUpdates` **or** webhook. Long-poll needs no public HTTPS endpoint. | Webhook only → requires a public HTTPS callback (tunnel/host). |
| Outbound freedom | Any message any time. `editMessageText` for live edits. | 24-hour customer-service window; outside it only pre-approved templates. |
| Rich controls | Inline keyboards (buttons) for permission prompts, Markdown, edited messages, forum topics. | Interactive buttons exist but are more constrained + templated. |
| Cost / fit for one user | Free, personal, zero infra. | Per-conversation pricing, business-oriented. |

For a single-user personal "agentic OS", Telegram is dramatically less friction
and its **long-poll + live message editing** map cleanly onto a streaming agent.
WhatsApp's 24h-window + template rules actively fight an assistant that speaks
whenever the agent produces output.

**Decision:** ship Telegram over long-polling first. Put every channel behind a
`ChatTransport` trait (§6) so WhatsApp — or Signal, iMessage, Slack DM — is an
implementation of that trait, not a rewrite. WhatsApp, when added, runs webhook
mode and must handle the 24h-window constraint (queue/replay + a "tap to
re-open the window" nudge); flagged, not solved, here.

## 3. Decision: the bridge lives *inside* `yalda-session-server`

This is the load-bearing architectural choice.

**Constraint that shapes it.** The server was simplified to **single-subscriber**
(`spec-session-server-actor.md`, 2026-06-11 amendment). Nuance confirmed by
reading the actor: the per-session transcript stream is a `watch` channel and is
technically **multi-consumer** — a second `attach` (an external
`connect_existing()` bridge) *would* receive the same `Agent`/`ReplyEvent`/
`TurnEnded` notifications alongside the GUI. What is **single** is the WAL-trim
**floor slot**: `ManagedSession.forwarder` is one `Option<ForwarderProgress>`, so
only one attacher's `sent_seq` pins the compaction floor. With a GUI **and** a
bridge attached to the same session, the other attacher can be **gapped** if the
log trims below its position mid-stream. So the naive "second socket client
attaches too" path is not *quite* free — it needs the trim floor generalized back
to `min` over all live forwarders (which the code's comments already anticipate;
it was the pre-simplification design). Since a server change is needed **either
way** to make concurrent GUI+bridge safe, embedding is the lower-friction change.

**Resolution.** The bridge is an **internal async task inside the
session-server process**, not a socket client. It taps the per-session event bus
**directly** — the same `push_event` / `log_tx` watch the forwarders tail — so
it observes every session with **zero forwarder contention**: the GUI keeps its
1:1 slot, and the bridge reads in-process alongside it. Inbound chat messages
inject through the existing **ungated `enqueue_prompt` core** (the `admin_prompt`
/ ADR-0015 path) — no lease, no ownership fight with an attached GUI.

Why this is the right seam, point by point:

1. **Lifecycle already matches.** The bridge must be always-on to receive a
   phone message when no GUI is running. The session-server is *already* the
   always-present launchd daemon (ADR-0013). Same process, same lifecycle, one
   thing to supervise.
2. **One source of truth, for free.** Both GUI and chat read/write the same
   `event_log` + WAL. A Telegram-sent prompt lands in the GUI transcript; a
   GUI-typed prompt lands in Telegram. No sync protocol to invent — the durable
   log *is* the sync.
3. **No revival of deleted complexity.** No second forwarder, no observer role,
   no lease. The bridge is a privileged in-process reader/writer, not a peer
   client.
4. **Crash survival.** Reuses the WAL: a bridge or server restart replays the
   log; nothing is lost.

**Alternative considered — a separate `yalda-bridge` binary (socket client).**
Genuinely viable, and the *lower-coupling* option: it reuses the clean public
`SessionServerClient` API (`connect_existing`, `create_session`, `attach`,
`admin_prompt`) with zero server-internals dependency, and the
`yalda-session-server prompt <sid> <text>` CLI subcommand already proves the
headless drive path works. Its cost: (a) to safely co-observe a session the GUI
also has open, the WAL trim floor must be generalized to `min` over all live
forwarders (see "Constraint" above) — a server change; (b) a second always-on
process to supervise, separate from the daemon that must already be always-on.
**Chosen embed over this** because the trim-floor change is required either way,
the lifecycle is identical to the server's, and one-source-of-truth is automatic
in-process. The isolation win (a bridge crash can't wedge the server) is real but
recovered cheaply by running the embedded bridge as a **supervised child task**
with panic-catch + restart (§6). If the bridge later grows heavy, promoting it
to the separate-process form is a small step — it already talks only to Manager
verbs that the public client mirrors.

## 4. What the user experiences — topic-per-session

The control plane is a **forum-enabled Telegram supergroup** (just you + the
bot; one-time setup in §4a). **Each session is a Telegram forum topic.** The
topic thread you are typing in *is* the session — routing is by
`message_thread_id`, so there is no "focused session", no `/focus`, and no
persisted focus pointer. Parallel agents are parallel threads.

**Reading an agent.** When a session produces output (whether kicked off from
the GUI or from its topic), the bridge streams a running message **into that
session's topic**: prose is coalesced and live-edited; tool calls appear as
compact status lines (`🔧 Edit src/foo.rs`, `🔧 Bash cargo test`, `✅ 42 passed`);
the message is finalized at the turn boundary. See §5 for the rendering rules.

**Replying.** A plain text message **in a session's topic** is sent as a prompt
to that session — the inbound update carries the topic's `message_thread_id`, so
the bridge routes it with no ambiguity. It streams a turn exactly as a GUI-typed
prompt would, and shows up in the GUI tile too.

**Topic lifecycle mirrors session lifecycle.**
- Session created (from GUI *or* via `/new`) → bridge `createForumTopic` →
  records the `session_id ⇄ message_thread_id` pair (§8). The GUI-created case
  means: spin up an agent at your desk, and a thread for it **appears on your
  phone automatically**.
- Session closed → `closeForumTopic` (history preserved, thread greyed out).
- Session resumed → `reopenForumTopic`.
- Session renamed → `editForumTopic` (topic name tracks the label).

**Commands** (allow-listed users only — §7). Session-scoped commands are issued
**inside the target session's topic** — the topic *is* the addressing, so no
selector argument:

| Command | Where | Effect |
|---|---|---|
| `/new <label> [cwd]` | General topic | Create a session (`CreateSession`); the bridge opens its topic. `cwd` defaults to a configured root. |
| `/sessions` | General topic | List all server-known sessions + which topic each maps to. Projects the **universal roster** (`agent_roster.rs`). Recovery aid; normally the topic list *is* the session list. |
| `/stop` | In a session topic | Cancel that session's in-flight turn (`Cancel`). |
| `/mode <read-only\|auto-edit\|yolo>` | In a session topic | Set that session's permission mode (`SetPermissionMode`, ADR-0014). |
| `/status` | In a session topic | That session: label, mode, turn state (idle / streaming), last activity. |

The **General topic** (every forum has one) is the control channel for
session-*creation* and cross-session listing; per-session control happens in the
session's own topic.

### 4a. One-time setup

1. Create a bot via `@BotFather`, get the token.
2. Create a Telegram **group**, promote it to a **forum** (enable Topics in group
   settings).
3. Add the bot and make it an **admin** with the **Manage Topics** right
   (`can_manage_topics`) — required for `createForumTopic` / `close` / `reopen`.
4. Put the token, the group's `chat_id`, and your `allowed_user_ids` in
   `~/.yalda/bridge.toml` (§8).

(The Bot API now also exposes topics in a 1:1 private chat with the bot — newer,
less battle-tested. The forum-supergroup path above is the reliable one and what
this spec targets; DM-topics can be a later transport variant.)

## 5. Streaming an `AgentEvent` stream into a chat

The bridge subscribes to the canonical `AgentEvent` stream
(`spec-event-stream.md`; `AgentEventKind`: `Chunk`, `ToolCallStarted`,
`ToolCallUpdated`, `PlanUpdated`, `TurnEnded`, `Notice`, `UserMessage`, …) and
folds it into chat messages. Chat is not a character terminal, so the fold
**coalesces**:

- **Prose (`Chunk` role=assistant).** Accumulate into a per-turn buffer. Emit /
  `editMessageText` the "assistant is typing" message on a **throttle** — the
  greater of a time floor (~1.5 s, to stay under Telegram's ~1 edit/sec/​chat
  rate limit) and a **flush trigger** (paragraph/sentence boundary, or buffer
  over N chars). Finalize (stop editing, mark done) on `TurnEnded`.
- **Tool calls.** `ToolCallStarted` → a compact one-line status appended to (or
  interleaved with) the running message: an emoji + verb + primary arg
  (`🔧 Edit src/foo.rs`). `ToolCallUpdated` with a terminal outcome → collapse to
  a result line (`✅`/`❌` + short summary). Full diffs/outputs are **not** dumped
  to chat (noise + rate limits); `/status` or the GUI is the place for detail.
- **`UserMessage` / `UserPrompt`.** A prompt that originated in the GUI (or from
  another chat) is echoed into the chat as a quoted `▶ <text>` line so the phone
  user sees the desktop-side input. The bridge suppresses the echo of a prompt
  it *itself* just injected (dedup by matching text + a short-lived pending
  marker), mirroring the GUI's own echo-suppression.
- **`Notice` / errors / `SessionDetached` / `PromptRejected`.** Surfaced as a
  distinct ⚠️ line so failures are visible on the phone, not silently dropped.
- **`TurnEnded`.** Finalize the running message; optionally append a one-line
  footer (turn #, elapsed). Ready for the next turn.

**Backfill on topic (re)open.** When a topic is created for a session (or
reopened on resume), the bridge renders a short tail of that session's
`event_log` (last turn or two) into the topic so the phone user has context —
reusing the same cursor-based replay the GUI's `attach` uses, read directly from
the in-memory log.

## 6. Components & seams

```
yalda-session-server (launchd daemon)
├─ Manager actor ............ existing: owns sessions, event_log, WAL, forwarders
│   └─ per-session event bus (push_event / log_tx watch)  ◄── bridge taps here
├─ bridge task (NEW) ........ spawned at startup iff config present
│   ├─ ChatTransport (trait) .. inbound msgs ⟶ / outbound msgs ⟵ (topic-aware)
│   │   └─ TelegramTransport ... long-poll getUpdates; sendMessage/editMessageText
│   │      (message_thread_id); createForumTopic/close/reopen/edit
│   │      (WhatsAppTransport ... webhook — later)
│   ├─ TopicRouter ............ session_id ⇄ message_thread_id map (persisted)
│   ├─ EventFolder ............ AgentEvent stream ⟶ coalesced messages into topic (§5)
│   └─ CommandParser .......... /new /sessions (General); /stop /mode /status (in-topic)
└─ socket listener .......... existing: GUI clients, CLI admin_prompt
```

- **`ChatTransport` trait** — topic-aware; the abstraction WhatsApp/Signal
  implement later (a transport without threads maps `ThreadId` to its own chat
  and ignores topic lifecycle):
  ```
  trait ChatTransport {
      async fn poll_inbound(&mut self) -> Vec<InboundMsg>;   // {chat_id, thread_id, from_user, text}
      async fn send(&self, thread: ThreadId, text) -> MessageId;   // new message in a topic
      async fn edit(&self, thread: ThreadId, MessageId, text);     // live-edit (streaming)
      async fn open_thread(&self, name) -> ThreadId;               // createForumTopic
      async fn close_thread(&self, thread: ThreadId);              // closeForumTopic
      async fn reopen_thread(&self, thread: ThreadId);             // reopenForumTopic
      async fn rename_thread(&self, thread: ThreadId, name);       // editForumTopic
      async fn send_buttons(&self, thread: ThreadId, text, options) -> MessageId; // §9
  }
  ```
  `ThreadId` wraps Telegram's `message_thread_id`. `poll_inbound` reads it off
  each update so routing is a map lookup, not stateful focus.
- **Injection path (inbound).** `InboundMsg` (non-command) → `TopicRouter`
  resolves `thread_id → session_id` → call the **existing ungated
  `enqueue_prompt` core** (same path as `Request::AdminPrompt`, ADR-0015). No new
  prompt-durability code: it's the WAL-backed path that already exists. A message
  in a thread with no mapped session (e.g. the General topic, non-command) gets a
  gentle nudge to use `/new` or a session topic.
- **Observation path (outbound).** The bridge task holds an in-process
  subscription to the Manager's event broadcast (the same signal `push_event`
  raises). It does **not** register a forwarder and does **not** count as the
  session's single subscriber (`subscriber_count` stays 0/1 = the GUI only). The
  `EventFolder` keys each session's running message by `session_id → ThreadId`
  via the `TopicRouter`, so output always lands in the right topic.
- **Topic lifecycle.** The bridge also observes the manager-wide
  `SessionCreated` / `SessionClosed` / `SessionRenamed` broadcasts (already sent
  to every connection) and drives `open_thread` / `close_thread` /
  `rename_thread` — so a session created *anywhere* (GUI included) grows a topic,
  with no per-surface coordination.
- **Supervision.** The bridge runs as a child task the Manager restarts on
  panic (catch + backoff), so a transport hiccup can't wedge the server. Absent
  config, the task is never spawned — zero cost, zero attack surface.

## 7. Security (this is a remote-code-execution surface — treat it as one)

A Yolo-mode session driven from chat can run arbitrary shell. The bridge is
therefore a hard security boundary:

- **Allowlist by chat/user id.** Config holds `allowed_user_ids`. Any update
  from a non-allowlisted Telegram user id is **dropped silently** (no reply — do
  not confirm the bot exists). No allowlist configured ⇒ bridge refuses to
  start.
- **Token handling.** Bot token from `~/.yalda/bridge.toml` (mode `600`) or env
  `YALDA_TELEGRAM_TOKEN`; never logged, never echoed.
- **Default permission mode for chat-created sessions is `read-only`.**
  Escalation to `auto-edit`/`yolo` is an explicit `/mode` command from an
  allow-listed user, per session (ADR-0014's fail-safe — an un-escalated
  session declines mutating tools even when driven headlessly).
- **Auditability.** Every chat-originated prompt is a normal `UserPrompt` in the
  WAL — the full record of "who told the agent what" survives. (Optional: add an
  `origin: Option<String>` tag to the prompt path so GUI + WAL can badge "via
  Telegram" vs "via GUI"; small additive change, §10.)

## 8. Persistence & config

- **`~/.yalda/bridge.toml`** (new):
  ```toml
  [telegram]
  token = "..."                 # or via YALDA_TELEGRAM_TOKEN
  chat_id = -1001234567890       # the forum supergroup (§4a)
  allowed_user_ids = [12345678]
  default_cwd = "/Users/scott/ws"
  ```
- **`session_id ⇄ message_thread_id` map** persists alongside the server state
  (`session_server_persist_path()`), so a restart re-binds each session to its
  existing topic instead of orphaning threads or making duplicates. This map
  replaces the v1 focus pointer entirely — routing is by topic, not focus.
  Reuses the existing state-file plumbing; honors the `YALDA_SESSION_SOCKET` test
  isolation seam. On startup the bridge reconciles: a live session with no topic
  gets one opened; a mapped topic whose session is gone gets closed.
- **Long-poll offset** (Telegram `update_id` cursor) persists so a restart
  doesn't replay old inbound messages.

## 9. Permission prompts over chat (v2 — depends on unbuilt infra)

Interactive per-tool approval does **not exist yet**: `acp_channel.rs` currently
auto-decides `session/request_permission` from the session's `PermissionMode`,
and `PermissionMode::AskEachTime` is a stub equal to `ReadOnly` (no UI to ask).

- **v1 (ships with this spec):** no interactive approvals over chat. The chat
  user controls autonomy coarsely via `/mode`. This is honest and safe.
- **v2 (target, gated on landing real inline approval):** when the interactive
  approval path lands (the `request_permission` callback parks the tool and
  asks a human), post that request **into the session's topic** as a
  `send_buttons` message (`Allow once` / `Allow always` / `Deny`) — so the
  approval sits right next to the tool call that raised it — and feed the tapped
  choice back to
  the ACP responder. The `ChatTransport::send_buttons` seam exists so v2 needs
  no transport rework — only the server-side approval plumbing that is a
  separate effort. **Flagged, not designed here.**

## 10. Wire-protocol impact

Deliberately **near-zero**, because the bridge is in-process:

- **No new socket verbs required** for v1 — the bridge calls Manager internals
  directly (`enqueue_prompt`, `create_session`, `cancel`, `set_permission_mode`)
  rather than round-tripping the socket.
- **Optional additive:** an `origin: Option<String>` on the prompt/`UserPrompt`
  path (`#[serde(default)]`, back-compatible) so both surfaces can badge message
  provenance. Not required for function.
- The GUI needs **no changes**: chat-injected prompts and turns arrive over its
  existing forwarder as ordinary `UserPrompt` / `Agent` / `TurnEnded`
  notifications and render with zero new code.

## 11. Verification plan

Per the harness protocol (`dev-system.md` §Verification harness), most of this
is headlessly testable:

- **Injection (headless, real path).** Drive the Manager's `enqueue_prompt` core
  from a simulated `InboundMsg` and assert the session's `event_log` gains the
  `UserPrompt` — reuses the pattern proven by
  `admin_prompt_drives_turn_without_owner` (`tests/session_transcript_test.rs`).
- **Event fold (headless, pure).** `EventFolder` is a **pure function**
  `Vec<AgentEvent> → Vec<ChatOp>` (send/edit/finalize). Unit-test it against
  synthetic streams (interleaved chunks + tool calls + `TurnEnded`) — no live
  agent, no network. Assert coalescing, tool-line rendering, echo-suppression,
  and finalize-on-turn-end.
- **Router / commands (headless, pure).** `CommandParser` + `TopicRouter` are
  pure over `(map, InboundMsg) → (actions, new map)`. Table-test `thread_id →
  session` routing, `/new`, in-topic `/stop`, allowlist rejection, and the
  startup reconciliation (session-without-topic, topic-without-session).
- **Topic lifecycle (headless, mocked).** Feed synthetic
  `SessionCreated/Closed/Renamed` broadcasts and assert the bridge calls
  `open_thread`/`close_thread`/`rename_thread` on the `FakeTransport` with the
  right names — proving a GUI-created session grows a topic.
- **Transport (mocked).** `ChatTransport` is a trait ⇒ a `FakeTransport`
  captures outbound (incl. topic ops) + injects inbound with `thread_id`; the
  whole bridge task is testable with no Telegram.
- **Genuine gaps (documented, per dev-system §Verification harness):** (1) the
  live Telegram long-poll loop against real Bot API ⇒ an `#[ignore]` integration
  test run with a scratch bot token; (2) the live agent subprocess loop ⇒
  existing `#[ignore]` live tests. Everything up to the network edge is
  deterministic and covered.

Negative-control every guard (assert it fails with the fix reverted) per the
anti-circling rules.

## 12. Phasing

1. **Scaffold in the server:** `bridge` module, config load, topic-aware
   `ChatTransport` trait + `FakeTransport`, spawn-iff-configured. No Telegram yet.
2. **Topic lifecycle:** `TopicRouter` + persisted `session_id ⇄ thread_id` map;
   drive `open/close/rename_thread` off the `SessionCreated/Closed/Renamed`
   broadcasts; startup reconciliation. Headless tests.
3. **Inbound → injection:** route `InboundMsg` by `thread_id → session` to
   `enqueue_prompt`. Headless tests.
4. **Outbound event fold:** `EventFolder` pure fn + tap the event bus; coalesced
   streaming with the throttle, keyed into the session's topic. Headless tests.
5. **Commands:** `/new /sessions` (General), `/stop /mode /status` (in-topic),
   allowlist gate.
6. **TelegramTransport:** long-poll `getUpdates`; `sendMessage`/`editMessageText`
   with `message_thread_id`; `createForumTopic`/`close`/`reopen`/`edit`;
   `#[ignore]` live test with a scratch bot + forum group.
7. **(v2) Permission-prompt-over-chat** once interactive approval infra lands.
8. **(later) WhatsAppTransport** behind the same trait (webhook mode + 24h
   window handling; no native topics → maps `ThreadId` to per-context chats).

## 13. Open questions

- **Streaming granularity vs rate limits:** exact throttle/flush heuristics need
  a live tune against Telegram's limits (the render is a proxy; final feel is a
  human check on-device). Note the per-topic edit-rate limits are the same as
  per-chat, so many active topics don't multiply the budget — a very busy set of
  parallel agents may need a global outbound scheduler.
- **`origin` tag:** worth the additive field to badge provenance in the GUI, or
  leave prompts un-attributed? Lean toward adding it — cheap, and "who drove
  this" is genuinely useful in a two-surface world.
- **Topic cleanup policy:** close-on-session-end keeps history but accumulates
  greyed-out threads. Offer a `/archive` or auto-delete-after-N-days? Deferred —
  close (not delete) is the safe default.
