#!/usr/bin/env bash
#
# Dev iteration loop for yalda — rebuild + restart BOTH gui and server.
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
# PROFILE: builds and runs RELEASE by default — a debug GPUI build re-runs
# full-window layout + paint unoptimized every frame and visibly STUTTERS on
# fast text input, so the app you actually use must be optimized. Use
# `DEBUG=1 ./dev-all.sh` only when you're iterating on code and want the fast
# compile loop (and can tolerate the input lag).
#
# SESSIONS SURVIVE this: agent history is durable in ~/.yalda/wal (ADR-0009,
# -0018), and the fresh server replays it on startup — this script clears only
# the socket/pid, never the WAL. The ONE exception is a rebuild that bumps the
# on-disk WAL format version (WAL_VERSION), which discards older WALs on read.
# (For a session-preserving server-only swap under a LIVE GUI, prefer
# scripts/rebuild-server.sh.)
#
#   Server logs -> ~/.yalda/session-server.log  (detached daemon)
#   GUI logs    -> this terminal
#
# Quit the GUI and re-run this script to iterate. Pass a file to open it:
#   ./dev-all.sh            # release, opens the browser
#   ./dev-all.sh notes.md   # release, opens a file
#   DEBUG=1 ./dev-all.sh    # fast-compile debug loop (expect input lag)
#
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${DEBUG:-0}" == "1" ]]; then
  PROFILE="debug"; CARGO_PROFILE_FLAG=()
else
  PROFILE="release"; CARGO_PROFILE_FLAG=(--release)
fi
BIN_DIR="target/${PROFILE}"

echo "▸ building yalda-gpui + yalda-session-server  (${PROFILE})…"
cargo build "${CARGO_PROFILE_FLAG[@]}" --bin yalda-gpui --bin yalda-session-server

echo "▸ stopping any running yalda server/gui (both profiles)…"
pkill -f 'target/debug/yalda-session-server'   2>/dev/null || true
pkill -f 'target/release/yalda-session-server' 2>/dev/null || true
pkill -f 'target/debug/yalda-gpui'             2>/dev/null || true
pkill -f 'target/release/yalda-gpui'           2>/dev/null || true
sleep 0.3   # let kill_on_drop + socket teardown settle

echo "▸ clearing stale socket/pid…"
rm -f "/tmp/yalda-session-${USER}.sock" "/tmp/yalda-session-${USER}.pid"

LOG="${HOME}/.yalda/session-server.log"
echo "▸ launching fresh server + gui   (${PROFILE}; server log: ${LOG})"
echo "  tail it in another terminal with:  tail -f \"${LOG}\""
exec "./${BIN_DIR}/yalda-gpui" "$@"
