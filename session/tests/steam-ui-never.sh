#!/usr/bin/env bash
# Gamepad UI watch of session/deckshell-session.sh (docs/steam-ui-never-2026-11-20.md).
# Fake /proc, a real FIFO for steam.pipe, fake Steam logs; touches nothing outside a temp dir, never runs Steam.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; SRC="$HERE/../deckshell-session.sh"
T="$(mktemp -d)"; READER=""; WATCH=""
cleanup() {
    { [ -n "$READER" ] && kill "$READER" 2>/dev/null; } || true
    { [ -n "$WATCH" ] && kill "$WATCH" 2>/dev/null; } || true
    rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $1" >&2; [ -f "$T/log" ] && sed 's/^/  log: /' "$T/log" >&2; exit 1; }

log() { echo "$*" >>"$T/log"; }
STEAM_DIR="$T/steam"; STEAM_PIPE="$STEAM_DIR/steam.pipe"; PROC="$T/proc"; STEAM_LOGS="$T/logs"
eval "$(sed -n '/^steam_master_pid() {/,/^}/p;/^proc_args() {/p;/^steam_is_headless() {/p;/^game_running() {/,/^}/p;/^sq() {/p;/^steam_pipe_send() {/,/^}/p;/^GPW_PID=/p;/^gamepadui_seen() {/,/^}/p;/^gamepadui_watch() {/,/^}/p' "$SRC")"
declare -F gamepadui_seen >/dev/null && declare -F gamepadui_watch >/dev/null || fail "watch functions missing from the session script"

mkproc() {   # pid ppid comm args...
    local pid=$1 ppid=$2 comm=$3; shift 3
    mkdir -p "$T/proc/$pid/fd"
    printf '%s\0' "$@" >"$T/proc/$pid/cmdline"
    printf 'Name:\t%s\nPPid:\t%s\n' "$comm" "$ppid" >"$T/proc/$pid/status"
    echo "$comm" >"$T/proc/$pid/comm"
}
reset() {
    { [ -n "$READER" ] && kill "$READER" 2>/dev/null; } || true; READER=""
    rm -rf "$T/proc" "$T/steam" "$T/log" "$T/pipe.out" "$T/logs"; mkdir -p "$T/proc" "$T/steam" "$T/logs"; : >"$T/log"
    GPW_LAST=-100; unset DECKSHELL_STEAM_UI_GUARD
    # The Deck's own lines (controller_ui.txt, 2026-11-18 17:57).
    printf '%s\n' "[2026-11-18 17:57:56] OnFocusWindowChanged to window type: k_nGameIDControllerConfigs_ClientUI, AppID 769" \
        "[2026-11-18 17:57:56] Guide button sent to JS" >"$T/logs/controller_ui.txt"
}
reader() { mkfifo "$T/steam/steam.pipe"; cat "$T/steam/steam.pipe" >"$T/pipe.out" & READER=$!; sleep 0.1; }
master() {   # pid args...
    local pid=$1; shift
    mkproc "$pid" 900 steam "$@"; ln -s "$T/steam/steam.pipe" "$T/proc/$pid/fd/3"; echo "$pid" >"$T/steam/steam.pid"
}
CLOSE="'~/.steam/root/ubuntu12_32/steam' 'steam://close/bigpicture'"
LINE="[2026-11-20 19:13:38] CreateMainWindow: UIMode: 4"
set +e

# 1. Headless master asks for its Gamepad UI window: reported with the controller context, closed over the pipe.
reset; reader; master 4309 /home/deck/.local/share/Steam/steamrt64/steam -steamdeck -steamos3 -silent
gamepadui_seen "$LINE"
grep -qF "GAMEPAD UI REQUESTED by headless Steam pid=4309 (game: none): $LINE" "$T/log" || fail "request not reported"
grep -q "controller: .*Guide button sent to JS" "$T/log" || fail "controller context missing"
sleep 0.2; [ "$(cat "$T/pipe.out")" = "$CLOSE" ] || fail "close not delivered: $(cat "$T/pipe.out")"
# The same burst (Steam logs several lines per request) is answered once.
gamepadui_seen "$LINE"; sleep 0.2
[ "$(grep -c 'close/bigpicture' "$T/pipe.out")" = 1 ] || fail "close repeated inside the gap"

# 2. During a game: the request is still closed (the master keeps running, so does the game) and the game is named.
reset; reader; master 4309 /home/deck/.local/share/Steam/steamrt64/steam -steamos3 -silent
mkproc 50000 4309 reaper /home/deck/.local/share/Steam/steamrt64/reaper SteamLaunch AppId=367520 -- /game
gamepadui_seen "$LINE"; sleep 0.2
grep -qF "(game: 367520)" "$T/log" || fail "running game not named"
[ "$(cat "$T/pipe.out")" = "$CLOSE" ] || fail "close not delivered during a game"
grep -q -- "-shutdown" "$T/pipe.out" && fail "Steam asked to exit during a game"

# 3. A stock master (no -silent) is foreign_steam's business: the watch neither logs nor writes.
reset; reader; master 47800 /home/deck/.local/share/Steam/steamrt64/steam -steamos3 -gamepadui
gamepadui_seen "$LINE"; sleep 0.2
[ ! -s "$T/log" ] || fail "stock master handled by the watch"
[ ! -s "$T/pipe.out" ] || fail "written to a stock master"

# 4. Log-only mode.
reset; reader; master 4309 /home/deck/.local/share/Steam/steamrt64/steam -steamos3 -silent
DECKSHELL_STEAM_UI_GUARD=log gamepadui_seen "$LINE"; sleep 0.2
grep -q "GAMEPAD UI REQUESTED" "$T/log" || fail "log-only mode did not log"
[ ! -s "$T/pipe.out" ] || fail "log-only mode wrote to the pipe"

# 5. No master: nothing is sent and nothing is started.
reset; mkfifo "$T/steam/steam.pipe"
gamepadui_seen "$LINE"
grep -q "with no master Steam" "$T/log" || fail "missing master not reported"

# 6. The watch loop: old lines are history, UIMode -1 is the desktop window, a rotated log is read from its start.
reset; reader; master 4309 /home/deck/.local/share/Steam/steamrt64/steam -steamos3 -silent
H="$T/logs/steamui_html.txt"
echo "[2026-11-19 19:48:41] CreateMainWindow: UIMode: 4" >"$H"
GPW_POLL=0.1 GPW_GAP=0 gamepadui_watch & WATCH=$!
sleep 0.4
echo "[2026-11-20 19:16:18] CreateMainWindow: UIMode: -1" >>"$H"; sleep 0.4
grep -q "GAMEPAD UI REQUESTED" "$T/log" && fail "history or the desktop window reported"
echo "$LINE" >>"$H"; sleep 0.5
[ "$(grep -c 'GAMEPAD UI REQUESTED' "$T/log")" = 1 ] || fail "new request not seen exactly once"
echo "[2026-11-20 20:00:01] CreateMainWindow: UIMode: 4" >"$H"; sleep 0.5   # Steam restarted: smaller file
[ "$(grep -c 'GAMEPAD UI REQUESTED' "$T/log")" = 2 ] || fail "request after log rotation missed"
kill "$WATCH" 2>/dev/null; WATCH=""

# 7. The real script: the watch starts with the headless Steam and is gone once the owner chooses stock.
mkdir -p "$T/s" "$T/conf/deckshell" "$T/state" "$T/fakeproc" "$T/bin" "$T/home"
cp "$SRC" "$T/s/deckshell-session.sh"
cat >"$T/bin/steam" <<'F'
#!/usr/bin/env bash
echo "steam $*" >>"$T_LOG"
F
cat >"$T/bin/stock" <<'F'
#!/usr/bin/env bash
echo "stock" >>"$T_LOG"; sleep 1.5
F
printf '#!/usr/bin/env bash\n' >"$T/bin/deckshell-run"
printf '#!/usr/bin/env bash\n' >"$T/bin/xprop"
cat >"$T/bin/systemctl" <<'F'
#!/usr/bin/env bash
case "$*" in
    *"show deckshell-ui.service -p MainPID --value"*) cat "$T_DIR/ui.pid" 2>/dev/null || echo 0 ;;
    *is-active*) exit 1 ;;
