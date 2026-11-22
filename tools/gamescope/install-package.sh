#!/usr/bin/env bash
# Install the host-runnable DeckShell package into ~/.local/share/deckshell/{bin,lib,session}.
# Run in the build container (or host).  Does NOT touch Steam, systemd or the session.
#   install-package.sh --deck USER@HOST PC_BINARY  (PC-built delivery + host checks)
#   install-package.sh [BINARY]  (on Deck; explicit Deck-built BINARY remains supported)
# Refuses a binary that needs GLIBC > 2.34 (it would crash on the older SteamOS partition).  GLIBC_MAX=2.43 overrides.
# Also puts STABLE copies of the install/rollback scripts + docs in ~/.local/share/deckshell/session/ (they do not depend
# on the source tree, which lives in a temporary session folder) and a "deckshell-rollback" one-liner.
set -eu
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$SELF/../.." && pwd)"
if [ "${1:-}" = --deck ]; then
    REMOTE="${2:?usage: --deck USER@HOST PC_BINARY}"
    PC_BIN="${3:?usage: --deck USER@HOST PC_BINARY}"
    case "$REMOTE" in -*|*[[:space:]]*) echo 'invalid SSH destination' >&2; exit 2;; esac
    [ -x "$PC_BIN" ] || { echo "no PC binary: $PC_BIN" >&2; exit 1; }
    ssh "$REMOTE" 'mkdir -p ~/.cache/deckshell/pc-package/source'
    scp -q -- "$PC_BIN" "$REMOTE:.cache/deckshell/pc-package/deckshell.new"
    tar -C "$ROOT" -cf - session tools/gamescope docs/ROLLBACK.md |
        ssh "$REMOTE" 'tar -xf - -C ~/.cache/deckshell/pc-package/source'
    ssh "$REMOTE" 'set -e; chmod 0755 ~/.cache/deckshell/pc-package/deckshell.new; bash ~/.cache/deckshell/pc-package/source/tools/gamescope/check-glibc.sh ~/.cache/deckshell/pc-package/deckshell.new 2.34; mv ~/.cache/deckshell/pc-package/deckshell.new ~/.cache/deckshell/pc-package/deckshell; bash ~/.cache/deckshell/pc-package/source/tools/gamescope/install-package.sh'
    exit
fi
BIN="${1:-$HOME/.cache/deckshell/pc-package/deckshell}"
D="${XDG_DATA_HOME:-$HOME/.local/share}/deckshell"
[ -x "$BIN" ] || { echo "no binary $BIN (run tools/gamescope/build.sh)" >&2; exit 1; }
"$SELF/check-glibc.sh" "$BIN" "${GLIBC_MAX:-2.34}"
mkdir -p "$D/bin" "$D/lib" "$D/session"
install -m 0755 "$BIN" "$D/bin/deckshell.new" && mv -f "$D/bin/deckshell.new" "$D/bin/deckshell"
install -m 0755 "$ROOT/session/deckshell-run" "$D/bin/deckshell-run"
# all runtime libs (asound, udev, fontconfig, freetype...) exist on the SteamOS host: nothing bundled.
# Any .so dropped into $D/lib is picked up by deckshell-run via LD_LIBRARY_PATH.
for f in deckshell-session.sh session-log.py power-recover.sh held-button.py install.sh uninstall.sh install-root.sh uninstall-root.sh rollback.sh dropin.conf.in; do
    install -m 0755 "$ROOT/session/$f" "$D/session/$f"
done
# The restart wrapper agents use on the Deck (pre-checks, one explicit switch, post-checks): docs/deck-session-restart-2026-11-15.md
install -m 0755 "$ROOT/tools/gamescope/deck-session-restart.sh" "$D/session/deck-session-restart.sh"
for f in gsctl.sh stage1.py; do install -m 0755 "$SELF/$f" "$D/session/$f"; done
install -m 0644 "$ROOT/docs/ROLLBACK.md" "$D/session/ROLLBACK.md"
mkdir -p ~/.local/share/applications
cat >~/.local/share/applications/deckshell.desktop <<EOT
[Desktop Entry]
Type=Application
Name=DeckShell
Comment=DeckShell (native Steam UI replica)
Exec=$D/bin/deckshell-run
Icon=steam
Terminal=false
Categories=Game;
EOT
echo "installed: $D/bin/{deckshell,deckshell-run}  $D/session/*  (+ .desktop Exec -> host launcher)"
