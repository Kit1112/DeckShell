#!/usr/bin/env bash
# ONE-LINE ROLLBACK of everything DeckShell may have changed (Stage 2 hook, kill-switch marker, Stage 1 shortcut).
#   bash ~/.local/share/deckshell/session/rollback.sh          user-level, no root, safe to run any time (desktop mode / SSH / TTY)
#   sudo bash ~/.local/share/deckshell/session/rollback.sh     also removes the system-level (/etc) hook if present
# Idempotent.  Never touches /usr, gamescope, Steam or the SteamOS image.
set -u
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
U="${SUDO_USER:-$(id -un)}"; UHOME="$(getent passwd "$U" | cut -d: -f6)"; UHOME="${UHOME:-$HOME}"
[ "$(id -u)" != 0 ] && UHOME="$HOME"   # non-root: act on the invoking HOME (also lets tests/sandboxes redirect it)
CONF="$UHOME/.config/deckshell"
echo "[rollback] marker: $CONF/disable"
mkdir -p "$CONF" && : >"$CONF/disable"; [ "$(id -u)" = 0 ] && chown -R "$U": "$CONF" 2>/dev/null
echo stock >"$CONF/mode"
DROP="$UHOME/.config/systemd/user/steam-launcher.service.d/deckshell.conf"
if [ -e "$DROP" ]; then rm -f "$DROP" && echo "[rollback] removed $DROP"; fi
if [ "$(id -u)" = 0 ]; then
    if [ -x "$SELF/uninstall-root.sh" ]; then bash "$SELF/uninstall-root.sh" --user "$U"; else rm -f /etc/systemd/user/steam-launcher.service.d/deckshell.conf; fi
    touch /etc/deckshell/disable 2>/dev/null
elif [ -e /etc/systemd/user/steam-launcher.service.d/deckshell.conf ]; then
    echo "[rollback] system-level hook still present: run   sudo bash $SELF/rollback.sh"
fi
XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u "$U")}" systemctl --user daemon-reload 2>/dev/null || true
python3 "$SELF/stage1.py" remove 2>/dev/null || true
echo "[rollback] done: next gaming-mode start = stock Steam UI.  Re-enable later: rm $CONF/disable"
