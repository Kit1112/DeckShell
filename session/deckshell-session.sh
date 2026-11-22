#!/usr/bin/env bash
# DeckShell gaming-mode launcher.  ExecStart of the *user* unit steam-launcher.service
# (installed through a systemd drop-in by session/install.sh or session/install-root.sh).
#
# Facts (SteamOS holo, context/archive/docs/gamescope-integration.md): gamescope runs WITHOUT a client; Steam is
# the separate unit steam-launcher.service, so this script ending never ends the session.
# The unit's EnvironmentFile (%t/gamescope-environment) gives DISPLAY, GAMESCOPE_WAYLAND_DISPLAY, STEAM_*.
#
# Mode = first line of ~/.config/deckshell/mode (else /etc/deckshell/mode, else stock):
#   stock   exec /usr/lib/steamos/steam-launcher unchanged                  (== no DeckShell at all)
#   hybrid  Steam runs headless (-silent, no gamepadui) + DeckShell is the home screen
#           ("shell" is accepted as an alias)
#
# FAILSAFES -> stock Steam UI (`exec /usr/lib/steamos/steam-launcher` = `steam -gamepadui ...`):
#   1. env DECKSHELL_DISABLE=1
#   2. file ~/.config/deckshell/disable   or   /etc/deckshell/disable      (persistent)
#   3. file ~/.config/deckshell/disable-once                              (consumed on use)
#   4. KILL SWITCH: VOLUME-DOWN or B (Esc in lizard mode) held 2 s within the first 5 s of the session
#      (raw evdev, session/held-button.py; this boot only)
#   5. DeckShell binary missing / not loadable (glibc, libs)
#   6. WATCHDOG: DeckShell must be rendering AND focused by gamescope within WATCHDOG_SECS (10) of its start.
#      Proof = ready file written by DeckShell (src/gamescope.rs) or gamescope's own GAMESCOPE_FOCUSED_WINDOW
#      belonging to the DeckShell pid.  Otherwise -> stock now + ~/.config/deckshell/disable (black-screen guard).
#   7. CRASH LOOP: DeckShell exits non-zero (or dies within 10 s) twice within 60 s -> stock + disable file.
#   Automatic fallbacks (6, 7) never fire while a game runs: run_stock shuts Steam down, which ends the game.
# DeckShell exit codes: 0 restart, 42 the OWNER chose stock Steam UI, 43 switch to desktop mode,
#   44 internal failure (Power reader lost): restart DeckShell, never stock.
#
# STOCK UI GUARD (docs/steam-ui-parallel-2026-11-20.md). The stock Gamepad UI next to DeckShell came from a Steam
# client started WITHOUT -silent inside this session: Steam then logs "ComputeStartupMode: forcing gamepadui for
# steamdeck + gamescope". `steam -shutdown`, `steam.sh <url>` and `xdg-open steam://` start such a client when no
# Steam owns ~/.steam/steam.pipe. So: Steam is only ever started here and only with -silent; every other request
# goes over steam.pipe (never starts anything); and the supervisor checks the master Steam process (Valve's own
# test from steam.sh: pid in ~/.steam/steam.pid with steam.pipe open). A master without -silent is logged as
# "FOREIGN STEAM UI" with its parent chain, cgroup and Steam's own ExecCommandLine lines, then asked to exit over
# steam.pipe (deferred while a game runs) and replaced by the headless one.
# GAMEPAD UI WATCH (docs/steam-ui-never-2026-11-20.md): the headless client itself may still ask for its Gamepad UI
# window (STEAM press read from hidraw, start-up); see gamepadui_watch below. DeckShell's own X11 guard
# (app/src/steam_ui_guard.rs) covers the same from what gamescope shows.
set -u

# Run inside the transient unit, after systemd has assembled its environment (including
# the manager and EnvironmentFile). Never let stale test settings reach the UI.
if [ "${1:-}" = --ui-exec ]; then
    shift
    if [ "${DECKSHELL_ALLOW_TEST:-}" != 1 ]; then
        for key in ${!DECKSHELL_TEST_@} DECKSHELL_DRYRUN DECKSHELL_CONTROL; do unset "$key"; done
    fi
    exec "$@"
