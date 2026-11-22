#!/usr/bin/env bash
# Read-only check of the DeckShell notification overlay window on a running gamescope Xwayland (task 526).
# Usage: DISPLAY=:0 check-notification-overlay.sh [--expect-mapped|--expect-unmapped]
# Reads only (xwininfo/xprop). Exit 0: ok, 1: contract broken, 2: window missing (it exists from startup).
set -u
TITLE="DeckShell REC"
expect="${1:-}"
id=$(xwininfo -root -children 2>/dev/null | grep -F "\"$TITLE\"" | grep -oE '0x[0-9a-f]+' | head -1)
if [ -z "$id" ]; then echo "FAIL: window \"$TITLE\" not found"; exit 2; fi
props=$(xprop -id "$id" 2>/dev/null)
info=$(xwininfo -id "$id" 2>/dev/null)
bad=0
fail() { echo "FAIL: $*"; bad=1; }
mapped=no; echo "$info" | grep -q "Map State: IsViewable" && mapped=yes
echo "window $id mapped=$mapped"
# The layer: a gamescope external overlay is drawn above the game.
echo "$props" | grep -qE '^GAMESCOPE_EXTERNAL_OVERLAY\(CARDINAL\) = 1$' || fail "GAMESCOPE_EXTERNAL_OVERLAY is not 1 (window would sit under the game)"
# No input/focus: must never carry focus or Steam overlay roles.
echo "$props" | grep -qE '^WM_HINTS' && ! echo "$props" | grep -A4 '^WM_HINTS' | grep -qE 'Client accepts input or input focus: False' && fail "WM_HINTS input is not False"
for atom in STEAM_OVERLAY STEAM_INPUT_FOCUS STEAM_GAME STEAM_BIGPICTURE; do
    echo "$props" | grep -q "^$atom" && fail "$atom is set: the window would take focus/layer from the game"
done
echo "$info" | grep -q "Override Redirect State: yes" || fail "window is not override-redirect (a WM request could focus it)"
case "$expect" in
    --expect-mapped)   [ "$mapped" = yes ] || fail "expected the overlay mapped (a toast is visible)" ;;
    --expect-unmapped) [ "$mapped" = no ]  || fail "expected the overlay unmapped while idle" ;;
esac
[ "$bad" = 0 ] && echo "OK" || exit 1
