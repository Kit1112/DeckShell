#!/usr/bin/env bash
# deck-session-restart.sh - the sanctioned way for an agent to restart the Gaming Mode session on the Deck.
# Runs ON the Deck as user deck (no root). The PC side calls it over ssh:
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh check'
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh restart gamescope'
#
# From stock Steam (after exit 42 or stock boot):
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh check from-stock'
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh from-stock restart gamescope'
#   (alias: enter-hybrid gamescope)
#
# To stock Steam (safely transition from hybrid to stock):
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh check to-stock'
#   ssh deck@192.168.1.50 'bash ~/.local/share/deckshell/session/deck-session-restart.sh to-stock restart gamescope'
#   (alias: enter-stock gamescope | leave-hybrid gamescope)
#
# Modes (no default action; a bare call only prints usage):
#   check [from-stock | to-stock]       read-only pre-checks and screenshot (~/.local/state/deckshell/restart-precheck.png)
#   restart gamescope                   pre-checks in hybrid, ONE session switch, post-checks.
#   from-stock restart gamescope        pre-checks in stock, ONE session switch into hybrid, post-checks.
#                                       (alias: enter-hybrid gamescope)
#   to-stock restart gamescope          pre-checks in hybrid, backup mode, ONE session switch into stock, post-checks.
#                                       (alias: enter-stock gamescope | leave-hybrid gamescope)
#   A failed switch stops the series; no automatic recovery switch.
# Exit: 0 checks passed (physical output unconfirmed) | 2 usage | 3 refused | 4 switch failed
#
# Why each check exists: docs/deck-session-restart-2026-11-15.md (the two failed switches of 2026-11-15).
set -u

usage() {
    echo 'usage: deck-session-restart.sh check [from-stock | to-stock] | restart gamescope | from-stock restart gamescope | to-stock restart gamescope' >&2
    exit 2
}

FROM_STOCK=0
TO_STOCK=0
case "${1:-}" in
    check)
        if [ "$#" -eq 1 ]; then
            MODE=check
        elif [ "$#" -eq 2 ] && [ "${2:-}" = from-stock ]; then
            MODE=check
            FROM_STOCK=1
        elif [ "$#" -eq 2 ] && [ "${2:-}" = to-stock ]; then
            MODE=check
            TO_STOCK=1
        else
            usage
        fi
        ;;
    from-stock)
        FROM_STOCK=1
        shift
        case "${1:-}" in
            check) [ "$#" -eq 1 ] || usage; MODE=check ;;
            restart) [ "$#" -eq 2 ] && [ "${2:-}" = gamescope ] || usage; MODE=restart ;;
            *) usage ;;
        esac
        ;;
    enter-hybrid)
        FROM_STOCK=1
        shift
        case "${1:-}" in
            check) [ "$#" -eq 1 ] || usage; MODE=check ;;
            gamescope) [ "$#" -eq 1 ] || usage; MODE=restart ;;
            restart) [ "$#" -eq 2 ] && [ "${2:-}" = gamescope ] || usage; MODE=restart ;;
            *) usage ;;
        esac
        ;;
    to-stock)
        TO_STOCK=1
        shift
        case "${1:-}" in
            check) [ "$#" -eq 1 ] || usage; MODE=check ;;
            gamescope) [ "$#" -eq 1 ] || usage; MODE=restart ;;
            restart) [ "$#" -eq 2 ] && [ "${2:-}" = gamescope ] || usage; MODE=restart ;;
            *) usage ;;
        esac
        ;;
    enter-stock|leave-hybrid)
        TO_STOCK=1
        shift
        case "${1:-}" in
            check) [ "$#" -eq 1 ] || usage; MODE=check ;;
            gamescope) [ "$#" -eq 1 ] || usage; MODE=restart ;;
            restart) [ "$#" -eq 2 ] && [ "${2:-}" = gamescope ] || usage; MODE=restart ;;
            *) usage ;;
        esac
        ;;
    restart)
        [ "$#" -eq 2 ] && [ "${2:-}" = gamescope ] || usage
        MODE=restart
        ;;
    *)
        usage
        ;;
