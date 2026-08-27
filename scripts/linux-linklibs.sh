#!/usr/bin/env bash
#
# Linux no-root link-library shim for yalda-gpui.
#
# GPUI links the system X11/xkb C libraries (-lxcb, -lxkbcommon,
# -lxkbcommon-x11). The *runtime* .so files ship in the base X11 packages, but
# the unversioned link-time symlinks that `ld` needs (libxcb.so -> libxcb.so.1,
# etc.) come only from the distro's -dev packages:
#
#   sudo apt install libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
#
# If you have root, run that and you're done — you do NOT need this script.
#
# On a box where you can't install -dev packages, this script recreates just
# those symlinks in a user-owned directory and prints the cargo search path to
# add. The repo's ~/.cargo/config.toml (or RUSTFLAGS) points `ld` there. The
# symlinks target the real /usr/lib .so files, so nothing is duplicated.
#
#   ./scripts/linux-linklibs.sh            # create symlinks, print the -L path
#   LINKDIR=/some/dir ./scripts/linux-linklibs.sh
#
set -euo pipefail

LINKDIR="${LINKDIR:-$HOME/.local/yalda-linklibs}"
LIBDIR="/usr/lib/x86_64-linux-gnu"

mkdir -p "$LINKDIR"

made=0
for stem in libxcb libxkbcommon libxkbcommon-x11; do
  # Already have a proper unversioned .so somewhere on the default path? skip.
  if [[ -e "$LIBDIR/$stem.so" ]]; then
    continue
  fi
  # Find the highest versioned runtime .so to point at.
  target="$(ls -1 "$LIBDIR/$stem.so."* 2>/dev/null | sort -V | tail -1 || true)"
  if [[ -z "$target" ]]; then
    echo "warning: $stem runtime library not found under $LIBDIR" >&2
    echo "         install it with your package manager (e.g. libxcb1, libxkbcommon0)" >&2
    continue
  fi
  ln -sf "$target" "$LINKDIR/$stem.so"
  echo "linked $LINKDIR/$stem.so -> $target"
  made=$((made + 1))
done

if [[ "$made" -eq 0 ]]; then
  echo "nothing to do — link-time symlinks already resolvable."
else
  echo
  echo "Add this to the linker search path (already wired in ~/.cargo/config.toml on this box):"
  echo "  export RUSTFLAGS=\"-L $LINKDIR\""
fi
