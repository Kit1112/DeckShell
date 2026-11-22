#!/usr/bin/env bash
# Reverse install.sh (user-level).  Safe from desktop mode, SSH or a TTY.   uninstall.sh [--purge]
# Minimal manual equivalent:  rm ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf
#                             systemctl --user daemon-reload
set -u
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/deckshell"; CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
DROPIN="$CONF/systemd/user/steam-launcher.service.d/deckshell.conf"
PURGE=0; [ "${1:-}" = "--purge" ] && PURGE=1
if [ -e "$DROPIN" ]; then
    if grep -q deckshell-managed "$DROPIN"; then rm -f "$DROPIN" && echo "removed $DROPIN"; else echo "skip $DROPIN (not ours)"; fi
fi
rmdir "$CONF/systemd/user/steam-launcher.service.d" 2>/dev/null || true
# NOTE: $DATA/session (stable copies of the rollback scripts) is deliberately KEPT; only the hook goes.
rm -f "$DATA/install.manifest"
systemctl --user daemon-reload 2>/dev/null || true
if [ "$PURGE" = 1 ]; then
    rm -rf "$CONF/deckshell" "$HOME/.local/state/deckshell" "$HOME/.local/share/applications/deckshell.desktop"
    rm -rf "$DATA/bin/deckshell" "$DATA/bin/deckshell-run" "$DATA/lib"   # keeps bin/ffmpeg + session/ (rollback copies)
    echo "purged config, state, .desktop, deckshell binaries (kept $DATA/session and $DATA/bin/ffmpeg; rm -rf $DATA to drop them too)"
fi
echo "done. Stock Steam UI returns at next gaming-mode start."