esac

# A non-login ssh (right after a reboot, or without pam_systemd) has neither XDG_RUNTIME_DIR nor
# DBUS_SESSION_BUS_ADDRESS: every `systemctl --user` then fails and a live UI looks "not active".
RT="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export XDG_RUNTIME_DIR="$RT"
[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] || export DBUS_SESSION_BUS_ADDRESS="unix:path=$RT/bus"
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/deckshell"
CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/deckshell"
SYS_DIR="${DECKSHELL_SYS_DIR:-/etc/deckshell}"
LOG="$STATE/session.log"                 # DeckShell's own log, read-only here
RESTART_LOG="$STATE/session-restart.log"
LAST="$STATE/session-restart.last"       # epoch of the last switch attempt
READY="$RT/deckshell-ready"              # "<ui pid> <xid> <ms>"
CLAIM="$RT/deckshell-power-claimed"
INCOMPLETE="$RT/deck-session-restart-incomplete" # blocks another attempt during this boot after failure/interruption
# The UI's ExecStopPost hook (installed beside this script). An old copy cancels gamescope's stop job
# during the switch and leaves a gamescope without a logind session: docs/drm-root-cause-2026-11-15.md.
POWER_HOOK="${POWER_HOOK:-$(dirname "$(readlink -f "$0")")/power-recover.sh}"
BASE_APPID=233594385                     # 0x0DEC5E11: DeckShell's base layer (home screen)
STEAM_APPID=769                          # Valve's Gamepad UI / Steam client
DECKSHELL_BIN="${DECKSHELL_BIN:-$HOME/.local/share/deckshell/bin/deckshell}"
DECKSHELL_RUN="${DECKSHELL_RUN:-$HOME/.local/share/deckshell/bin/deckshell-run}"
export DISPLAY="${DISPLAY:-:0}"
SHOT="${SHOT_FILE:-/tmp/gamescope.png}"  # gamescope writes its screenshot here (same as gsctl.sh)
MIN_GAP="${MIN_GAP:-120}"                # s since the last switch and since the current gamescope started
UI_MIN_AGE="${UI_MIN_AGE:-60}"           # s the DeckShell UI must have been up (not starting or stopping)
INPUT_QUIET="${INPUT_QUIET:-600}"        # s without a physical press on the Deck
POST_WAIT="${POST_WAIT:-30}"             # s of the counter window after the switch (20-40 s)
SESSION_WAIT="${SESSION_WAIT:-120}"      # polls of 0.5 s for the new session (60 s)
OLD_WAIT="${OLD_WAIT:-60}"               # polls of 0.5 s for the old gamescope to exit (30 s)
mkdir -p "$STATE" 2>/dev/null

FAILS=0
GS_PIDS=(); OLD_GS=(); UI_PID=""; T0=0
log() { printf '%s %s\n' "$(date '+%F %T')" "$*" | tee -a "$RESTART_LOG"; }
ok()  { log "  ok    $*"; }
bad() { log "  FAIL  $*"; FAILS=$((FAILS + 1)); }

