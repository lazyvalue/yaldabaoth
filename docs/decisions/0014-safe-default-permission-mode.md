# ADR-0014: Sessions start in a safe permission mode, not Yolo

**Status:** Accepted
**Date:** 2026-06-07
**Related:** spec-session-server-actor.md § Authorization & safe defaults / Rollout phase 7, ADR-0013 (launchd — makes the server always-present, which *widens* the blast radius this ADR narrows)

## Context

`create_session` defaulted every new session's `permission_mode` to
`PermissionMode::Yolo` — auto-approve **all** gated tool calls, including shell
execution (`Execute`), file deletion (`Delete`), and external `Fetch`. Two
defaults set it: the session-server's `create_session` (`main.rs`) and the
direct `AcpChannelClient` (`acp_channel.rs`).

This is a live foot-gun, and ADR-0013 made it worse: the server is now
always-present (starts at login, restarted on crash) and listens on a Unix
socket. Any same-uid process can connect, `create_session`, and immediately
drive an agent that auto-approves shell commands — no human in the loop. Even
absent a *malicious* peer, an unattended/headless session auto-running shell is
a large default blast radius.

The trust model is single local user (per spec § Non-goals); a capability token
is explicitly **not** pursued — against a same-uid peer on a `0600` socket it is
theater (that peer can read the handshake). The load-bearing control is the
**permission mode** plus owner-only socket perms.

## Options considered

- **Keep `Yolo` default.** REJECTED: the foot-gun above; contradicts
  "escalate to power only on explicit user action."
- **Default `ReadOnly`.** Safe, but a dead-end name — it has no forward path to
  "ask the user." Functionally identical to `AskEachTime` today.
- **Default `AutoEdit`.** Allows Edit/Move/Read/Search but blocks Execute /
  Delete / Fetch. Tempting (preserves editing) but still auto-approves file
  mutation by a peer the user never authorized, and the product's core loop
  (edit→build→test) needs `Execute` anyway, so it does **not** avoid the
  escalation step — it's an awkward half-measure.
- **Default `AskEachTime`.** CHOSEN — see below.
- **Caller-specified default mode over the wire.** REJECTED: a malicious caller
  would simply send `Yolo`; it adds wire surface without adding any control in
  the single-uid model.

## Decision

1. **One source of truth.** Introduce
   `pub const DEFAULT_PERMISSION_MODE: PermissionMode = PermissionMode::AskEachTime;`
   in `acp_channel.rs` and use it at every session-creation default
   (`create_session` in `main.rs`, `AcpChannelClient` in `acp_channel.rs`).
   Flipping this one constant changes the default everywhere.

2. **Why `AskEachTime`.** It is literally the "don't auto-approve, ask the user"
   mode. Today (no inline-approval UI) it declines all gated tools — safe. When
   the approval UI lands it becomes *prompt-the-user* with **no default change
   required**. It is the only choice whose semantics already match
   "escalate to Yolo only on explicit user action."

3. **Escalation stays an explicit, owner-gated action.** Reaching `Yolo` (or
   `AutoEdit`) goes through the existing `SetPermissionMode` path, which the GUI
   drives on the user's mode-cycle keypress and which the server accepts **only
   from the session owner** (`main.rs` `set_permission_mode`).

4. **Owner-only socket, atomically.** The server socket is created `0600`. The
   mode is clamped via `umask(0o177)` *around* the `bind()` (not chmod-after-bind)
   so there is no TOCTOU window where the inode is briefly group/other-readable
   while `connect()`s already queue; an explicit `set_permissions(0o600)` follows
   as a belt-and-suspenders assertion.

## Consequences

- **Foot-gun closed:** a freshly created session no longer auto-approves shell /
  delete / fetch; an unattended or peer-initiated session is inert until a human
  escalates.
- **Behavior change (flagged for runtime review):** because the inline-approval
  UI does not exist yet, a brand-new GUI session will **silently decline**
  Edit/Write/Execute until the user cycles the mode (`<space> k m`). This is the
  intended posture, not a regression — but it changes the day-one happy path and
  should be made visible in the Claude-panel chrome (a hint that the session
  starts in `ask-each` and how to escalate) when the GUI is next touched. If the
  ergonomics are unwanted, the default is a one-line revert (flip the constant to
  `AutoEdit` or `Yolo`).
- **Recovered sessions** keep their persisted `permission_mode` from the WAL
  header (a session the user previously escalated stays escalated across restart);
  only the *creation* default changed.

## Verification (headless)

`tests/session_resilience_test.rs`:
`new_session_defaults_to_safe_permission_mode` (wire-level default is
`AskEachTime`, owner can escalate to `Yolo`), `non_owner_cannot_change_permission_mode`
(escalation is owner-gated; rejection leaves the safe default intact),
`server_socket_is_owner_only` (socket mode is `0600`). Unit tests in
`acp_channel.rs`: the default denies `Execute`/`Delete`; explicit `Yolo` allows
`Execute`.


## Addendum (2026-06-07): default reverted to Yolo, now config-driven

The safe default proved too annoying in day-to-day use without an inline-approval
UI: a brand-new session silently declined every gated tool, so the common case
(start a session, ask the agent to do work) required cycling the mode first
every single time. Pending the approval UI, the creation default is now
**`Yolo`** (auto-approve gated tools).

What changed:

- `DEFAULT_PERMISSION_MODE` is now `PermissionMode::Yolo` — the hard-coded
  fallback when nothing overrides it.
- The default is now **user-configurable** via a top-level `default-permission-mode`
  node in `config.kdl` (e.g. `default-permission-mode "auto-edit"`), parsed by
  `PermissionMode::parse` and surfaced on `Config::default_permission_mode`. The
  session server loads the config once at startup and threads it into
  `SessionManager`, so new sessions honour the configured default with the
  hard-coded constant as the ultimate fallback.

What is retained from the original decision:

- The **0600 owner-only socket** still gates who can reach the session-driving
  surface — auto-approve changes *what the owner's own sessions do*, not *who can
  drive them*.
- **Owner-gated escalation** is unchanged: only the session owner can change a
  session's permission mode.
- The **safe modes** (`ask-each` / `auto-edit` / `read-only`) remain available
  and are the **recommended default once an inline-approval UI exists** — at
  which point `ask-each` (prompt the user inline) should become the shipped
  default again. The original decision text above stands as the rationale for
  that target end state.

Verification update: `tests/session_resilience_test.rs` now pins the no-config
default as `Yolo` (`new_session_defaults_to_safe_permission_mode`,
`non_owner_cannot_change_permission_mode`, `admin_status_reports_live_sessions`);
the owner-gate and `server_socket_is_owner_only` (0600) assertions are unchanged.
`tests/config_test.rs` pins config parsing of `default-permission-mode`
(valid → parsed, invalid → error, absent → `Yolo`). `acp_channel.rs` unit tests
pin `DEFAULT_PERMISSION_MODE == Yolo`, that the safe modes still decline
`Execute`/`Delete`, and that `PermissionMode::parse` round-trips every
`short_label()`.
