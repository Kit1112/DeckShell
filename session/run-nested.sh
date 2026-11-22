#!/usr/bin/env bash
# Desktop-mode test launcher: DeckShell inside a NESTED gamescope.  Does not touch Steam or the session.
#   run-nested.sh            plain gamescope (no Steam mode): window is shown directly
#   run-nested.sh -e         Steam mode like the real session: DeckShell claims the base layer (DECKSHELL_GS_BASE=1)
#   run-nested.sh -e --headless   no window on the desktop; take pixels with: gsctl.sh shot OUT.png (DISPLAY of the nested X)
# Binary: $DECKSHELL_BIN, else ~/.local/share/deckshell/bin/deckshell-run.
set -u
EMODE=0; HEADLESS=0
for a in "$@"; do case "$a" in -e) EMODE=1 ;; --headless) HEADLESS=1 ;; esac; done
BIN="${DECKSHELL_BIN:-$HOME/.local/share/deckshell/bin/deckshell-run}"
[ -x "$BIN" ] || { echo "no $BIN; run tools/gamescope/install-package.sh" >&2; exit 1; }
command -v gamescope >/dev/null || { echo "gamescope missing (run on the SteamOS host: distrobox-host-exec $0 ...)" >&2; exit 1; }
FLAGS=(-W 1280 -H 800 -w 1280 -h 800)
[ "$EMODE" = 1 ] && FLAGS+=(-e)
[ "$HEADLESS" = 1 ] && FLAGS+=(--backend headless)
export DECKSHELL_FULLSCREEN=1 DECKSHELL_GAMESCOPE=1 DECKSHELL_READY_FILE="${DECKSHELL_READY_FILE:-/tmp/deckshell-nested-ready}"
[ "$EMODE" = 1 ] && export DECKSHELL_GS_BASE=1
echo "gamescope ${FLAGS[*]} -- $BIN"
exec gamescope "${FLAGS[@]}" -- env -u WAYLAND_DISPLAY "$BIN"