# "Permission denied" lines of this boot (optionally since epoch $1): the DRM master loss counter.
pd_count() {
    local args=(--user -b --no-pager)
    [ -n "${1:-}" ] && args+=(--since "@$1")
    local output
    output=$(journalctl "${args[@]}" 2>/dev/null) || return 1
    printf '%s\n' "$output" | grep -c "Permission denied" || :
}
# gamescope's cmdline starts with "gamescope " but its comm is "gamescope-wl"; accept either match.
gs_pids() { { pgrep -f '^gamescope ' 2>/dev/null; pgrep -u "$(id -u)" -x 'gamescope-wl' 2>/dev/null; } | sort -un; }
focused_app() { xprop -root GAMESCOPE_FOCUSED_APP 2>/dev/null | sed -n 's/.*= *\([0-9][0-9]*\).*/\1/p'; }
# Windows whose STEAM_GAME app id is neither Steam's client (769) nor DeckShell's base (233594385): a game is on screen,
# even when DeckShell's QAM is open over it and the focus stays on the base layer.
game_windows() {
    local w v
    for w in $(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+ /{print $1}'); do
        v=$(xprop -id "$w" STEAM_GAME 2>/dev/null | sed -n 's/.*= *\([0-9][0-9]*\).*/\1/p')
        case "$v" in ""|0|769|"$BASE_APPID") ;; *) echo "$w:$v" ;; esac
    done
}
find_deckshell() {
    local c
    for c in "${DECKSHELL_RUN:-}" "${DECKSHELL_BIN:-}" "$HOME/.local/share/deckshell/bin/deckshell" "$HOME/.local/share/deckshell/bin/deckshell-run"; do
        [ -n "$c" ] && [ -x "$c" ] && return 0
    done
    return 1
}
reaper_count() { pgrep -f 'reaper.*SteamLaunch' 2>/dev/null | wc -l; }
ui_active() { systemctl --user is-active --quiet deckshell-ui.service; }
ui_main_pid() { systemctl --user show -p MainPID --value deckshell-ui.service 2>/dev/null; }
age_of() { ps -o etimes= -p "$1" 2>/dev/null | tr -d ' '; }
# alive = present and not a zombie (a killed child that nobody has reaped yet counts as gone)
pid_alive() { [ -d "/proc/$1" ] && ! grep -q '^State:[[:space:]]*Z' "/proc/$1/status" 2>/dev/null; }

# gamescope screenshot (type 0 = full output), copied to $1. Non-empty PNG > 20 kB counts as a frame.
shot() {
    rm -f "$SHOT"
    xprop -root -f GAMESCOPECTRL_REQUEST_SCREENSHOT 32c -set GAMESCOPECTRL_REQUEST_SCREENSHOT 0 2>/dev/null || return 1
    local i; for i in $(seq 16); do sleep 0.5; [ -s "$SHOT" ] && break; done
    [ -s "$SHOT" ] && cp -- "$SHOT" "$1" && [ "$(stat -c %s "$1")" -gt 20000 ]
}

