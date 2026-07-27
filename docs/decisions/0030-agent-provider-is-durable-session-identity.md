# ADR-0030: Agent provider is part of durable session identity

**Status:** Accepted
**Date:** 2026-07-26
**Related:** `UXI-AgentTile-30`, `AgentProvider`, ADR-0003, ADR-0012, ADR-0025

## Context

Yaldabaoth previously had one process-wide ACP command. That can select one
adapter, but cannot run Claude and Codex beside each other, and it cannot know
which adapter should resume a persisted session after a server restart.

Codex also supports two materially different authentication paths: interactive
ChatGPT login (subscription/plan limits) and API-key authentication (metered API
billing). The requested default is the former.

## Decision

1. `AgentProvider` (`claude` or `codex`) travels on create requests and session
   snapshots, is stored on the managed session and WAL header, and defaults to
   Claude when absent for additive compatibility.
2. Spawning is per session. Claude resolves
   `YALDA_CLAUDE_ACP_AGENT` (then legacy `YALDA_ACP_AGENT`) and Codex resolves
   `YALDA_CODEX_ACP_AGENT`; their defaults are `claude-agent-acp` and `codex-acp`.
3. Restart, recovery, clear, and cwd respawn preserve the stored provider.
4. Codex adapter children inherit the user's cached `codex login`, but not ambient
   API-auth environment variables. `YALDA_CODEX_ALLOW_API_KEY=1` is the explicit
   escape hatch for users who intentionally want metered API authentication.
5. Claude-only ACP `_meta` extensions are omitted from Codex session requests.

## Consequences

- A single server can own concurrent Claude and Codex sessions.
- Old clients and pre-field WAL records remain Claude sessions.
- `codex-acp` is an external runtime prerequisite; Yaldabaoth reports an install
  hint when it is absent.
- A user's Codex subscription and rate limits remain governed by their ChatGPT
  plan. Yaldabaoth does not proxy or store their account credentials.
