# bug-0068: Astra missing from model menu

**Status:** FIXED
**First seen:** 2026-09-09
**Component:** `docs/components/agent-tile/model.md` / Codex ACP toolchain

## Symptom

`gpt-6-astra` did not appear in the switch-model menu for Codex sessions.

## Context / root cause

The first attempted fix incorrectly treated Astra as a Claude model and added
the alias `astra` to Claude-only session metadata. Astra is a Codex model whose
official model key is `gpt-6-astra`.

Yalda correctly projects the model `Select` options returned by the active ACP
adapter. The installed Codex CLI 0.150.1 and `codex-acp` 1.6.2 returned only the
five older Codex models from `model/list`, so the menu had nothing to display.
Current Codex CLI 0.154.0 and `codex-acp` 1.11.0 advertise `gpt-6-astra` and
accept it through the existing `session/set_config_option` path.

## Solution

Upgrade the Codex CLI and Codex ACP adapter, remove the incorrect Claude
allowlist entry, and guard the real authenticated Codex ACP round trip: Astra
must be present in `ModelsAvailable`, and selecting it must be confirmed as the
session's current model.

## Approaches already tried (do NOT repeat)

- Adding `astra` to Claude's `availableModels`; wrong provider and wrong key.
- Synthesizing a GUI-only option; the adapter would reject a model absent from
  its own catalog.

---

## Log

### 2026-09-09 — incorrect Claude-provider attempt

Added `astra` to Claude-only metadata and proved only its outbound
serialization. A later live Claude test correctly failed to find it in the
adapter response. The filesystem Claude setting added during that investigation
was restored from its pre-change backup; no server restart occurred.

### 2026-09-10 — corrected to Codex and verified

Official OpenAI Codex documentation identified `gpt-6-astra`. A live guard
against Codex CLI 0.150.1 / `codex-acp` 1.6.2 was RED: the returned catalog held
Sol, Terra, Luna, GPT-5.5, and Codex Spark only. After upgrading to Codex CLI
0.154.0 / `codex-acp` 1.11.0, the same real-path guard was GREEN and the adapter
confirmed a live switch to `gpt-6-astra`.
