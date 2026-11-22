#!/usr/bin/env bash
# Called by the Desktop launcher; REPO selects the worktree to build and install.
set -eu
DECK="${DECK:-deck@192.168.1.50}"
REPO="${REPO:-$HOME/Projects/deckshell/repo}"
if [ "$#" -gt 1 ] || { [ "$#" -eq 1 ] && [ "$1" != --gaming ]; }; then
    echo 'usage: desktop-build-install.sh [--gaming]' >&2; exit 2
fi
cd "$REPO"
echo "== branch: $(git branch --show-current)  commit: $(git rev-parse --short HEAD)"
git status --short | head -5 | sed 's/^/   uncommitted: /' || true
echo '== ping Deck'; ssh -o ConnectTimeout=8 "$DECK" 'echo ok: $(hostname)'
echo '== build (glibc 2.34 compatible)'
tools/gamescope/build.sh
BIN="$(tools/gamescope/build.sh --bin-path)"
[ -x "$BIN" ] || { echo "no binary $BIN" >&2; exit 1; }
echo '== install on Deck'
tools/gamescope/install-package.sh --deck "$DECK" "$BIN"
echo "== PC binary sha256: $(sha256sum "$BIN" | cut -c1-16)…"
echo "== Deck: $(ssh "$DECK" 'sha256sum ~/.local/share/deckshell/bin/deckshell | cut -c1-16')…"
if [ "${1:-}" = --gaming ]; then
    # steamos-session-select restarts the whole session: gamescope loses its seat and DRM master,
    # then atomic commits fail with "Permission denied" and the picture freezes (task 338).
    # Refuse unless this boot is clean; a Deck with such errors needs a reboot, not a session switch.
    denied=$(ssh "$DECK" 'journalctl --user -b --no-pager | grep -c "Permission denied"; true')
    if [ "$denied" != 0 ]; then
        echo "refused: Deck has '${denied:-unknown}' 'Permission denied' lines this boot; reboot the Deck first" >&2
        exit 1
    fi
    # One explicit switch through the wrapper: pre-checks (home screen, no game, owner idle, no second ssh,
    # no Steam helper running), then the switch, then 0 'Permission denied' in a 30 s window (docs/deck-session-restart-2026-11-15.md).
    ssh "$DECK" 'bash ~/.local/share/deckshell/session/deck-session-restart.sh restart gamescope'
else
    echo 'Done. New build takes effect after the next Gaming Mode restart.'
fi