fi
# Do not pass test settings to Steam or copy them to the transient unit by default.
if [ "${DECKSHELL_ALLOW_TEST:-}" != 1 ]; then
    for key in ${!DECKSHELL_TEST_@} DECKSHELL_DRYRUN DECKSHELL_CONTROL; do unset "$key"; done
fi

CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/deckshell"
SYS_DIR="${DECKSHELL_SYS_DIR:-/etc/deckshell}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/deckshell"
LOG="$STATE_DIR/session.log"
STOCK="${DECKSHELL_STOCK_BIN:-/usr/lib/steamos/steam-launcher}"   # override only for tests
READY="${XDG_RUNTIME_DIR:-/tmp}/deckshell-ready"
mkdir -p "$CONF_DIR" "$STATE_DIR" 2>/dev/null

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# One writer owns rotation for the lifetime of the launcher and its UI pipe.
if command -v python3 >/dev/null 2>&1 && [ -f "$SELF_DIR/session-log.py" ]; then
    exec > >(python3 "$SELF_DIR/session-log.py" "$LOG") 2>&1
else
    exec >>"$LOG" 2>&1
fi
log() { printf '%s [session %s] %s\n' "$(date '+%F %T')" "$$" "$*"; }

STEAM_BIN="${STEAM_BIN:-/usr/bin/steam}"
# Full native Gamepad UI made physical Power work, but also consumed hidden
# controller input and played native UI sounds/animations over DeckShell.
# Our explicit power menu uses the OS backend; physical-key ownership still
# needs a separate, exclusive integration with steamos-powerbuttond.
STEAM_SHELL_ARGS="${STEAM_SHELL_ARGS:--steamos3 -silent}"
# Never let a configuration start the stock UI: drop UI-mode flags, require -silent.
headless_args() {
    local a out=()
    for a in $1; do case "$a" in -gamepadui|-bigpicture|-steampal|-tenfoot) ;; *) out+=("$a") ;; esac; done
    case " ${out[*]} " in *" -silent "*) ;; *) out+=(-silent) ;; esac
    echo "${out[*]}"
}
_args="$(headless_args "$STEAM_SHELL_ARGS")"
[ "$_args" = "$STEAM_SHELL_ARGS" ] || log "STEAM_SHELL_ARGS '$STEAM_SHELL_ARGS' -> '$_args' (Steam here is always headless)"
STEAM_SHELL_ARGS="$_args"; unset _args
STEAM_DIR="${DECKSHELL_STEAM_DIR:-$HOME/.steam}"
STEAM_PIPE="$STEAM_DIR/steam.pipe"
PROC="${DECKSHELL_PROC:-/proc}"   # override only for tests
STEAM_LOGS="${DECKSHELL_STEAM_LOGS:-$HOME/.local/share/Steam/logs}"
MAX_FAILS="${MAX_FAILS:-2}"
FAIL_WINDOW="${FAIL_WINDOW:-60}"
MIN_LIFE="${MIN_LIFE:-10}"
WATCHDOG_SECS="${WATCHDOG_SECS:-10}"
KS_WINDOW="${KS_WINDOW:-5}"
KS_HOLD="${KS_HOLD:-2}"
GSCTL="$SELF_DIR/gsctl.sh"; [ -x "$GSCTL" ] || GSCTL="$SELF_DIR/../tools/gamescope/gsctl.sh"

STEAM_PID=""; DS_PID=""; KS_PID=""

