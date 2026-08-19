# Worklog — yalda-control MCP (`create_session` + per-session injection)

Date: 2026-08-18
Branch: `yalda-control-mcp`

## What was built

An MCP server, `yalda-mcp`, that lets an agent control Yalda, plus the wiring
that makes every agent session Yalda spawns carry that MCP server automatically.

- **`src/bin/yalda-mcp.rs`** — a hand-rolled MCP stdio JSON-RPC server (modeled
  on `yalda-channel`): `initialize` / `notifications/initialized` / `tools/list`
  / `tools/call` / `ping`, single stdout-writer thread, optional `YALDA_MCP_LOG`.
  One tool, **`create_session`**, with inputSchema
  `{ agent: enum[claude,codex] (required), prompt: string (required), cwd?, label? }`.
  Handler: `SessionServerClient::connect_existing()` →
  `create_session_with_provider(cwd, label, provider, None)` →
  `admin_prompt(session_id, prompt)` (ADR-0015 headless enqueue — no ownership,
  definitive Ack). Returns the new session id; a missing server or an invalid
  agent produces a graceful `isError` tool result, not a crash.
- **`Cargo.toml`** — new `[[bin]] yalda-mcp`.
- **`src/acp_channel.rs`** — `yalda_mcp_servers()` + `yalda_mcp_binary_path()`
  (resolves `yalda-mcp` sibling-of-exe, bare-name PATH fallback, empty vec if
  unresolved). Applied `.mcp_servers(yalda_mcp_servers())` to both
  `NewSessionRequest::new(cwd)` sites and the `LoadSessionRequest::new(...)` site
  inside `worker_async`. Provider-agnostic — reaches **both** Claude and Codex,
  on every create / resume / respawn / branch path.
- **README** — new "Controlling Yalda from an agent (`yalda-mcp`)" section +
  binary listed under Build.

## Why this injection point

`NewSessionRequest`/`LoadSessionRequest.mcp_servers()` is the single provider-
agnostic lever. Configuring via `.claude/settings.json` / `agent_meta()` would
reach Claude only — Codex gets an empty `_meta`. All spawn paths funnel through
`worker_async`, so the three construction sites cover every case. See ADR-0030
(provider is durable session identity).

## Tests (real-path, negative-controlled)

- `tests/yalda_mcp_test.rs` (4 tests) — spawns the **real** `yalda-mcp` binary
  and drives stdio: `initialize` reports `serverInfo.name=yalda`; `tools/list`
  exposes `create_session` with agent enum `[claude,codex]` and `agent`+`prompt`
  required; `create_session` with no server reachable → graceful `isError`;
  unknown agent → `isError`. **4/4 pass.**
- `src/acp_channel.rs` unit tests — `yalda_mcp_servers_yields_one_named_stdio_server`
  (exactly one Stdio server named `yalda` → `yalda-mcp`) and
  `new_session_request_serializes_yalda_mcp_server` (serialized request JSON
  carries `mcpServers[0].name=yalda` with a stdio command). **Both pass (lib
  19 passed).**

### Negative controls (observed RED, then restored)

- Injection helper → `Vec::new()`: both unit tests FAILED at
  `acp_channel.rs:2883/2908`; restored → GREEN.
- Dropped `codex` from the tool enum, rebuilt the binary:
  `tools_list_exposes_create_session` FAILED ("agent enum should offer claude +
  codex: [claude]"); restored → GREEN.

## Build

`cargo build` (full workspace) and the targeted test runs are green. Built
against the shared `target/` (worktree disk was full; the per-worktree target
was removed and the build reuses the main dep tree via `CARGO_TARGET_DIR`).

## Runtime caveat (NEEDS-RUNTIME — genuine gap #2)

The live end-to-end loop (an in-Yalda Claude/Codex agent actually calling
`mcp__yalda__create_session` and a new tile appearing) needs a running agent on
PATH + auth and the GUI; not headlessly reproducible. Headless coverage proves
the tool schema, the graceful-failure path, and that the MCP is on the wire for
every session. The full round-trip should be smoke-checked in the running app.

## Cog execution evidence

- Graph id: `746`

### Initial render

```
graph yalda-control-mcp (frontiers)
frontier 0: scaffold-mcp-bin [open]
frontier 1: inject-mcp-into-sessions [open], test-mcp-handshake [open]
frontier 2: test-injection [open]
frontier 3: verify-and-worklog [open]
frontier 4: omega [open] (omega)
```

### Node execution

Each node was claimed with actor `claude-code` and closed `done` with a JSON
output after its acceptance criteria were verified:

- `scaffold-mcp-bin` (et74) — claimed, closed with output (binary + tool + bin
  entry; build Finished; 4/4 integration).
- `inject-mcp-into-sessions` (aofz) — claimed, closed with output (helper + 3
  request sites; negative control RED→GREEN).
- `test-mcp-handshake` (vyfl) — claimed, closed with output (4/4; enum negative
  control RED→GREEN).
- `test-injection` (ngnn) — claimed, closed with output (2 unit tests; helper
  negative control RED→GREEN).
- `verify-and-worklog` (u6zv) — claimed, closed with output (full build + tests
  + README + this worklog).
- `omega` (gfpn) — claimed and closed to confirm the whole graph.

### Notes

- Graph note (topic `decision`): the injection-point rationale
  (`mcp_servers()` over `settings.json` for provider-agnostic reach; `yalda`
  stdio server; `admin_prompt` for the initial prompt).

### Final status

- Status: `complete`

```
graph yalda-control-mcp (frontiers)
frontier 0: scaffold-mcp-bin [done]
frontier 1: inject-mcp-into-sessions [done], test-mcp-handshake [done]
frontier 2: test-injection [done]
frontier 3: verify-and-worklog [done]
frontier 4: omega [done] (omega)
```