esac
exit 0
F
cat >"$T/bin/systemd-run" <<'F'
#!/usr/bin/env bash
ready=""; for a in "$@"; do case "$a" in --setenv=DECKSHELL_READY_FILE=*) ready="${a#*=}"; ready="${ready#*=}" ;; esac; done
echo $$ >"$T_DIR/ui.pid"; echo "$$ 1 0" >"$ready"
echo "ui exit 42" >>"$T_LOG"; sleep 0.3; rm -f "$T_DIR/ui.pid"; exit 42
F
chmod +x "$T/bin/"*
: >"$T/log"; echo hybrid >"$T/conf/deckshell/mode"
T_LOG="$T/log" T_DIR="$T" PATH="$T/bin:$PATH" DISPLAY=:99 XDG_RUNTIME_DIR="$T" HOME="$T/home" \
    XDG_CONFIG_HOME="$T/conf" XDG_STATE_HOME="$T/state" DECKSHELL_SYS_DIR="$T/none" DECKSHELL_RUN="$T/bin/deckshell-run" \
    DECKSHELL_STOCK_BIN="$T/bin/stock" STEAM_BIN="$T/bin/steam" DECKSHELL_PROC="$T/fakeproc" DECKSHELL_STEAM_DIR="$T/steam2" \
    DECKSHELL_STEAM_LOGS="$T/logs" MIN_LIFE=0 timeout 60 bash "$T/s/deckshell-session.sh" &
RUN=$!
sleep 1   # stock is running (exec'd launcher sleeps)
S="$T/state/deckshell/session.log"
grep -q "gamepad-ui watch pid=" "$S" || { sed 's/^/  session: /' "$S" >&2; fail "watch not started with the headless Steam"; }
grep -q "STOCK: owner chose" "$S" || fail "owner's stock choice not taken"
# Only the exec'd stock launcher may be left of the session script: no watch loop next to the stock UI.
left="$(pgrep -f "$T/s/deckshell-session.sh" | grep -vx "$RUN" || true)"
[ -z "$left" ] || fail "watch loop still running next to the stock UI: $left"
wait "$RUN" 2>/dev/null
echo "steam-ui-never: ok"