UI_UNIT=deckshell-ui.service
# The UI is its own transient unit, outside this script's cgroup. Stop the UNIT, not only the pid we
# started: a unit left over from an earlier launcher (crash, restart) would otherwise stay on screen
# next to the stock Steam UI, with both taking input.
stop_ui_unit() {
    command -v systemctl >/dev/null 2>&1 || return 0
    if systemctl --user is-active --quiet "$UI_UNIT" 2>/dev/null; then
        log "stopping $UI_UNIT"
        timeout 10s systemctl --user stop "$UI_UNIT" 2>/dev/null || systemctl --user kill --signal=KILL "$UI_UNIT" 2>/dev/null
    fi
    systemctl --user reset-failed "$UI_UNIT" 2>/dev/null
    return 0
}
stop_deckshell() {
    if [ -n "$DS_PID" ]; then
        kill "$DS_PID" 2>/dev/null
        local i; for i in 1 2 3 4 5 6 7 8; do kill -0 "$DS_PID" 2>/dev/null || break; sleep 0.25; done
        kill -KILL "$DS_PID" 2>/dev/null
        DS_PID=""
    fi
    stop_ui_unit
}
# DeckShell names itself as gamescope's base layer. With the window gone that entry selects nothing,
# and stock Steam does not rewrite it on start: nothing is focused. Without the atom gamescope
# picks focus itself, as in a session that never ran DeckShell.
release_base_layer() {
    [ -n "${DISPLAY:-}" ] && command -v xprop >/dev/null 2>&1 || return 0
    xprop -root -remove GAMESCOPECTRL_BASELAYER_APPID 2>/dev/null && log "base layer released"
    return 0
}
# --- Steam process identity and the pipe ----------------------------------------------------------
# The master Steam client, by Valve's definition (steam.sh is_steam_running): the pid in steam.pid that has
# steam.pipe open. Prints the pid or nothing.
steam_master_pid() {
    local p=""
    read -r p 2>/dev/null <"$STEAM_DIR/steam.pid"
    case "$p" in ''|*[!0-9]*) return 0 ;; esac
    [ -d "$PROC/$p/fd" ] || return 0
    [ -n "$(find "$PROC/$p/fd" -lname "$STEAM_PIPE" -print -quit 2>/dev/null)" ] && echo "$p"
    return 0
}
proc_args() { local -a v=(); mapfile -d '' -t v 2>/dev/null <"$PROC/$1/cmdline"; echo "${v[*]}"; }
# A client we started: -silent among its arguments (Steam: "ComputeStartupMode: forcing desktopui via -silent").
steam_is_headless() { case " $(proc_args "$1") " in *" -silent "*) return 0 ;; *) return 1 ;; esac; }
# A running game = a Steam reaper with SteamLaunch AppId=… (as steam_control::running_game). Prints the AppId.
game_running() {
    local c a; local -a v
    for c in "$PROC"/[0-9]*/cmdline; do
        v=(); mapfile -d '' -t v 2>/dev/null <"$c" || continue
        case "${v[0]:-}" in *reaper) ;; *) continue ;; esac
        a=" ${v[*]} "
        case "$a" in *" SteamLaunch AppId="*) a="${a#* SteamLaunch AppId=}"; echo "${a%% *}"; return 0 ;; esac
    done
    return 1
}
sq() { local s=$1; printf "'%s'" "${s//\'/\'\\\'\'}"; }   # g_shell_quote
# Deliver a command line to the RUNNING Steam (format of Valve's steam-runtime-steam-remote). Never starts Steam:
# no master -> not sent. The blocking open is bounded: if the reader vanished meanwhile, timeout ends it.
steam_pipe_send() {
    local a msg="'~/.steam/root/ubuntu12_32/steam'"
    for a in "$@"; do msg+=" $(sq "$a")"; done
    if [ ! -p "$STEAM_PIPE" ] || [ -z "$(steam_master_pid)" ]; then
        log "steam-ipc: '$*' NOT sent: no Steam reads $STEAM_PIPE (nothing started)"; return 2
    fi
    if timeout 3 bash -c 'printf "%s\n" "$1" >>"$2"' _ "$msg" "$STEAM_PIPE" 2>/dev/null; then
        log "steam-ipc: '$*' delivered"; return 0
    fi
    log "steam-ipc: '$*' NOT delivered (no reader within 3 s; nothing started)"; return 2
}
# Who started this Steam: everything needed to name the source next time.
steam_provenance() {
    local p="$1" n=0 st
    log "  pid=$p args: $(proc_args "$p")"
    log "  cgroup: $(cat "$PROC/$p/cgroup" 2>/dev/null | tr '\n' ' ')"
    while [ -n "$p" ] && [ "$p" != 0 ] && [ "$p" != 1 ] && [ "$n" -lt 8 ]; do
        st="$(sed -n 's/^PPid:[[:space:]]*//p' "$PROC/$p/status" 2>/dev/null)" || st=""
        [ -n "$st" ] || break
        p="$st"; n=$((n + 1))
        log "  parent[$n] pid=$p $(cat "$PROC/$p/comm" 2>/dev/null): $(proc_args "$p" | cut -c1-200)"
    done
    if [ -r "$STEAM_LOGS/console_log.txt" ]; then
        tail -n 4000 "$STEAM_LOGS/console_log.txt" 2>/dev/null | grep -a 'ExecCommandLine\|ComputeStartupMode' | tail -n 6 | sed 's/^/  steam: /'
    fi
    [ -r "$STEAM_LOGS/bootstrap_log.txt" ] && grep -a 'launched with' "$STEAM_LOGS/bootstrap_log.txt" 2>/dev/null | tail -n 4 | sed 's/^/  bootstrap: /'
    return 0
}
FOREIGN_PID=""; FOREIGN_GAME=""; FOREIGN_ASKED=-100
# A master Steam without -silent while DeckShell runs = the stock Gamepad UI on screen next to DeckShell.
foreign_steam() {
    local p="$1" g
    if [ "$p" != "$FOREIGN_PID" ]; then
        FOREIGN_PID="$p"; FOREIGN_GAME=""; FOREIGN_ASKED=-100
        log "FOREIGN STEAM UI: master Steam pid=$p was started without -silent (stock Gamepad UI next to DeckShell)"
        steam_provenance "$p"
    fi
    if g="$(game_running)"; then
        [ "$FOREIGN_GAME" = "$g" ] || { FOREIGN_GAME="$g"; log "FOREIGN STEAM UI pid=$p: game $g is running; exit request deferred until it ends"; }
        return 0
    fi
    if [ $((SECONDS - FOREIGN_ASKED)) -ge 20 ]; then
        FOREIGN_ASKED=$SECONDS
        log "FOREIGN STEAM UI pid=$p: no game; asking it to exit over steam.pipe, headless Steam follows"
        steam_pipe_send -shutdown || true
    fi
}
steam_shutdown() {
    local m; m="$(steam_master_pid)"
    [ -n "$STEAM_PID$m" ] || return 0
    # Over the pipe only: `steam -shutdown` STARTS a client when Steam is already gone (Deck 2026-11-18 14:03:34).
    steam_pipe_send -shutdown || true
    local i; for i in $(seq 15); do
        { [ -n "$STEAM_PID" ] && kill -0 "$STEAM_PID" 2>/dev/null; } || { [ -n "$m" ] && [ -d "$PROC/$m" ]; } || return 0
        sleep 1
    done
    log "steam did not exit in 15 s: SIGTERM"
    [ -n "$STEAM_PID" ] && kill "$STEAM_PID" 2>/dev/null
    [ -n "$m" ] && kill "$m" 2>/dev/null
    sleep 2
}
# --- Gamepad UI watch (docs/steam-ui-never-2026-11-20.md) -------------------------------------------
# The headless client itself sometimes asks for its Gamepad UI window: steamui_html.txt logs
# "CreateMainWindow: UIMode: 4" / "ensure gamepadui window exists" (Deck: 18.11 14:04:27, 17:57:56, 18:43:34;
# 19.11 19:48:41; 20.11 19:13:38), twice right after a STEAM press that Steam read from the Deck's hidraw.
# Every such line while the master is headless is logged with the controller context and answered with
# steam://close/bigpicture over steam.pipe (never starts Steam; a running game is not touched).
# DECKSHELL_STEAM_UI_GUARD=log only logs.
GPW_PID=""; GPW_LAST=-100
gamepadui_seen() {   # $1 = one steamui_html.txt line asking for the Gamepad UI window
    local m g
    m="$(steam_master_pid)"
    if [ -z "$m" ]; then log "GAMEPAD UI REQUESTED with no master Steam: $1"; return 0; fi
    steam_is_headless "$m" || return 0   # a stock master is foreign_steam's business
    g="$(game_running)" || g=none
    log "GAMEPAD UI REQUESTED by headless Steam pid=$m (game: $g): $1"
    if [ -r "$STEAM_LOGS/controller_ui.txt" ]; then
        tail -n 400 "$STEAM_LOGS/controller_ui.txt" 2>/dev/null | grep -a 'Guide button\|OnFocusWindowChanged' | tail -n 3 \
            | while IFS= read -r l; do log "  controller: $l"; done
    fi
    if [ "${DECKSHELL_STEAM_UI_GUARD:-close}" = log ]; then log "  close not requested (DECKSHELL_STEAM_UI_GUARD=log)"; return 0; fi
    if [ $((SECONDS - GPW_LAST)) -ge "${GPW_GAP:-3}" ]; then
        GPW_LAST=$SECONDS
        steam_pipe_send steam://close/bigpicture || true
    fi
}
gamepadui_watch() {   # polls the log's size; Steam rotates it to .previous at its start (size drops -> reread)
    local f="$STEAM_LOGS/steamui_html.txt" off sz l
    off="$(stat -c %s "$f" 2>/dev/null || echo 0)"
    while :; do
        sleep "${GPW_POLL:-1}"
        sz="$(stat -c %s "$f" 2>/dev/null || echo 0)"
        [ "$sz" -lt "$off" ] && off=0
        [ "$sz" -gt "$off" ] || continue
        while IFS= read -r l; do gamepadui_seen "$l"; done \
            < <(tail -c "+$((off + 1))" "$f" 2>/dev/null | head -c "$((sz - off))" | grep -a 'CreateMainWindow: UIMode: 4')
        off="$sz"
    done
}
start_gamepadui_watch() {
    [ -z "$GPW_PID" ] || return 0
    gamepadui_watch & GPW_PID=$!
    log "gamepad-ui watch pid=$GPW_PID on $STEAM_LOGS/steamui_html.txt (${DECKSHELL_STEAM_UI_GUARD:-close})"
}
stop_gamepadui_watch() { if [ -n "$GPW_PID" ]; then kill "$GPW_PID" 2>/dev/null; GPW_PID=""; fi; return 0; }
run_stock() {
    log "STOCK: $1 (game: $(game_running || echo none))"
    stop_gamepadui_watch   # the stock UI is wanted from here on
    [ -n "$KS_PID" ] && kill "$KS_PID" 2>/dev/null
    stop_deckshell
    steam_shutdown
    release_base_layer
    rm -f "$READY"
    exec "$STOCK"
}
on_term() { log "TERM: shutting down"; stop_gamepadui_watch; stop_deckshell; steam_shutdown; exit 0; }
trap on_term TERM INT HUP

