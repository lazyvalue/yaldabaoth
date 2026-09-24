#!/usr/bin/env bash
# Add a Claude model to Yalda's per-session model picker — no code change, no
# recompile, no process restart.
#
# Yalda advertises a Claude model list on every `session/new` / `session/load`:
# the compiled defaults (`YALDA_CLAUDE_AVAILABLE_MODELS` in src/acp_channel.rs)
# merged with an optional per-install file of extra model ids, one per line. This
# script appends an id to that file (de-duplicated). The next Claude session you
# open picks it up. See docs/components/agent-tile/model.md (UXI-AgentTile-16).
#
# Usage:
#   scripts/yalda-add-claude-model.sh <model-id> [--refresh-adapter]
#
# Examples:
#   scripts/yalda-add-claude-model.sh claude-opus-6
#   scripts/yalda-add-claude-model.sh 'claude-opus-6[1m]' --refresh-adapter
#
# The config file is $YALDA_CLAUDE_MODELS if set, else
# ~/.config/yalda/claude-models.conf.
#
# Notes:
#  - The ACP adapter is authoritative for labels/ordering, but it does NOT drop
#    an id it doesn't recognize: an unknown id is surfaced verbatim and passed to
#    the API on switch. So a brand-new model works immediately; only its display
#    label lacks polish until the adapter's SDK ships an entry for it.
#  - --refresh-adapter upgrades the global `claude-agent-acp` to @latest (network,
#    changes your global npm). Optional — only affects labels, not whether the id
#    works.
set -euo pipefail

usage() {
    sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

model_id=""
refresh_adapter=0
for arg in "$@"; do
    case "$arg" in
        -h | --help) usage 0 ;;
        --refresh-adapter) refresh_adapter=1 ;;
        -*) echo "error: unknown flag '$arg'" >&2; usage 1 ;;
        *)
            if [ -n "$model_id" ]; then
                echo "error: only one model id may be given (got '$model_id' and '$arg')" >&2
                exit 1
            fi
            model_id="$arg"
            ;;
    esac
done

if [ -z "$model_id" ]; then
    echo "error: no model id given" >&2
    usage 1
fi

# Reject whitespace / comment markers that the loader would ignore or split.
case "$model_id" in
    *[[:space:]]* | \#*)
        echo "error: model id must be a single token with no whitespace and no leading '#': '$model_id'" >&2
        exit 1
        ;;
esac

config_file="${YALDA_CLAUDE_MODELS:-$HOME/.config/yalda/claude-models.conf}"
mkdir -p "$(dirname "$config_file")"
[ -f "$config_file" ] || : >"$config_file"

# Already advertised? (present in the config file — the compiled defaults are
# added by Yalda regardless, so a default id doesn't need a line here.)
if grep -qxF "$model_id" "$config_file"; then
    echo "already in $config_file: $model_id"
else
    printf '%s\n' "$model_id" >>"$config_file"
    echo "added to $config_file: $model_id"
fi

echo
echo "Per-install extra Claude models ($config_file):"
grep -vE '^\s*(#|$)' "$config_file" | sed 's/^/  - /' || echo "  (none)"

# Report whether the installed adapter's SDK recognizes the id (informational).
adapter_bin="$(command -v claude-agent-acp 2>/dev/null || true)"
if [ -n "$adapter_bin" ]; then
    adapter_root="$(dirname "$(dirname "$(readlink -f "$adapter_bin")")")"
    if [ "$refresh_adapter" -eq 1 ]; then
        echo
        echo "==> refreshing claude-agent-acp to @latest"
        npm i -g @agentclientprotocol/claude-agent-acp@latest
    fi
    if grep -rqF "$model_id" "$adapter_root" 2>/dev/null; then
        echo
        echo "adapter recognizes '$model_id' — it will get a polished label."
    else
        echo
        echo "note: the installed claude-agent-acp does not yet list '$model_id';"
        echo "      it still works (passed to the API verbatim) but shows the raw id."
        echo "      Run with --refresh-adapter, or: npm i -g @agentclientprotocol/claude-agent-acp@latest"
    fi
fi

echo
echo "Done. Open a NEW Claude agent session to see '$model_id' in the model picker"
echo "(space → M, or the 'model ▾' badge). No rebuild or restart needed."
