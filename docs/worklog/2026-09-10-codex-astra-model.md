# Worklog: Codex Astra model availability

**Date:** 2026-09-10

## Cog execution evidence

- Graph id: `twl`

### Initial render

```text
graph fix-astra-missing-from-model-menu (frontiers)
frontier 0: localize [done]
frontier 1: fix-guard [claimed]
frontier 2: verify-log [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `gqi7` `localize`: claimed → closed; output: traced the menu to ACP
  `ModelsAvailable` and identified that its catalog is session-scoped.
- `8n1r` `fix-guard`: claimed → closed; output: corrected the provider, removed
  the Claude workaround, upgraded the Codex toolchain, and observed the real
  Codex guard RED before and GREEN after the upgrade.
- `6a4b` `verify-log`: claimed → closed; output: focused tests, live adapter
  round trip, release build, documentation, and worklog validation passed.
- `nm96` `omega`: claimed → closed; output: confirmed graph completion and
  activation boundary.

### Notes

- Provider correction was recorded on `8n1r`: official key is
  `gpt-6-astra`, and the implementation scope moved from Claude to Codex.
- Repository-wide `cargo fmt --check` remains RED on unrelated pre-existing
  formatting; the changed Rust test was formatted directly with `rustfmt`.

### Correction and implementation

The initial localization followed an incorrect premise that Astra belonged to
Claude. After the provider correction, official OpenAI Codex documentation was
used to identify the exact key `gpt-6-astra`. The Claude-only code and global
settings workaround were reverted.

The model menu already consumes the adapter's model `Select` options without
provider filtering. The real issue was the installed Codex toolchain's catalog:
Codex CLI 0.150.1 with `codex-acp` 1.6.2 did not return Astra from `model/list`.
Both were upgraded to current releases, Codex CLI 0.154.0 and `codex-acp`
1.11.0.

### Negative control

The authenticated live guard used Yalda's production `AcpChannelClient` Codex
path. Before the toolchain upgrade it failed because the advertised catalog
contained only Sol, Terra, Luna, GPT-5.5, and Codex Spark. After the upgrade it
passed, and `session/set_config_option` confirmed `gpt-6-astra` as the current
model.

### Verification

- `cargo test --test model_switch_live codex_advertises_astra_to_the_model_switcher_live -- --ignored --nocapture`
- `cargo test --lib acp_channel::tests::session_meta_advertises_fable_5_1_only_to_claude`
- `cargo test --test model_switch_live --no-run`
- `cargo build --release --bin yalda-gpui`
- `git diff --check`
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-10-codex-astra-model.md`

## Runtime status

Neither Yalda nor its session server was restarted. Existing sessions retain
their creation-time adapter catalog; newly created Codex sessions use the
upgraded adapter. Activating the new catalog for existing sessions requires a
separately approved session-server restart.

### Final status

- Status: `complete`

```text
graph fix-astra-missing-from-model-menu (frontiers)
frontier 0: localize [done]
frontier 1: fix-guard [done]
frontier 2: verify-log [done]
frontier 3: omega [done] (omega)
```