# --- decide mode ---------------------------------------------------------------------------
MODE="$(head -n1 "$CONF_DIR/mode" 2>/dev/null | tr -d '[:space:]')"
[ -n "$MODE" ] || MODE="$(head -n1 "$SYS_DIR/mode" 2>/dev/null | tr -d '[:space:]')"
MODE="${MODE:-stock}"; [ "$MODE" = shell ] && MODE=hybrid
log "start: mode=$MODE"
[ "${DECKSHELL_DISABLE:-0}" = 1 ] && run_stock "DECKSHELL_DISABLE=1"
[ -e "$CONF_DIR/disable" ] && run_stock "marker $CONF_DIR/disable"
[ -e "$SYS_DIR/disable" ] && run_stock "marker $SYS_DIR/disable"
if [ -e "$CONF_DIR/disable-once" ]; then rm -f "$CONF_DIR/disable-once"; run_stock "marker disable-once"; fi
[ "$MODE" = stock ] && run_stock "mode=stock"
[ "$MODE" = hybrid ] || run_stock "unknown mode '$MODE'"

# --- kill switch: raw evdev monitor for the first KS_WINDOW s; hold KS_HOLD s => stock ------------
# Runs in the background; we wait only KS_HOLD+0.5 s here (catches a button already held at boot) and
# keep polling it while DeckShell starts.
ks_tripped() {   # 0 = kill switch fired
    [ -n "$KS_PID" ] || return 1
    kill -0 "$KS_PID" 2>/dev/null && return 1
    wait "$KS_PID"; local rc=$?; KS_PID=""
    [ "$rc" -eq 0 ]
}
if [ -f "$SELF_DIR/held-button.py" ] && command -v python3 >/dev/null 2>&1; then
    python3 "$SELF_DIR/held-button.py" --window "$KS_WINDOW" --hold "$KS_HOLD" &
    KS_PID=$!
    for _ in $(seq $((KS_HOLD * 10 + 5))); do kill -0 "$KS_PID" 2>/dev/null || break; sleep 0.1; done
    ks_tripped && run_stock "kill switch (volume-down / B held at boot)"
