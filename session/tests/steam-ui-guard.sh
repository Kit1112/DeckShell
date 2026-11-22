#!/usr/bin/env bash
# Stock-UI guard of session/deckshell-session.sh (docs/steam-ui-parallel-2026-11-20.md).
# Runs the real script against a fake /proc, a real FIFO for steam.pipe and fake steam/systemd-run/systemctl;
# touches nothing outside a temp dir and never runs the real Steam.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; SRC="$HERE/../deckshell-session.sh"
T="$(mktemp -d)"; READER=""
cleanup() { { [ -n "$READER" ] && kill "$READER" 2>/dev/null; } || true; rm -rf "$T"; }
trap cleanup EXIT
fail() { echo "FAIL: $1" >&2; [ -f "$T/log" ] && sed 's/^/  log: /' "$T/log" >&2; [ -f "$T/state/deckshell/session.log" ] && sed 's/^/  session: /' "$T/state/deckshell/session.log" >&2; exit 1; }

# ---------------------------------------------------------------- part 1: helpers, sourced from the script
helpers() {
    log() { echo "$*" >>"$T/log"; }
    STEAM_DIR="$T/steam"; STEAM_PIPE="$STEAM_DIR/steam.pipe"; PROC="$T/proc"; STEAM_LOGS="$T/none"
    STEAM_BIN="$T/bin/steam"; STEAM_SHELL_ARGS="-steamos3 -silent"; STEAM_PID=""; HOLD="$T/hold"; HOLD_MAX=240
    eval "$(sed -n '/^headless_args() {/,/^}/p;/^steam_master_pid() {/,/^}/p;/^proc_args() {/p;/^steam_is_headless() {/p;/^game_running() {/,/^}/p;/^sq() {/p;/^steam_pipe_send() {/,/^}/p;/^steam_provenance() {/,/^}/p;/^FOREIGN_PID=/p;/^foreign_steam() {/,/^}/p;/^STEAM_MASTER=/p;/^steam_tick() {/,/^}/p;/^hold_active() {/,/^}/p' "$SRC")"
    start_steam() { echo "start_steam $STEAM_SHELL_ARGS" >>"$T/log"; }
}
mkproc() {   # pid ppid comm args... ; fd/3 -> steam.pipe when comm=steam-master
    local pid=$1 ppid=$2 comm=$3; shift 3
    mkdir -p "$T/proc/$pid/fd"
    printf '%s\0' "$@" >"$T/proc/$pid/cmdline"
    printf 'Name:\t%s\nPPid:\t%s\n' "$comm" "$ppid" >"$T/proc/$pid/status"
    echo "$comm" >"$T/proc/$pid/comm"
    echo "0::/user.slice/app.slice/$comm.service" >"$T/proc/$pid/cgroup"
}
reset() {
    { [ -n "$READER" ] && kill "$READER" 2>/dev/null; } || true; READER=""
    rm -rf "$T/proc" "$T/steam" "$T/log" "$T/pipe.out"; mkdir -p "$T/proc" "$T/steam" "$T/bin"; : >"$T/log"
}
reader() { mkfifo "$T/steam/steam.pipe"; cat "$T/steam/steam.pipe" >"$T/pipe.out" & READER=$!; sleep 0.1; }
master() {   # pid args...
    local pid=$1; shift
    mkproc "$pid" 900 steam "$@"; ln -s "$T/steam/steam.pipe" "$T/proc/$pid/fd/3"; echo "$pid" >"$T/steam/steam.pid"
}
helpers
set +e   # the script itself runs without -e; checks below call fail explicitly

[ "$(headless_args '-steamos3 -steampal -gamepadui')" = "-steamos3 -silent" ] || fail "UI-mode flags not stripped"
[ "$(headless_args '-steamos3')" = "-steamos3 -silent" ] || fail "-silent not enforced"

# 1. No Steam reads the pipe: nothing is sent and nothing is started.
reset; mkfifo "$T/steam/steam.pipe"
if steam_pipe_send -shutdown; then fail "sent with no reader"; fi
grep -q "NOT sent" "$T/log" || fail "refusal not logged"
# stale pid file: the process exists but has no steam.pipe open (pid recycling, steam.sh's own test)
mkproc 500 1 bash /bin/bash; echo 500 >"$T/steam/steam.pid"
[ -z "$(steam_master_pid)" ] || fail "stale steam.pid accepted as master"

