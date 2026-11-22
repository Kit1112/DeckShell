#!/usr/bin/env bash
# Runs check-notification-overlay.sh against fake xwininfo/xprop outputs: good, unmarked (the task 526 bug), focus-stealing.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd -P)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin"
cat > "$T/bin/xwininfo" <<'S'
#!/bin/sh
case "$*" in
  *-root*) echo '     0x800003 "DeckShell REC": ("deckshell" "deckshell")  1280x800+0+0  +0+0';;
  *) echo "  Map State: ${MAPSTATE:-IsUnMapped}"; echo "  Override Redirect State: ${OR:-yes}";;
esac
S
cat > "$T/bin/xprop" <<'S'
#!/bin/sh
echo 'WM_CLASS(STRING) = "deckshell", "deckshell"'
[ -n "${EXT:-}" ] && echo "GAMESCOPE_EXTERNAL_OVERLAY(CARDINAL) = $EXT"
[ -n "${NOINPUT:-}" ] && printf 'WM_HINTS(WM_HINTS):\n\t\tClient accepts input or input focus: False\n'
[ -n "${EXTRA:-}" ] && echo "$EXTRA(CARDINAL) = 1"
exit 0
S
chmod +x "$T/bin/"*
run() { PATH="$T/bin:$PATH" bash "$ROOT/tools/gamescope/check-notification-overlay.sh" "$@"; }
EXT=1 NOINPUT=1 MAPSTATE=IsViewable run --expect-mapped | grep -q '^OK'
EXT=1 NOINPUT=1 run --expect-unmapped | grep -q '^OK'
# the state found on the Deck on 2026-11-20: window mapped, role never set
! EXT= MAPSTATE=IsViewable run --expect-mapped >/dev/null
! EXT=1 NOINPUT=1 MAPSTATE=IsViewable run --expect-unmapped >/dev/null
! EXT=1 NOINPUT=1 EXTRA=STEAM_INPUT_FOCUS run >/dev/null
! EXT=1 NOINPUT=1 OR=no run >/dev/null
echo "notification overlay check: ok"
