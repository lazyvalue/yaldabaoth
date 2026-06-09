#!/usr/bin/env bash
#
# Dev iteration loop for sketch — rebuild + restart BOTH gui and server.
#
# Builds the latest binaries, stops any persistent session server (so the
# rebuild actually takes effect instead of the GUI reconnecting to the OLD
# running server), clears stale socket state, then launches a fresh server +
# GUI. The GUI auto-spawns a fresh DETACHED session server on launch; killing
# the old one here is what forces your new server binary to be used.
#
# Use this when you've changed the SERVER (session-server, WAL, ACP channel,
# permission logic) and need the new server binary live. If you only touched
# the GUI and want the gentlest loop, use ./dev-gui.sh instead.
#
# SESSIONS SURVIVE this: agent history is durable in ~/.sketch/wal (ADR-0009,
# -0018), and the fresh server replays it on startup — this script clears only
# the socket/pid, never the WAL. The ONE exception is a rebuild that bumps the
# on-disk WAL format version (WAL_VERSION), which discards older WALs on read.
# (For a session-preserving server-only swap under a LIVE GUI, prefer
# scripts/rebuild-server.sh.)
#
#   Server logs -> ~/.sketch/session-server.log  (detached daemon)
#   GUI logs    -> this terminal
#
# Quit the GUI and re-run this script to iterate. Pass a file to open it:
#   ./dev-all.sh            # opens the browser
#   ./dev-all.sh notes.md   # opens a file
#
set -euo pipefail
cd "$(dirname "$0")"

echo "▸ building sketch-gpui + sketch-session-server…"
cargo build --bin sketch-gpui --bin sketch-session-server

echo "▸ stopping any running sketch server/gui…"
pkill -f 'target/debug/sketch-session-server' 2>/dev/null || true
pkill -f 'target/debug/sketch-gpui'           2>/dev/null || true
sleep 0.3   # let kill_on_drop + socket teardown settle

echo "▸ clearing stale socket/pid…"
rm -f "/tmp/sketch-session-${USER}.sock" "/tmp/sketch-session-${USER}.pid"

LOG="${HOME}/.sketch/session-server.log"
echo "▸ launching fresh server + gui   (server log: ${LOG})"
echo "  tail it in another terminal with:  tail -f \"${LOG}\""
exec ./target/debug/sketch-gpui "$@"
