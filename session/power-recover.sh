#!/usr/bin/env bash
# ExecStopPost of the supervised UI: runs independently after crash/SIGKILL.
#
# It restores the stock Power daemon ONLY while the Gaming session stays up (UI crash, UI restart).
# During a session teardown it must not start anything: steamos-powerbuttond.service has
# Requisite=gamescope-session.service, so a plain `systemctl start` adds a verify-active job for
# gamescope-session.service, and that job CANCELS gamescope's queued stop job (it waits behind
# steam-launcher for seconds, the unit still reads "active"). gamescope then outlives its logind
# session and loses DRM master: frozen panel, `drmModeAtomicCommit: Permission denied`.
# See docs/drm-root-cause-2026-11-15.md. The next session starts the stock daemon itself
# (gamescope-session.service.wants), so skipping here loses nothing.
set -eu
# The unit names are overridable only for session/tests/teardown-model.sh.
marker="${DECKSHELL_POWER_MARKER:-${XDG_RUNTIME_DIR:?}/deckshell-power-claimed}"
gs="${DECKSHELL_GS_SERVICE:-gamescope-session.service}"
target="${DECKSHELL_GS_TARGET:-gamescope-session.target}"
stock="${DECKSHELL_STOCK_POWER:-steamos-powerbuttond.service}"
[ -e "$marker" ] || exit 0

# The session stays up: its target is still active and nothing is queued for gamescope.
session_stays() {
    systemctl --user is-active --quiet "$target" || return 1
    systemctl --user is-active --quiet "$gs" || return 1
    [ -z "$(systemctl --user show -p Job --value "$gs" 2>/dev/null)" ]
}

if session_stays; then
    # --job-mode=fail: if a stop job appeared after the checks, fail instead of cancelling it.
    if timeout 5s systemctl --user --job-mode=fail start "$stock"; then
        echo '[power] independent recovery: stock daemon restored'
    else
        echo '[power] independent recovery: stock daemon not started (session is changing)'
    fi
else
    echo '[power] independent recovery: session teardown, stock daemon left to the next session'
fi
rm -f -- "$marker"