else
    log "kill switch unavailable (python3 or held-button.py missing)"
fi

find_deckshell() {
    local c
    for c in "${DECKSHELL_RUN:-}" "${DECKSHELL_BIN:-}" "$HOME/.local/share/deckshell/bin/deckshell-run"; do
        [ -n "$c" ] && [ -x "$c" ] && { readlink -f "$c"; return 0; }
    done; return 1
}
DECKSHELL="$(find_deckshell)" || run_stock "deckshell binary not found"
REAL="$(dirname "$DECKSHELL")/deckshell"; [ -x "$REAL" ] || REAL="$DECKSHELL"
if LD_TRACE_LOADED_OBJECTS=1 LD_LIBRARY_PATH="$(dirname "$DECKSHELL")/../lib" "$REAL" 2>&1 | grep -q 'not found\|version .GLIBC'; then
    run_stock "deckshell has unresolved libs / glibc symbols"
fi
log "binary=$DECKSHELL DISPLAY=${DISPLAY:-} GWL=${GAMESCOPE_WAYLAND_DISPLAY:-}"
[ -n "${DISPLAY:-}" ] || run_stock "DISPLAY not set (EnvironmentFile missing?)"

# --- Steam, headless ------------------------------------------------------------------------
start_steam() {
    # shellcheck disable=SC2086
    "$STEAM_BIN" $STEAM_SHELL_ARGS >>"$STATE_DIR/steam.log" 2>&1 &
    STEAM_PID=$!; log "steam (headless) pid=$STEAM_PID"
}
start_steam
start_gamepadui_watch

