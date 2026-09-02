#!/usr/bin/env bash
#
# Dev iteration loop for yalda — rebuild + restart the session server.
#
# ADR-0037 (bug-0064 split-brain recurrence): the systemd USER service is the
# ONE owner of the server's lifecycle. The GUI never launches a server, and no
# dev flow may pkill + rely on a GUI respawn (that path shipped a stale-binary
# split-brain: two servers, twin agents per session, shared WAL appends). This
# script therefore just delegates to ./deploy-server.sh — build, publish to
# ~/.local/bin (the stable path the unit executes), systemctl --user restart.
#
# SESSIONS SURVIVE this: agent history is durable in ~/.yalda/wal and the fresh
# server replays it on startup (lossless since bug-0064's fix — which only
# protects you if the deployed binary is CURRENT; that is exactly what this
# script guarantees).
#
# Prereq (once): ./install-service.sh  — installs the unit + enables linger.
#
#   ./dev-server.sh            # release: build, install, restart
#   DEBUG=1 ./dev-server.sh    # fast-compile debug loop
#
set -euo pipefail
cd "$(dirname "$0")"
exec ./deploy-server.sh "$@"
