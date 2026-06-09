#!/usr/bin/env bash
# rebuild-server.sh — rebuild sketch-session-server and bring the fresh one up.
#
# The server a GUI talks to is the `sketch-session-server` binary sitting NEXT TO
# the GUI's own executable (the GUI respawns that sibling on disconnect). So to
# update the server your RUNNING GUI uses, this script builds the binary in the
# GUI's checkout, then bounces the daemon and lets the GUI respawn the fresh one.
#
# Target selection (first match wins):
#   1. $SKETCH_REPO set            -> build that checkout (standalone launch)
#   2. a sketch-gpui is running    -> build ITS checkout, let the GUI respawn
#   3. neither                     -> build this checkout, launch standalone
#
# Usage:
#   scripts/rebuild-server.sh            # auto-detect (debug)
#   scripts/rebuild-server.sh release    # force release profile (standalone modes)
#   SKETCH_REPO=/path scripts/rebuild-server.sh
#
# Sessions PERSIST across the bounce (per-session WAL replay).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOCK="${SKETCH_SESSION_SOCKET:-/tmp/sketch-session-$USER.sock}"
LOG="${SKETCH_SERVER_LOG:-$HOME/Library/Caches/sketch/session-server.log}"
DAEMON_PAT='sketch-session-server$'
GUI_PAT='sketch-gpui$'

exe_path() { ps -o command= -p "$1" 2>/dev/null | awk '{print $1}'; }

# --- pick the target binary + how it gets relaunched -----------------------
GUI_RESPAWNS=0
if [ -n "${SKETCH_REPO:-}" ]; then
  CHECKOUT="$SKETCH_REPO"; PROFILE="${1:-debug}"
  echo "==> target: \$SKETCH_REPO = $CHECKOUT ($PROFILE), standalone launch"
elif gui_pid="$(pgrep -f "$GUI_PAT" | head -1)" && [ -n "$gui_pid" ]; then
  gui_exe="$(exe_path "$gui_pid")"
  CHECKOUT="${gui_exe%/target/*}"
  case "$gui_exe" in */release/*) PROFILE=release ;; *) PROFILE=debug ;; esac
  GUI_RESPAWNS=1
  echo "==> target: running GUI's checkout = $CHECKOUT ($PROFILE)"
  echo "    (GUI pid $gui_pid will respawn the fresh sibling on bounce)"
else
  CHECKOUT="$HERE"; PROFILE="${1:-debug}"
  echo "==> target: this checkout = $CHECKOUT ($PROFILE), standalone launch"
fi
BIN="$CHECKOUT/target/$PROFILE/sketch-session-server"

# --- build -----------------------------------------------------------------
echo "==> building sketch-session-server ($PROFILE) in $CHECKOUT"
flags=(--manifest-path "$CHECKOUT/Cargo.toml" --bin sketch-session-server)
[ "$PROFILE" = release ] && flags=(--release "${flags[@]}")
cargo build "${flags[@]}"
[ -x "$BIN" ] || { echo "!! build did not produce $BIN" >&2; exit 1; }

# --- bounce ----------------------------------------------------------------
echo "==> stopping running daemon(s)"
pkill -f "$DAEMON_PAT" 2>/dev/null || true
for _ in $(seq 1 60); do pgrep -f "$DAEMON_PAT" >/dev/null || break; sleep 0.1; done

mkdir -p "$(dirname "$LOG")"
if [ "$GUI_RESPAWNS" -eq 1 ]; then
  echo "==> waiting for the GUI to respawn the fresh daemon..."
  # The GUI reconnect path spawns its sibling (== $BIN). Don't launch our own.
else
  rm -f "$SOCK" 2>/dev/null || true
  echo "==> launching $BIN"
  nohup "$BIN" >>"$LOG" 2>&1 & disown || true
fi

# --- verify the listening daemon is the freshly-built one ------------------
for _ in $(seq 1 100); do          # up to ~10s (GUI reconnect backoff can be slow)
  pid="$(pgrep -f "$DAEMON_PAT" | head -1 || true)"
  if [ -n "$pid" ] && [ -S "$SOCK" ] && [ "$(exe_path "$pid")" = "$BIN" ]; then
    n=$(ls "$HOME/Library/Caches/sketch/wal"/*.log 2>/dev/null | wc -l | tr -d ' ')
    echo "==> up: pid $pid"
    echo "    $BIN listening on $SOCK; $n session WAL(s) replayed."
    exit 0
  fi
  sleep 0.1
done

echo "!! fresh daemon ($BIN) is not the one on $SOCK." >&2
echo "   listening: $(exe_path "$(pgrep -f "$DAEMON_PAT" | head -1)")" >&2
echo "   If a GUI keeps winning with a stale binary, quit it or set SKETCH_REPO." >&2
tail -6 "$LOG" >&2
exit 1