# Steam restart on request (account switch, task 430). DeckShell writes HOLD, runs `steam -shutdown`, edits the
# Steam login files while Steam is down and removes HOLD; only then is Steam started again, here, with the same
# headless arguments. A HOLD older than HOLD_MAX seconds is stale (DeckShell died mid-switch) and is ignored, so
# Steam never stays down. Without HOLD a dead Steam is also restarted (rate-limited).
HOLD="${DECKSHELL_STEAM_HOLD:-${XDG_RUNTIME_DIR:-/tmp}/deckshell-steam-hold}"
HOLD_MAX="${HOLD_MAX:-240}"
hold_active() {
    [ -e "$HOLD" ] || return 1
    local age=$(( $(date +%s) - $(stat -c %Y "$HOLD" 2>/dev/null || echo 0) ))
    [ "$age" -lt "$HOLD_MAX" ] && return 0
    log "steam hold is stale (${age}s): ignored"; rm -f "$HOLD"; return 1
}
STEAM_MASTER=""
# One supervisor tick: adopt a headless master, report/handle a foreign one, start Steam only when none runs.
# A launcher that only forwarded its arguments to a running master exits at once ("Steam is already running");
# that is not a dead Steam (it was restarted every 5 s on 2026-11-20 15:53-15:55 while the stock UI ran).
steam_tick() {
    local m; m="$(steam_master_pid)"
    if [ -n "$m" ]; then
        if steam_is_headless "$m"; then
            [ "$m" = "$STEAM_MASTER" ] || { STEAM_MASTER="$m"; log "steam master pid=$m (headless)"; }
            FOREIGN_PID=""
        else
            foreign_steam "$m"
        fi
        return 0
    fi
    kill -0 "$STEAM_PID" 2>/dev/null && return 0   # starting, or inside Steam's own restart
    hold_active && return 0
    [ $((SECONDS - ${STEAM_LAST_START:--100})) -lt 5 ] && return 0
    log "steam is not running while DeckShell is: starting it"; start_steam; STEAM_LAST_START=$SECONDS
}
supervise_steam() {   # until the UI unit ends; sets rc like `wait`
    while kill -0 "$RUN_PID" 2>/dev/null; do
        steam_tick
        sleep 0.5
    done
    wait "$RUN_PID"; rc=$?
}

