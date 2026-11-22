#!/usr/bin/env bash
# USER-LEVEL install (no root).  Hooks DeckShell into gaming mode via a systemd *user* drop-in.
# NEVER run automatically; the live switch is the user's call.   Reversible: uninstall.sh
#
#   bash session/install.sh [--dry-run]
#
# Prereq: tools/gamescope/install-package.sh already put bin/deckshell + bin/deckshell-run in
#   ~/.local/share/deckshell/.  Files written:
#   ~/.local/share/deckshell/session/{deckshell-session.sh,gsctl.sh,held-button.py}
#   ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf   (the only hook)
#   ~/.config/deckshell/mode  (= stock: install changes NOTHING until you write "hybrid")
# The unit is NOT restarted; the hook takes effect at the next gaming-mode start.
set -eu
DRY=0
for a in "$@"; do case "$a" in --dry-run) DRY=1 ;; -h|--help) sed -n '2,12p' "$0"; exit 0 ;; *) echo "unknown option $a" >&2; exit 2 ;; esac; done
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$SELF/.." && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/deckshell"; CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
DEST="$DATA/session"; DROPDIR="$CONF/systemd/user/steam-launcher.service.d"; DROPIN="$DROPDIR/deckshell.conf"
MANIFEST="$DATA/install.manifest"
run() { if [ "$DRY" = 1 ]; then echo "[dry-run] $*"; else "$@"; fi; }

[ -f /usr/lib/systemd/user/steam-launcher.service ] || { echo "steam-launcher.service not found: not a SteamOS gaming-mode image" >&2; exit 1; }
[ -x "$DATA/bin/deckshell-run" ] && [ -x "$DATA/bin/deckshell" ] || { echo "run tools/gamescope/install-package.sh first" >&2; exit 1; }
[ -e /etc/systemd/user/steam-launcher.service.d/deckshell.conf ] && { echo "root install present; run uninstall-root.sh first" >&2; exit 1; }
[ -e "$MANIFEST" ] && echo "reinstalling over existing install"

run mkdir -p "$DEST" "$DROPDIR" "$CONF/deckshell"
[ "$SELF" = "$DEST" ] || run install -m 0755 "$SELF/deckshell-session.sh" "$DEST/deckshell-session.sh"
GS="$SELF/gsctl.sh"; [ -f "$GS" ] || GS="$ROOT/tools/gamescope/gsctl.sh"
[ "$GS" -ef "$DEST/gsctl.sh" ] || run install -m 0755 "$GS" "$DEST/gsctl.sh"
[ "$SELF" = "$DEST" ] || run install -m 0755 "$SELF/held-button.py" "$DEST/held-button.py"
# explicit opt-in: a leftover marker from rollback.sh / the watchdog would silently keep stock
[ -e "$CONF/deckshell/disable" ] && { run rm -f "$CONF/deckshell/disable"; echo "removed old $CONF/deckshell/disable"; }
[ -e "$CONF/deckshell/mode" ] || { [ "$DRY" = 1 ] || echo stock >"$CONF/deckshell/mode"; }

if [ "$DRY" = 1 ]; then echo "[dry-run] write $DROPIN"; exit 0; fi
sed -e "s|@RUN@|$DATA/bin/deckshell-run|" -e "s|@SESSION@|$DEST/deckshell-session.sh|" "$SELF/dropin.conf.in" >"$DROPIN"
{ echo "dropin=$DROPIN"; echo "dir=$DEST"; } >"$MANIFEST"
systemctl --user daemon-reload || true
systemd-analyze --user verify steam-launcher.service 2>&1 | grep -iv 'gamescope-environment' | head -n 5 || true
echo "installed. mode=$(cat "$CONF/deckshell/mode")"
echo "  enable DeckShell:   echo hybrid > $CONF/deckshell/mode   (then switch to gaming mode)"
echo "  kill switch:        touch $CONF/deckshell/disable   |  hold volume-down (or B) 2 s at boot  |  bash $SELF/uninstall.sh"