# Every pre-check. Fills GS_PIDS and UI_PID; FAILS counts the refusals.
prechecks() {
    local now_s now_ms n age last hid pts vpad helpers reapers rpid focus gw
    now_s=$(date +%s); now_ms=$(date +%s%3N)
    FAILS=0

    if [ -d "$RT" ]; then ok "user runtime dir $RT"; else bad "no user runtime dir $RT (not a Deck session?)"; fi

    if [ -e "$INCOMPLETE" ]; then bad "previous switch failed or was interrupted this boot; stop and investigate"; else ok "no incomplete switch this boot"; fi

    if [ ! -e "$POWER_HOOK" ]; then ok "no UI power hook installed ($POWER_HOOK)"
    elif grep -q -- '--job-mode=fail' "$POWER_HOOK"; then ok "UI power hook is teardown-safe"
    else bad "UI power hook $POWER_HOOK starts steamos-powerbuttond during the switch (cancels gamescope's stop job): install the fixed session/power-recover.sh first"; fi

    if ! n=$(pd_count); then bad "cannot read user journal; DRM readiness unknown"; n=unknown; fi
    if [ "$n" = 0 ]; then ok "'Permission denied' this boot: 0"
    else bad "'Permission denied' this boot: $n (DRM master lost; a switch cannot fix it, see docs)"; fi

    if n=$(journalctl --user -b --no-pager --since "@$((now_s - 600))" 2>/dev/null); then
        n=$(printf '%s\n' "$n" | grep -c "Could not take device" || :)
    else bad "cannot read recent journal; logind readiness unknown"; n=unknown; fi
    if [ "$n" = 0 ]; then ok "no 'Could not take device' in the last 10 min"
    else bad "'Could not take device' in the last 10 min: $n (logind session already gone)"; fi

    GS_PIDS=(); while read -r p; do [ -n "$p" ] && GS_PIDS+=("$p"); done < <(gs_pids)
    if [ "${#GS_PIDS[@]}" = 1 ]; then ok "one gamescope process: ${GS_PIDS[0]}"
    else bad "gamescope processes: ${#GS_PIDS[@]} (want 1): ${GS_PIDS[*]:-none}"; fi

    last=0; [ -s "$LAST" ] && last=$(cat "$LAST")
    if [ "$last" = 0 ]; then ok "no earlier switch recorded"
    elif [ $((now_s - last)) -ge "$MIN_GAP" ]; then ok "last switch attempt $((now_s - last)) s ago"
    else bad "last switch attempt $((now_s - last)) s ago (min $MIN_GAP)"; fi

    age=$(age_of "${GS_PIDS[0]:-0}")
    if [ -n "$age" ] && [ "$age" -ge "$MIN_GAP" ]; then ok "gamescope up ${age} s"
    else bad "gamescope up ${age:-?} s (min $MIN_GAP): the session is still starting or stopping"; fi

    if [ "$FROM_STOCK" = 1 ]; then
        if ui_active; then
            bad "deckshell-ui is active: already in hybrid session (use normal restart gamescope)"
        else
            ok "deckshell-ui not active (stock session verified)"
        fi
        UI_PID=""

        if [ -e "$READY" ]; then
            bad "stale ready marker $READY present while UI is not active"
        else
            ok "no stale ready marker"
        fi

        if find_deckshell; then
            ok "deckshell binary installed"
        else
            bad "deckshell binary missing or not executable: install package first"
        fi

        if [ -e "$CONF_DIR/disable" ]; then
            bad "DeckShell is disabled ($CONF_DIR/disable present)"
        elif [ -e "$SYS_DIR/disable" ]; then
            bad "DeckShell is disabled ($SYS_DIR/disable present)"
        else
            ok "no disable marker"
        fi

        if [ -e "$CLAIM" ]; then
            bad "stale power marker $CLAIM (UI not running): clear before starting hybrid"
        else
            ok "power marker consistent"
        fi

        focus=$(focused_app)
        case "$focus" in
            ""|0|"$STEAM_APPID") ok "focus is stock Steam (app ${focus:-none})" ;;
            "$BASE_APPID") bad "focused app is DeckShell base layer $BASE_APPID (unexpected in stock session)" ;;
            *) bad "focused app '${focus:-none}' is a game or external app (want stock Steam $STEAM_APPID)" ;;
        esac
    elif [ "$TO_STOCK" = 1 ]; then
        local cur_mode
        cur_mode="$(head -n1 "$CONF_DIR/mode" 2>/dev/null | tr -d '[:space:]')"
        [ -n "$cur_mode" ] || cur_mode="$(head -n1 "$SYS_DIR/mode" 2>/dev/null | tr -d '[:space:]')"
        cur_mode="${cur_mode:-stock}"
        if [ "$cur_mode" = stock ] && ! ui_active; then
            bad "already in stock session (mode=stock and deckshell-ui not active)"
        else
            ok "not in stock session (mode=$cur_mode, ui_active=$(ui_active && echo true || echo false))"
        fi

        if ui_active; then
            UI_PID="$(ui_main_pid)"
            ok "deckshell-ui active (pid $UI_PID)"
        else
            UI_PID=""
            ok "deckshell-ui not active"
        fi

        if find_deckshell; then
            ok "deckshell binary installed (verifiable return to hybrid available)"
        else
            bad "deckshell binary missing or not executable (cannot verify return to hybrid)"
        fi

        focus=$(focused_app)
        case "$focus" in
            ""|0|"$STEAM_APPID"|"$BASE_APPID")
                ok "focused app is safe for stock transition (${focus:-none})"
                ;;
            *)
                bad "focused app '${focus:-none}' is a game or external app (refusing to switch to stock)"
                ;;
        esac
    else
        if ui_active; then ok "deckshell-ui active"; else bad "deckshell-ui not active (if in stock Steam, use 'deck-session-restart.sh from-stock check' or 'check from-stock')"; fi
        UI_PID="$(ui_main_pid)"
        age=$(age_of "${UI_PID:-0}")
        if [ -n "$age" ] && [ "$age" -ge "$UI_MIN_AGE" ]; then ok "UI pid $UI_PID up ${age} s"
        else bad "UI up ${age:-?} s (min $UI_MIN_AGE): still starting or stopping"; fi
        rpid=$(cut -d' ' -f1 "$READY" 2>/dev/null)
        if [ -n "$rpid" ] && [ "$rpid" = "$UI_PID" ]; then ok "ready marker names UI pid $rpid"
        else bad "ready marker pid '${rpid:-none}' differs from UI pid '${UI_PID:-none}'"; fi
        if [ -e "$CLAIM" ] && ! ui_active; then bad "stale power marker $CLAIM (UI not running): DeckShell would exit 42 at once"
        else ok "power marker consistent"; fi

        focus=$(focused_app)
        if [ "$focus" = "$BASE_APPID" ]; then ok "focus is DeckShell home (base layer $BASE_APPID)"
        else bad "focused app '${focus:-none}' is not DeckShell home: a game or another app is on top"; fi
    fi

    gw=$(game_windows)
    if [ -z "$gw" ]; then ok "no game window (STEAM_GAME) on screen"
    else bad "game window(s) on screen (window:appid): $(echo $gw) (a game is running)"; fi

    reapers=$(reaper_count)
    if [ "$reapers" = 0 ]; then ok "no game reaper running"
    else bad "$reapers game reaper process(es) running: Steam game launcher is active"; fi

    if grep '"event":"native_recording.observation"' "$LOG" 2>/dev/null | tail -n1 | grep -q '"active_session_observed":true'; then
        bad "a recording session is active (native_recording.observation)"
    elif grep '"event":"native_recording.state"' "$LOG" 2>/dev/null | tail -n1 | grep -q '"command_pending":true'; then
        bad "a recording command is pending (native_recording.state)"
    else ok "no recording in progress"; fi

    hid=$(grep '"event":"record_input.hid"' "$LOG" 2>/dev/null | tail -n1 | grep -o '"ts_ms":[0-9]*' | cut -d: -f2)
    if [ -z "$hid" ]; then ok "no physical press in the DeckShell log"
    else
        age=$(( (now_ms - hid) / 1000 ))
        if [ "$age" -ge "$INPUT_QUIET" ]; then ok "last physical press ${age} s ago"
        else bad "physical press ${age} s ago (min quiet $INPUT_QUIET): the owner may be at the Deck"; fi
    fi

    # ssh sessions of user deck: "sshd-session: deck@notty" (non-tty, what agents use) or "deck@pts/N".
    # 'who' misses non-tty sessions, so it is not used.
    pts=$(pgrep -u "$(id -un)" -f '^sshd-session: deck@' 2>/dev/null | wc -l)
    if [ "$pts" -le 1 ]; then ok "ssh sessions of deck: $pts (this one)"
    else bad "ssh sessions of deck: $pts: another session is connected"; fi

    vpad=$(systemctl --user list-units --no-legend --state=active 'vpad*' 2>/dev/null | wc -l)
    if [ "$vpad" = 0 ]; then ok "no vpad unit running"
    else bad "vpad unit active: Steam Input could see a virtual pad"; fi

    helpers=$(pgrep -f 'wineserver|vulkandriverquery|hellovr|GE-Proton|proton waitforexitandrun' 2>/dev/null | wc -l)
    if [ "$helpers" = 0 ]; then ok "no Wine/Proton/vulkan helper running"
    else bad "$helpers Wine/Proton helper process(es) running: Steam starts these right before a session change"; fi
}

