#!/usr/bin/env bash
#
# Ship a new session-server build to the managed systemd USER service.
#
# The fast dev loop for the persistent daemon: rebuild yalda-session-server,
# publish it to the STABLE installed path (~/.local/bin), and restart the
# service. Restart is LOSSLESS — agent history is durable in ~/.yalda/wal
# (ADR-0009/-0018) and the fresh server replays it on startup; the live GUI
# reconnects and your agents (mid-turn context, transcripts, the lot) survive.
# The ONE exception is a rebuild that bumps the on-disk WAL format version
# (WAL_VERSION), which discards older WALs on read.
#
# Prereq: run ./install-service.sh once first (sets up the unit + linger).
# If you have NOT installed the managed service, use ./dev-server.sh instead
# (that path relies on the GUI to respawn a detached, non-persistent server).
#
# PROFILE: RELEASE by default (a debug server is fine, but release matches the
# running GUI). Use `DEBUG=1 ./deploy-server.sh` for the fast-compile loop.
#
#   ./deploy-server.sh          # release: build, install, restart
#   DEBUG=1 ./deploy-server.sh  # fast-compile debug loop
#
set -euo pipefail
cd "$(dirname "$0")"

UNIT="yalda-session-server"
BIN_DIR="${HOME}/.local/bin"
BIN_DEST="${BIN_DIR}/yalda-session-server"
LOG="${HOME}/.yalda/session-server.log"

if [[ "${DEBUG:-0}" == "1" ]]; then
  PROFILE="debug"; CARGO_PROFILE_FLAG=()
else
  PROFILE="release"; CARGO_PROFILE_FLAG=(--release)
fi

# Fail early with a clear message if the managed service was never installed —
# otherwise `systemctl restart` below would error cryptically.
if ! systemctl --user list-unit-files "${UNIT}.service" >/dev/null 2>&1 \
   || ! systemctl --user cat "${UNIT}.service" >/dev/null 2>&1; then
  echo "error: ${UNIT}.service is not installed for this user." >&2
  echo "       run ./install-service.sh once first (or use ./dev-server.sh for the" >&2
  echo "       non-persistent GUI-respawned server)." >&2
  exit 1
fi

echo "▸ building yalda-session-server  (${PROFILE})…"
cargo build "${CARGO_PROFILE_FLAG[@]}" --bin yalda-session-server

echo "▸ publishing binary → ${BIN_DEST}…"
mkdir -p "${BIN_DIR}"
# Copy to a temp name then rename: an atomic replace, so the running service's
# mapped inode is never truncated mid-copy (it keeps running the old inode until
# the restart below).
install -m 0755 "target/${PROFILE}/yalda-session-server" "${BIN_DEST}.new"
mv -f "${BIN_DEST}.new" "${BIN_DEST}"

echo "▸ restarting ${UNIT} (lossless — WAL replays on startup)…"
systemctl --user restart "${UNIT}"

# Give it a moment, then report status so a failed start is obvious.
sleep 0.5
if systemctl --user is-active --quiet "${UNIT}"; then
  echo "▸ done — ${UNIT} is active."
else
  echo "warning: ${UNIT} is not active after restart. Recent log:" >&2
  systemctl --user --no-pager status "${UNIT}" 2>&1 | tail -15 >&2 || true
  exit 1
fi
echo "  tail the server log with:  tail -f \"${LOG}\""
