#!/usr/bin/env bash
#
# Dev iteration loop for yalda — rebuild + restart JUST the session server.
#
# Builds ONLY yalda-session-server, kills the running daemon, and clears the
# stale socket/pid so the LIVE GUI respawns the freshly-built server sibling on
# its next reconnect. The GUI window itself is left untouched. This is the
# server-side counterpart to ./dev-gui.sh: change the SERVER (session-server,
# WAL, ACP channel, permission logic) and bounce it under a running GUI without
# restarting the GUI.
#
# SESSIONS SURVIVE this: agent history is durable in ~/.yalda/wal (ADR-0009,
# -0018), and the fresh server replays it on startup — this script clears only
# the socket/pid, never the WAL. The ONE exception is a rebuild that bumps the
# on-disk WAL format version (WAL_VERSION), which discards older WALs on read.
#
# If you only touched the GUI, use ./dev-gui.sh instead (it leaves the server
# alone). To bounce both, run ./dev-server.sh then ./dev-gui.sh.
#
# PROFILE: builds RELEASE by default to match the running GUI/server. Use
# `DEBUG=1 ./dev-server.sh` for the fast-compile loop.
#
#   Server logs -> ~/.yalda/session-server.log  (detached daemon, GUI-respawned)
#
#   ./dev-server.sh            # release
#   DEBUG=1 ./dev-server.sh    # fast-compile debug loop
#
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${DEBUG:-0}" == "1" ]]; then
  PROFILE="debug"; CARGO_PROFILE_FLAG=()
else
  PROFILE="release"; CARGO_PROFILE_FLAG=(--release)
fi

echo "▸ building yalda-session-server  (${PROFILE})…"
cargo build "${CARGO_PROFILE_FLAG[@]}" --bin yalda-session-server

echo "▸ stopping any running yalda server (gui left alone — it respawns the fresh one)…"
pkill -f 'target/debug/yalda-session-server'   2>/dev/null || true
pkill -f 'target/release/yalda-session-server' 2>/dev/null || true
sleep 0.3   # let kill_on_drop + socket teardown settle

echo "▸ clearing stale socket/pid (forces the live GUI to spawn the new server)…"
rm -f "/tmp/yalda-session-${USER}.sock" "/tmp/yalda-session-${USER}.pid"

LOG="${HOME}/.yalda/session-server.log"
echo "▸ done — the running GUI will respawn the fresh server on reconnect."
echo "  no GUI running? launch one with ./dev-gui.sh"
echo "  tail the server log with:  tail -f \"${LOG}\""