# New gamescope and the UI back on the new pid (or stock session up). Waits up to 60 s.
wait_new_session() {
    local i p new=() ui n
    for i in $(seq "$SESSION_WAIT"); do
        sleep 0.5
        # the stale gamescope logs 'Permission denied' within ~0.2 s of the stop: stop waiting at once
        if [ $((i % 4)) = 0 ]; then
            if ! n=$(pd_count "$T0"); then bad "cannot read journal after switch; DRM health unknown"; return 1; fi
            if [ "$n" -gt 0 ]; then bad "'Permission denied' since the switch: $n (DRM health failed)"; return 1; fi
        fi
        new=()
        while read -r p; do
            [ -n "$p" ] || continue
            case " ${OLD_GS[*]} " in *" $p "*) ;; *) new+=("$p") ;; esac
        done < <(gs_pids)
        if [ "$TO_STOCK" = 1 ]; then
            if [ "${#new[@]}" = 1 ] && ! ui_active; then
                log "  new gamescope ${new[0]}, stock session up (deckshell-ui inactive)"
                return 0
            fi
        else
            ui="$(ui_main_pid)"
            if [ "${#new[@]}" = 1 ] && [ -n "$ui" ] && [ "$ui" != "$OLD_UI" ] && ui_active \
               && [ "$(cut -d' ' -f1 "$READY" 2>/dev/null)" = "$ui" ]; then
                log "  new gamescope ${new[0]}, UI $ui (was ${OLD_UI:-none})"
                return 0
            fi
        fi
    done
    if [ "$TO_STOCK" = 1 ]; then
        bad "new stock session not up in time: gamescope pids '${new[*]:-none}', ui active: $(ui_active && echo yes || echo no)"
    else
        bad "new session not up in time: gamescope pids '${new[*]:-none}', UI '${ui:-none}'"
    fi
    return 1
}