# 2. Headless master + our launcher already gone ("Steam is already running, exiting"): adopt, do not restart.
reset; reader; master 4309 /home/deck/.local/share/Steam/steamrt64/steam -steamdeck -steamos3 -silent
STEAM_PID=999999; steam_tick; steam_tick
grep -q "start_steam" "$T/log" && fail "headless master running, yet Steam was started again"
grep -q "steam master pid=4309 (headless)" "$T/log" || fail "headless master not adopted"

# 3. Foreign master (no -silent), no game: logged with provenance, asked to exit over the pipe, nothing started.
reset; reader
mkproc 900 1 deckshell /home/deck/.local/share/deckshell/bin/deckshell
master 47800 /home/deck/.local/share/Steam/steamrt64/steam 'steam://openurl/https://store.steampowered.com/account/'
STEAM_PID=999999; steam_tick
grep -q "FOREIGN STEAM UI: master Steam pid=47800" "$T/log" || fail "foreign master not reported"
grep -q "parent\[1\] pid=900 deckshell" "$T/log" || fail "parent chain missing from the report"
grep -q "cgroup: " "$T/log" || fail "cgroup missing from the report"
sleep 0.2
[ "$(cat "$T/pipe.out")" = "'~/.steam/root/ubuntu12_32/steam' '-shutdown'" ] || fail "exit request not delivered over the pipe: $(cat "$T/pipe.out")"
grep -q "start_steam" "$T/log" && fail "Steam started while a master runs"

# 4. Foreign master during a game: exit deferred (it would end the game), nothing written.
reset; reader
master 47800 /home/deck/.local/share/Steam/steamrt64/steam 'steam://openurl/https://store.steampowered.com/account/'
mkproc 50000 47800 reaper /home/deck/.local/share/Steam/steamrt64/reaper SteamLaunch AppId=367520 -- /game
FOREIGN_PID=""; steam_tick; steam_tick
grep -q "game 367520 is running; exit request deferred" "$T/log" || fail "game not protected"
[ "$(grep -c 'deferred' "$T/log")" = 1 ] || fail "deferral logged every tick"
sleep 0.2; [ ! -s "$T/pipe.out" ] || fail "exit request sent during a game"

# 5. No master, launcher dead, no hold: start (headless only); with a hold: do not start.
reset; STEAM_PID=999999; STEAM_LAST_START=-100; steam_tick
grep -q "start_steam -steamos3 -silent" "$T/log" || fail "dead Steam not restarted headless"
reset; : >"$T/hold"; STEAM_LAST_START=-100; steam_tick
grep -q "start_steam" "$T/log" && fail "Steam started during an account-switch hold"
rm -f "$T/hold"

# 6. The script never runs the Steam binary for anything but a -silent start.
grep -vE '^[[:space:]]*#' "$SRC" | sed 's/[[:space:]]#.*$//' | grep -nE '"\$STEAM_BIN"[^#]*-shutdown|steam\.sh |xdg-open' && fail "a launcher-type Steam call is back in the session script"

# ---------------------------------------------------------------- part 2: the real script, exit-code policy
mkdir -p "$T/s" "$T/conf/deckshell" "$T/state" "$T/fakeproc"
cp "$SRC" "$T/s/deckshell-session.sh"   # without held-button.py / session-log.py: no kill switch, plain log
cat >"$T/bin/steam" <<'F'
#!/usr/bin/env bash
echo "steam $*" >>"$T_LOG"
F
cat >"$T/bin/stock" <<'F'
#!/usr/bin/env bash
echo "stock" >>"$T_LOG"
F
cat >"$T/bin/deckshell-run" <<'F'
#!/usr/bin/env bash
F
cat >"$T/bin/xprop" <<'F'
#!/usr/bin/env bash
F
cat >"$T/bin/systemctl" <<'F'
#!/usr/bin/env bash
case "$*" in
    *"show deckshell-ui.service -p MainPID --value"*) cat "$T_DIR/ui.pid" 2>/dev/null || echo 0 ;;
    *is-active*) exit 1 ;;
