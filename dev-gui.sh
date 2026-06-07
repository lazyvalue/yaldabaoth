#!/usr/bin/env bash
#
# Dev iteration loop for sketch — rebuild + restart JUST the gui.
#
# Builds ONLY sketch-gpui and restarts it, leaving the running session server
# (and every live agent session it holds) UNTOUCHED. We deliberately do NOT
# kill the server and do NOT clear the socket: the fresh GUI reconnects to the
# existing server and re-attaches its sessions via the proven GUI-restart /
# owner-reclaim path, so your agents — mid-turn context, transcripts, the lot —
# survive the bounce. This is the fast loop for GUI-only changes.
#
# If you changed the SERVER itself, use ./dev-all.sh instead (it rebuilds and
# restarts both, dropping live sessions).
#
#   GUI logs -> this terminal
#
# Quit the GUI and re-run this script to iterate. Pass a file to open it:
#   ./dev-gui.sh            # opens the browser
#   ./dev-gui.sh notes.md   # opens a file
#
set -euo pipefail
cd "$(dirname "$0")"

echo "▸ building sketch-gpui…"
cargo build --bin sketch-gpui

echo "▸ stopping any running sketch gui (server left alone — agents survive)…"
pkill -f 'target/debug/sketch-gpui' 2>/dev/null || true
sleep 0.3   # let the old window's teardown settle before the new one attaches

echo "▸ launching fresh gui (reconnecting to the existing server)…"
exec ./target/debug/sketch-gpui "$@"
