# Worklog: Preserve Agent workspace ownership across reboot

**Date:** 2026-08-25
**Branches touched:**

- `codex/fix-reboot-detaches-agent-tile` (`6b1280f`) — boot-order fix,
  production-path regression, and durable contract
- `main` (`77c2022`) — merged implementation and release verification

## Cog execution evidence

- Graph id: `d14`

### Initial render

Shown before tracked implementation edits:

```text
graph fix-reboot-detaches-agent-tile (frontiers)
frontier 0: prove-race [open]
frontier 1: record-contract [open]
frontier 2: fix-and-guard [open]
frontier 3: verify [open]
frontier 4: integrate-log [open]
frontier 5: omega [open] (omega)
```

### Node execution

- `kexv` `prove-race`: claimed → closed; output: localized the real boot race
  from `refresh_roster` through Detached materialization and the pre-restore
  `workspace.json` save, corroborated by the reported Outlook state.
- `uwtn` `record-contract`: claimed → closed; output: added bug-0059 and
  `UXI-Workspace-28`, requiring durable `Frame` ownership before roster startup.
- `1uyb` `fix-and-guard`: claimed → closed; output: production boot restores
  workspace ownership first; the focused guard passed and the reversed-order
  negative control reproduced the empty-Outlook/Detached overwrite.
- `sm8d` `verify`: claimed → closed; output: 714 full-suite tests passed, the
  in-diff production-helper mutant was caught, and `git diff --check` passed.
- `lsnu` `integrate-log`: claimed → closed; output: implementation commit
  `6b1280f` merged to `main` as `77c2022`; the focused guard passed again from
  primary `main`, and the release GUI rebuilt and restarted as one process.
- `w2wt` `omega`: claimed → closed; output: reboot ownership fix is merged,
  active, recorded, and verified; existing corrupted membership retains a
  documented one-time manual repair boundary.

### Notes

- Node `sm8d`, seq `3`, topic `deviation`: `cargo fmt --all -- --check`
  encounters extensive pre-existing repository-wide rustfmt drift outside this
  patch. Changed hunks pass `git diff --check` and were inspected directly.

### Final status

- Status: `complete`

```text
graph fix-reboot-detaches-agent-tile (frontiers)
frontier 0: prove-race [done]
frontier 1: record-contract [done]
frontier 2: fix-and-guard [done]
frontier 3: verify [done]
frontier 4: integrate-log [done]
frontier 5: omega [done] (omega)
```

## Built (with status)

- `SHIPPED`: no-argument GUI boot restores `workspace.json` before it starts
  the universal session pump or roster seed. An immediately returning roster
  therefore reconciles against the durable ownership graph and cannot overwrite
  an Attached Agent as Detached.
- `SHIPPED`: `boot_restores_attached_agent_before_fast_roster_save` drives the
  same production initializer with a persisted Outlook Agent and immediate
  roster result, asserting both live and on-disk ownership of stable tile 1175.
- `SHIPPED`: bug-0059 and `UXI-Workspace-28` record the cause, durability
  boundary, enforcement, and repair limit.
- `ACTIVE`: `./dev-gui.sh` rebuilt and restarted the release GUI from merged
  `main`; exactly one release process was observed and it reconnected to the
  existing session server.

## Open / unresolved

- The fix cannot reconstruct a named-workspace choice already erased from the
  saved ownership graph. The reported `outlook lead` tile remains Detached until
  it is explicitly sent to `Outlook` once; future reboots then preserve it.
- Repository-wide rustfmt drift predates this patch and remains outside scope.

## Decisions

- No ADR added. The component-local boot and durable-membership rule is owned by
  `UXI-Workspace-28`.
- Session cwd is not used to infer a workspace: it identifies only the project,
  which can contain multiple named workspaces.

## Verification status

- Focused guard on the isolated branch: **1 passed**.
- Required negative control: reversing restore/roster order failed at `Outlook
  remains present after boot`; restoring the production order returned GREEN.
- Full isolated-branch `cargo test --bin yalda-gpui`: **714 passed, 0 failed, 2
  ignored**.
- `cargo mutants --in-diff ...`: **1 production-helper mutant tested, 1 caught**;
  the OS `main` replacement was excluded because the entry point is not callable
  headlessly.
- Focused guard from primary `main`: **1 passed**.
- Primary `main` release build and GUI restart: passed; one release process
  (`yalda-gpui`) reconnected to the existing server.
- `git diff --check`: passed.
- `scripts/check-cog-worklog.sh docs/worklog/2026-08-25-reboot-agent-ownership.md`
  passes.

## Next

- Send `outlook lead` to `Outlook` once in the running GUI, then use ordinary
  reboot/restart behavior; the persisted membership will now survive roster
  reconciliation.