# Is DeckShell actually on screen?  (a) its ready file for THIS pid, or (b) gamescope says the focused window is ours.
presenting() {
    [ -s "$READY" ] && [ "$(cut -d' ' -f1 "$READY" 2>/dev/null)" = "$DS_PID" ] && return 0
    command -v xprop >/dev/null 2>&1 || return 1
    local f
    f="$(xprop -root GAMESCOPE_FOCUSED_WINDOW 2>/dev/null | sed -n 's/.*= *\([0-9]\+\).*/\1/p')"
    [ -n "$f" ] && xprop -id "$f" _NET_WM_PID 2>/dev/null | grep -q "= $DS_PID\$"
}
diag() {
    command -v xprop >/dev/null 2>&1 || return 0
    { echo "--- diag: gamescope root atoms"; xprop -root 2>/dev/null | grep -E 'GAMESCOPE_(FOCUS|BASE)|GAMESCOPECTRL|STEAM_'
      echo "--- diag: windows"; "$GSCTL" probe 2>&1 | head -n 60; } 2>&1 | sed 's/^/  /'
}

fails=()
while :; do
    [ -e "$CONF_DIR/disable" ] && run_stock "marker appeared"
    steam_tick

    t0=$(date +%s); rm -f "$READY"
    # The UI itself is supervised: ExecStopPost restores stock Power even after
    # SIGKILL. No independent listener remains alive after the UI has gone.
    # Stopping or restarting the launcher or the session stops the UI. It must never be
    # RESTARTED with them (PartOf did that): the restarted UI ran beside whatever the new
    # launcher started, including stock Steam.
    stop_ui_unit
    extra_env=()
    for key in DECKSHELL_DIAG DECKSHELL_FRAMESTATS DECKSHELL_NOSOUND DECKSHELL_RASTER_ICONS DECKSHELL_ANIM_DIR DECKSHELL_MOCK DECKSHELL_STEAM_UI_GUARD DECKSHELL_KEEP_STEAM_GUIDE_FOCUS; do
        [ -z "${!key+x}" ] || extra_env+=("--setenv=$key=${!key}")
    done
    if [ "${DECKSHELL_ALLOW_TEST:-}" = 1 ]; then
        for key in DECKSHELL_ALLOW_TEST DECKSHELL_CONTROL DECKSHELL_DRYRUN ${!DECKSHELL_TEST_@}; do
            [ -z "${!key+x}" ] || extra_env+=("--setenv=$key=${!key}")
        done
    fi
    systemd-run --user --unit=deckshell-ui.service --service-type=exec --wait --collect --pipe \
        --property="StopPropagatedFrom=gamescope-session.service steam-launcher.service" \
        --property="Requisite=gamescope-session.service" --property="After=gamescope-session.service" \
        --property="EnvironmentFile=-${XDG_RUNTIME_DIR:?}/gamescope-environment" \
        --property="ExecStopPost=$SELF_DIR/power-recover.sh" --property="TimeoutStopSec=5s" \
        --setenv=DECKSHELL_FULLSCREEN=1 --setenv=DECKSHELL_GAMESCOPE=1 --setenv=DECKSHELL_GS_BASE=1 \
        --setenv="DECKSHELL_READY_FILE=$READY" --setenv="DECKSHELL_SESSION_MODE=$MODE" \
        --setenv=DECKSHELL_POWER_MANAGED=1 "${extra_env[@]}" "$SELF_DIR/deckshell-session.sh" --ui-exec "$DECKSHELL" &
    RUN_PID=$!
    DS_PID=""
    for _ in {1..50}; do
        DS_PID="$(systemctl --user show deckshell-ui.service -p MainPID --value 2>/dev/null)"
        [ "${DS_PID:-0}" != 0 ] && [ -n "$DS_PID" ] && break
        kill -0 "$RUN_PID" 2>/dev/null || break
        sleep 0.1
    done
    [ -n "$DS_PID" ] && [ "$DS_PID" != 0 ] || { wait "$RUN_PID"; run_stock "supervised UI did not start"; }
    log "deckshell pid=$DS_PID"

    deadline=$((SECONDS + WATCHDOG_SECS)); ready=0
    while kill -0 "$DS_PID" 2>/dev/null; do
        if [ "$ready" = 0 ]; then
            if presenting; then
                ready=1; log "deckshell presenting after $(( $(date +%s) - t0 )) s"
            elif [ "$SECONDS" -ge "$deadline" ] && g="$(game_running)"; then
                # The game owns focus: not a black screen, and stock would end the game.
                [ "${wd_game:-}" = "$g" ] || { wd_game="$g"; log "watchdog paused: game $g is running"; }
                deadline=$((SECONDS + WATCHDOG_SECS))
            elif [ "$SECONDS" -ge "$deadline" ]; then
                log "WATCHDOG: deckshell not presenting after ${WATCHDOG_SECS} s"; diag
                : >"$CONF_DIR/disable"
                run_stock "watchdog (black screen guard); wrote $CONF_DIR/disable"
            fi
        fi
        ks_tripped && run_stock "kill switch (volume-down / B held at boot)"
        if [ "$ready" = 1 ] && [ -z "$KS_PID" ]; then break; fi
        sleep 0.25
    done
    supervise_steam
    DS_PID=""
    now=$(date +%s); life=$((now - t0))
    log "deckshell exited rc=$rc after ${life}s"
    case "$rc" in
        42) run_stock "owner chose stock Steam UI in DeckShell (exit 42)" ;;
        43) log "switching to desktop"; stop_gamepadui_watch; steam_shutdown; exec steamos-session-select plasma ;;
        44) log "deckshell internal failure (exit 44, Power reader lost): restarting DeckShell, not stock" ;;
        0) ;;
        *) log "supervised UI failed (rc=$rc); stock Power restored by ExecStopPost; restarting DeckShell" ;;
    esac
    if [ "$rc" -ne 0 ] || [ "$life" -lt "$MIN_LIFE" ]; then
        kept=(); for t in "${fails[@]}"; do [ $((now - t)) -lt "$FAIL_WINDOW" ] && kept+=("$t"); done
        fails=("${kept[@]}" "$now")
        if [ "${#fails[@]}" -ge "$MAX_FAILS" ]; then
            if g="$(game_running)"; then
                log "STOCK DEFERRED: ${#fails[@]} failures in ${FAIL_WINDOW}s (rc=$rc) but game $g is running; retrying DeckShell in 5 s"
                fails=(); sleep 5; continue
            fi
            : >"$CONF_DIR/disable"
            run_stock "auto-fallback: ${#fails[@]} failures in ${FAIL_WINDOW}s (rc=$rc); wrote $CONF_DIR/disable"
        fi
    fi
    sleep 1
done
