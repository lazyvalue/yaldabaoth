# 001 — Implement Cog runtime-delivery protocol v1

**Goal:** Ship the Yalda side of accepted Cog runtime-delivery v9 while keeping
live activation disabled until Cog advertises the complete capability set.

**Branch/worktree:** `yalda-cog-runtime-adapter` at
`.claude/worktrees/yalda-cog-runtime-adapter`

## Subtasks

- [x] Accept standalone v9 and record the live capability 404.
- [x] Specify placement, activation, ownership, claims, recovery, provider
      correlation, journal, retirement, shutdown, and verification.
- [x] Implement exact wire types, codecs, errors, HTTP operations, and wake SSE.
- [x] Implement the exact two-block session-manager provider bridge.
- [x] Implement durable journal and coordinator recovery/claim lifecycle.
- [x] Wire optional config, supervision, and capability-gated activation.
- [ ] Negative-control focused guards and pass full verification.
- [ ] Document, worklog, merge to main, rebuild, and runtime-check activation.

## Verification

Follow `spec-cog-runtime-adapter.md` section 9 and Cog graph `vkf`. No live
ownership transfer or dispatch is permitted while the capability endpoint is
404 or missing any required feature.