esac
exit 0
F
# One UI run per call: presents (ready file), then exits with the next code from $T_DIR/codes.
cat >"$T/bin/systemd-run" <<'F'
#!/usr/bin/env bash
ready=""; for a in "$@"; do case "$a" in --setenv=DECKSHELL_READY_FILE=*) ready="${a#*=}"; ready="${ready#*=}" ;; esac; done
echo $$ >"$T_DIR/ui.pid"; echo "$$ 1 0" >"$ready"
code="$(head -n1 "$T_DIR/codes")"; sed -i 1d "$T_DIR/codes"
echo "ui exit $code" >>"$T_LOG"; sleep 0.3; rm -f "$T_DIR/ui.pid"; exit "$code"
F
chmod +x "$T/bin/"*
script() {   # codes...
    rm -f "$T/conf/deckshell/disable" "$T/state/deckshell/session.log"; : >"$T/log"
    printf '%s\n' "$@" >"$T/codes"; echo hybrid >"$T/conf/deckshell/mode"
    T_LOG="$T/log" T_DIR="$T" PATH="$T/bin:$PATH" DISPLAY=:99 XDG_RUNTIME_DIR="$T" HOME="$T/home" \
        XDG_CONFIG_HOME="$T/conf" XDG_STATE_HOME="$T/state" DECKSHELL_SYS_DIR="$T/none" DECKSHELL_RUN="$T/bin/deckshell-run" \
        DECKSHELL_STOCK_BIN="$T/bin/stock" STEAM_BIN="$T/bin/steam" DECKSHELL_PROC="$T/fakeproc" DECKSHELL_STEAM_DIR="$T/steam2" \
        MIN_LIFE=0 MAX_FAILS=2 FAIL_WINDOW=60 timeout 60 bash "$T/s/deckshell-session.sh" || true
}
S="$T/state/deckshell/session.log"
# A. Power reader lost (44) restarts DeckShell; only the owner's 42 brings stock.
script 44 42
grep -q "exit 44, Power reader lost" "$S" || fail "44 not handled as restart"
[ "$(grep -c '^ui exit' "$T/log")" = 2 ] || fail "DeckShell not restarted after 44"
grep -q "STOCK: owner chose stock Steam UI in DeckShell (exit 42)" "$S" || fail "owner choice not taken to stock"
[ "$(grep -c '^stock' "$T/log")" = 1 ] || fail "stock started more than once"
# B. A single crash restarts DeckShell instead of stock; two in the window still fall back (no game).
script 101 0 101
[ "$(grep -c '^ui exit' "$T/log")" -ge 2 ] || fail "single crash went straight to stock"
grep -q "auto-fallback: 2 failures" "$S" || fail "crash loop did not fall back"
[ -e "$T/conf/deckshell/disable" ] || fail "crash loop did not write disable"
# C. The same crash loop during a game: no stock, DeckShell retried.
mkdir -p "$T/fakeproc/50000"; printf '%s\0' /x/reaper SteamLaunch AppId=367520 -- /game >"$T/fakeproc/50000/cmdline"
script 101 101 42
grep -q "STOCK DEFERRED: 2 failures in 60s (rc=101) but game 367520 is running" "$S" || fail "crash loop during a game took stock"
grep -q "STOCK: owner chose" "$S" || fail "owner choice during a game ignored"
grep -q "auto-fallback" "$S" && fail "auto-fallback fired during a game"
rm -rf "$T/fakeproc/50000"
# D. Steam started only headless, never via -shutdown.
grep '^steam ' "$T/log" | grep -qv -- '-silent' && fail "Steam started without -silent: $(grep '^steam ' "$T/log")"
grep -q -- '^steam .*-shutdown' "$T/log" && fail "steam -shutdown executed (it starts a client when none runs)"
echo "ok: stock-UI guard (pipe only, foreign master reported/handled, no automatic stock during a game, 44 restarts)"
