#!/usr/bin/env bash
#
# Dev iteration loop for yalda — rebuild + restart JUST the gui.
#
# Builds ONLY yalda-gpui and restarts it, leaving the running session server
# (and every live agent session it holds) UNTOUCHED. We deliberately do NOT
# kill the server and do NOT clear the socket: the fresh GUI reconnects to the
# existing server and re-attaches its sessions via the proven GUI-restart /
# owner-reclaim path, so your agents — mid-turn context, transcripts, the lot —
# survive the bounce. This is the fast loop for GUI-only changes.
#
# If you changed the SERVER itself, use ./dev-server.sh instead (it rebuilds and
# bounces the daemon under this live GUI; run both to refresh everything).
#
# PROFILE: RELEASE by default — a debug GPUI build stutters on fast text input.
# Use `DEBUG=1 ./dev-gui.sh` for the fast-compile loop (expect input lag).
#
#   GUI logs -> this terminal
#
# Quit the GUI and re-run this script to iterate. Pass a file to open it:
#   ./dev-gui.sh            # release, opens the browser
#   ./dev-gui.sh notes.md   # release, opens a file
#   DEBUG=1 ./dev-gui.sh    # fast-compile debug loop (expect input lag)
#
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${DEBUG:-0}" == "1" ]]; then
  PROFILE="debug"; CARGO_PROFILE_FLAG=()
else
  PROFILE="release"; CARGO_PROFILE_FLAG=(--release)
fi

echo "▸ building yalda-gpui  (${PROFILE})…"
cargo build "${CARGO_PROFILE_FLAG[@]}" --bin yalda-gpui

echo "▸ stopping any running yalda gui (server left alone — agents survive)…"
pkill -f 'target/debug/yalda-gpui'   2>/dev/null || true
pkill -f 'target/release/yalda-gpui' 2>/dev/null || true
sleep 0.3   # let the old window's teardown settle before the new one attaches

echo "▸ launching fresh gui (${PROFILE}; reconnecting to the existing server)…"
exec "./target/${PROFILE}/yalda-gpui" "$@"
