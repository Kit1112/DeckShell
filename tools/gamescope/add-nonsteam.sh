#!/usr/bin/env bash
# Step-1 helper: add DeckShell to Steam as a Non-Steam game WITHOUT touching shortcuts.vdf.
# Run on the HOST in desktop mode with Steam running (user runs it; it is not run automatically).
#   add-nonsteam.sh            use steam://addnonsteamgame (Steam adds it itself)
#   add-nonsteam.sh --manual   just print the click path (KDE "Add to Steam" / Steam UI)
set -eu
RUN="${XDG_DATA_HOME:-$HOME/.local/share}/deckshell/bin/deckshell-run"
[ -x "$RUN" ] || { echo "missing $RUN (tools/gamescope/install-package.sh)"; exit 1; }
if [ "${1:-}" = "--manual" ]; then
cat <<EOT
Manual (always works), desktop mode:
  Steam -> bottom-left "Add a Game" -> "Add a Non-Steam Game..." -> tick "DeckShell" -> Add Selected Programs
  (or right-click ~/.local/share/applications/deckshell.desktop in Dolphin/menu -> "Add to Steam")
Then in the Steam library: DeckShell -> Properties -> Launch Options:  (leave empty)
EOT
exit 0; fi
pgrep -x steam >/dev/null || { echo "Steam is not running; start it first" >&2; exit 1; }
enc="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$RUN")"
echo "asking Steam to add: $RUN"
steam "steam://addnonsteamgame/$enc" >/dev/null 2>&1 &
echo "If Steam shows a confirmation, accept it. Verify: library -> Non-Steam -> DeckShell."