# The old gamescope must be gone; its presence makes the session transition incomplete.
old_gone() {
    local i p alive
    for i in $(seq "$OLD_WAIT"); do
        alive=()
        for p in "${OLD_GS[@]}"; do pid_alive "$p" && alive+=("$p"); done
        [ "${#alive[@]}" = 0 ] && { ok "old gamescope gone (${OLD_GS[*]:-none})"; return 0; }
        sleep 0.5
    done
    bad "old gamescope still alive: ${alive[*]} (session transition incomplete)"; return 1
}

# Counter window, STOCK fallback check, and a frame from gamescope.
post_checks() {
    local n stock t focus
    sleep "$POST_WAIT"
    if ! n=$(pd_count "$T0"); then bad "cannot read journal after switch; DRM health unknown"; n=unknown; fi
    if [ "$n" = 0 ]; then ok "'Permission denied' since the switch, ${POST_WAIT} s window: 0"
    else bad "'Permission denied' since the switch: $n"; fi

    if [ "$TO_STOCK" = 1 ]; then
        if ui_active; then
            bad "deckshell-ui is still active after switch to stock"
        else
            ok "deckshell-ui inactive (stock verified)"
        fi
        if [ -e "$READY" ]; then
            bad "stale ready marker $READY present in stock session"
        else
            ok "no stale ready marker in stock session"
        fi
        if [ "$(head -n1 "$CONF_DIR/mode" 2>/dev/null | tr -d '[:space:]')" = stock ]; then
            ok "mode confirmed stock in $CONF_DIR/mode"
        else
            bad "mode in $CONF_DIR/mode is not stock"
        fi
        focus=$(focused_app)
        case "$focus" in
            ""|0|"$STEAM_APPID") ok "focus is stock Steam (app ${focus:-none})" ;;
            "$BASE_APPID") bad "focused app is DeckShell base layer $BASE_APPID in stock session" ;;
            *) bad "focused app '${focus:-none}' is unexpected app or game" ;;
        esac
        ok "verifiable return path available via 'from-stock restart gamescope'"
    else
        t="$(date -d "@$T0" '+%F %T')"
        stock=$(awk -v t="$t" '$1" "$2 >= t && /STOCK:/' "$LOG" 2>/dev/null | wc -l)
        if [ "$stock" = 0 ]; then ok "no fallback to stock Steam"
        else bad "DeckShell fell back to stock Steam ($stock STOCK line(s))"; fi
    fi

    if shot "$STATE/restart-postcheck.png"; then ok "gamescope frame ok: $STATE/restart-postcheck.png"
    else bad "no gamescope screenshot after the switch"; fi
}

