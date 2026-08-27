#!/usr/bin/env bash
#
# One-time setup for the persistent session server on Linux (systemd USER
# service). After this, the daemon survives logout and starts at boot; ship new
# builds with ./deploy-server.sh.
#
# What it does:
#   1. Enables LINGER for your user (loginctl) so the user service manager runs
#      even when you are logged out, and the service starts at boot.
#   2. Stops any stray, unmanaged server (a GUI-auto-launched detached daemon)
#      and clears its socket, so the managed instance can claim the socket. This
#      is lossless — the WAL survives (ADR-0009/-0018).
#   3. Installs the unit into ~/.config/systemd/user/, enables it, and does the
#      first build+publish+start via ./deploy-server.sh.
#
# Re-runnable: safe to run again to refresh the unit file.
#
#   ./install-service.sh          # release build for the first start
#   DEBUG=1 ./install-service.sh  # debug build for the first start
#
set -euo pipefail
cd "$(dirname "$0")"

UNIT="yalda-session-server"
UNIT_SRC="dist/systemd/${UNIT}.service"
UNIT_DIR="${HOME}/.config/systemd/user"
SOCK="/tmp/yalda-session-${USER}.sock"
PID="/tmp/yalda-session-${USER}.pid"

if ! command -v systemctl >/dev/null 2>&1; then
  echo "error: systemctl not found — this installer targets systemd (Linux)." >&2
  echo "       on macOS use the built-in LaunchAgent: yalda-session-server install." >&2
  exit 1
fi
if [[ ! -f "${UNIT_SRC}" ]]; then
  echo "error: ${UNIT_SRC} not found (run from the repo root)." >&2
  exit 1
fi

echo "▸ enabling linger for ${USER} (service runs when logged out + at boot)…"
# May prompt for authentication via polkit; harmless if already enabled.
loginctl enable-linger "${USER}" || {
  echo "warning: could not enable linger; the service will still run while you" >&2
  echo "         are logged in, but not across logout/boot until linger is on." >&2
}

echo "▸ stopping any stray (GUI-respawned) server + clearing socket…"
# Only targets the unmanaged detached daemon; the managed unit, if already
# running, is handled by the restart in deploy-server.sh. WAL is untouched.
pkill -TERM -f 'target/debug/yalda-session-server'   2>/dev/null || true
pkill -TERM -f 'target/release/yalda-session-server' 2>/dev/null || true
pkill -TERM -f "${HOME}/.local/bin/yalda-session-server" 2>/dev/null || true
sleep 0.3
rm -f "${SOCK}" "${PID}"

echo "▸ installing unit → ${UNIT_DIR}/${UNIT}.service…"
mkdir -p "${UNIT_DIR}"
install -m 0644 "${UNIT_SRC}" "${UNIT_DIR}/${UNIT}.service"

echo "▸ reloading + enabling ${UNIT}…"
systemctl --user daemon-reload
systemctl --user enable "${UNIT}"

echo "▸ first deploy (build + publish + start)…"
# deploy-server.sh restarts the (now enabled) unit, which starts it fresh.
DEBUG="${DEBUG:-0}" ./deploy-server.sh

echo
echo "▸ installed. The session server now starts at boot and survives logout."
echo "  status:  systemctl --user status ${UNIT}"
echo "  logs:    journalctl --user -u ${UNIT} -f   (or tail ~/.yalda/session-server.log)"
echo "  deploy:  ./deploy-server.sh                 (build + restart, lossless)"
echo "  remove:  systemctl --user disable --now ${UNIT} && rm ${UNIT_DIR}/${UNIT}.service"