switch() {
    T0=$(date +%s)
    echo "$T0" >"$LAST"
    if ! printf '%s\n' "$T0" >"$INCOMPLETE"; then bad "cannot record incomplete switch; refusing to switch"; return 1; fi
    log "SWITCH: steamos-session-select gamescope (old gamescope: ${OLD_GS[*]:-none}, old UI: ${OLD_UI:-none})"
    timeout 90 steamos-session-select gamescope >>"$RESTART_LOG" 2>&1
    local rc=$?
    [ "$rc" = 0 ] || bad "steamos-session-select rc=$rc"
    if [ "$rc" = 0 ] && wait_new_session; then old_gone; post_checks; fi
    [ "$FAILS" = 0 ]
}

if [ "$MODE" = check ]; then
    if [ "$FROM_STOCK" = 1 ]; then
        log "CHECK (from-stock) $(date '+%F %T')"
    elif [ "$TO_STOCK" = 1 ]; then
        log "CHECK (to-stock) $(date '+%F %T')"
    else
        log "CHECK $(date '+%F %T')"
    fi
    prechecks
    if shot "$STATE/restart-precheck.png"; then
        if [ "$FROM_STOCK" = 1 ]; then
            ok "screenshot: $STATE/restart-precheck.png (look at it: stock home screen, no game running)"
        elif [ "$TO_STOCK" = 1 ]; then
            ok "screenshot: $STATE/restart-precheck.png (look at it: safe screen before switching to stock, no game running)"
        else
            ok "screenshot: $STATE/restart-precheck.png (look at it: home screen, no QAM or menu open)"
        fi
    else bad "no gamescope screenshot"; fi
    if [ "$FAILS" = 0 ]; then log "RESULT: READY (the owner's ready-to-switch conditions hold)"; exit 0; fi
    log "RESULT: NOT READY, $FAILS check(s) failed; nothing was changed"; exit 3
fi

# restart gamescope
exec 9>"$RT/deck-session-restart.lock"
flock -n 9 || { log "REFUSED: another restart is running"; exit 3; }
if [ "$FROM_STOCK" = 1 ]; then
    log "RESTART (from-stock) $(date '+%F %T')"
elif [ "$TO_STOCK" = 1 ]; then
    log "RESTART (to-stock) $(date '+%F %T')"
else
    log "RESTART $(date '+%F %T')"
fi
prechecks
shot "$STATE/restart-precheck.png" || bad "no pre-restart screenshot"
if [ "$FAILS" != 0 ]; then log "REFUSED: $FAILS pre-check(s) failed; nothing was changed"; exit 3; fi
OLD_GS=("${GS_PIDS[@]}"); OLD_UI="$UI_PID"

if [ "$FROM_STOCK" = 1 ]; then
    mkdir -p "$CONF_DIR" 2>/dev/null
    echo hybrid >"$CONF_DIR/mode"
    rm -f "$CONF_DIR/disable-once"
elif [ "$TO_STOCK" = 1 ]; then
    mkdir -p "$CONF_DIR" 2>/dev/null
    if [ -f "$CONF_DIR/mode" ]; then
        cp -f "$CONF_DIR/mode" "$CONF_DIR/mode.bak" 2>/dev/null && ok "backed up $CONF_DIR/mode -> $CONF_DIR/mode.bak" || bad "failed to backup $CONF_DIR/mode"
    else
        echo hybrid >"$CONF_DIR/mode.bak" && ok "created fallback $CONF_DIR/mode.bak (hybrid)"
    fi
    echo stock >"$CONF_DIR/mode"
    log "set mode=stock in $CONF_DIR/mode (backup: $CONF_DIR/mode.bak)"
fi

if switch; then
    rm -f "$INCOMPLETE" || { log "RESULT: FAILED: cannot clear incomplete marker"; exit 4; }
    log "RESULT: OK (checks passed; physical output not confirmed)"
    exit 0
fi
log "RESULT: FAILED. No automatic recovery or second switch; physical output is not confirmed. Investigate with the owner; logind reboot may require interactive authentication."
exit 4
