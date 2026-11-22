#!/usr/bin/env bash
# Runs deck-session-restart.sh against fake Deck commands in a temp dir (no Deck, no session switch).
# Covers: argument rules, each pre-check refusal, the happy path, and failure during logind teardown and refusal to retry.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; SCRIPT="$HERE/../deck-session-restart.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"; [ -n "${OLD_PID:-}" ] && kill "$OLD_PID" 2>/dev/null || true' EXIT
mkdir -p "$T/bin" "$T/rt" "$T/state/deckshell"
FAKE_NOW_MS="$(date +%s%3N)"

# --- fake commands -----------------------------------------------------------------------------
cat >"$T/bin/journalctl" <<'F'
#!/usr/bin/env bash
[ ! -e "$T/journal_unavailable" ] || exit 1
case "$*" in
  *--since*) n=$(cat "$T/pd_since" 2>/dev/null || echo 0) ;;
  *"Could not take device"*) n=0 ;;
  *) n=$(cat "$T/pd" 2>/dev/null || echo 0) ;;
esac
for _ in $(seq "$n"); do echo "gamescope-wl: [Error] liftoff: drmModeAtomicCommit: Permission denied"; done
F
cat >"$T/bin/pgrep" <<'F'
#!/usr/bin/env bash
case "$*" in
  *"^gamescope "*) [ -e "$T/gs_cmdline_odd" ] || cat "$T/gs_pids" 2>/dev/null ;;
  *"-x gamescope-wl"*) cat "$T/gs_pids" 2>/dev/null ;;
  *"^sshd-session: deck@"*) for _ in $(seq "$(cat "$T/ssh_sessions" 2>/dev/null || echo 1)"); do echo 7575; done ;;
  *wineserver*|*vulkandriverquery*) for _ in $(seq "$(cat "$T/helpers" 2>/dev/null || echo 0)"); do echo 8888; done ;;
  *reaper*) for _ in $(seq "$(cat "$T/reapers" 2>/dev/null || echo 0)"); do echo 9999; done ;;
esac
F
cat >"$T/bin/ps" <<'F'
#!/usr/bin/env bash
echo 600
F
cat >"$T/bin/who" <<'F'
#!/usr/bin/env bash
echo "deck tty1 Nov 15 20:37"
F
cat >"$T/bin/systemctl" <<'F'
#!/usr/bin/env bash
# like the real one: no user bus without XDG_RUNTIME_DIR / DBUS_SESSION_BUS_ADDRESS in the environment
[ -n "${XDG_RUNTIME_DIR:-}" ] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ] || { echo "Failed to connect to user scope bus" >&2; exit 1; }
case "$*" in
  *is-active*) [ -e "$T/ui_active" ] ;;
  *"show -p MainPID"*) cat "$T/ui_pid" ;;
  *list-units*) : ;;
esac
F
cat >"$T/bin/xprop" <<'F'
#!/usr/bin/env bash
case "$*" in
  *GAMESCOPE_FOCUSED_APP*) echo "GAMESCOPE_FOCUSED_APP(CARDINAL) = $(cat "$T/focus")" ;;
  *REQUEST_SCREENSHOT*) head -c 30000 /dev/urandom >"$SHOT_FILE" ;;
  *STEAM_GAME*) v=$(cat "$T/game_app" 2>/dev/null); [ -n "$v" ] && echo "STEAM_GAME(CARDINAL) = $v" ;;
esac
F
cat >"$T/bin/xwininfo" <<'F'
#!/usr/bin/env bash
echo '  0x111 "DeckShell": ("deckshell" "DeckShell")'
echo '  0x222 "Steam": ("steam" "Steam")'
F
# steamos-session-select: records the call. mode per call from $T/plan (one word per line, last word repeats).
cat >"$T/bin/steamos-session-select" <<'F'
#!/usr/bin/env bash
echo "steamos-session-select $*" >>"$T/calls"
n=$(( $(wc -l <"$T/calls") ))
mode=$(sed -n "${n}p" "$T/plan"); [ -n "$mode" ] || mode=$(tail -n1 "$T/plan")
if [ "$mode" = ok ]; then
  echo $(( 4200 + n )) >"$T/gs_pids"; echo 0 >"$T/pd_since"
  if [ -f "$T/conf/deckshell/mode" ] && [ "$(cat "$T/conf/deckshell/mode")" = stock ]; then
    rm -f "$T/ui_active" "$T/rt/deckshell-ready"; echo "" >"$T/ui_pid"; echo 769 >"$T/focus"
  else
    ui_pid=$(( 5200 + n ))
    echo "$ui_pid" >"$T/ui_pid"; echo "$ui_pid 8388611 $FAKE_NOW_MS" >"$T/rt/deckshell-ready"
    touch "$T/ui_active"; echo 233594385 >"$T/focus"
  fi
else
  echo "$OLD_PID" >"$T/gs_pids"; echo 1 >"$T/pd_since"   # stuck: the old gamescope stays and keeps failing
fi
F
chmod +x "$T/bin/"*

fail() { echo "FAIL: $1" >&2; [ -f "$T/out" ] && sed 's/^/  | /' "$T/out" >&2; exit 1; }
calls() { [ -f "$T/calls" ] && wc -l <"$T/calls" || echo 0; }

reset() {  # healthy DeckShell at home, one gamescope, no switch yet
    : >"$T/calls"; echo 1 >"$T/ssh_sessions"
    printf '%s\n' 1111 >"$T/gs_pids"; echo 5151 >"$T/ui_pid"; touch "$T/ui_active"
    echo "5151 8388611 $FAKE_NOW_MS" >"$T/rt/deckshell-ready"
    echo 0 >"$T/pd"; echo 0 >"$T/pd_since"; echo 233594385 >"$T/focus"
    rm -f "$T/rt/deck-session-restart-incomplete" "$T/journal_unavailable" "$T/state/deckshell/session-restart.last" "$T/rt/deckshell-power-claimed" "$T/game_app" "$T/helpers" "$T/reapers" "$T/conf/deckshell/disable"
    echo '{"event":"record_input.hid","ts_ms":1}' >"$T/state/deckshell/session.log"   # physical press long ago
    unset INPUT_QUIET
    printf '%s\n' 'systemctl --user --job-mode=fail start steamos-powerbuttond.service' >"$T/power-recover.sh"
    mkdir -p "$T/local/bin" "$T/conf/deckshell"
    touch "$T/local/bin/deckshell"; chmod +x "$T/local/bin/deckshell"
    echo hybrid >"$T/conf/deckshell/mode"
}
reset_stock() {  # healthy stock Steam session, one gamescope, no switch yet
    : >"$T/calls"; echo 1 >"$T/ssh_sessions"
    printf '%s\n' 1111 >"$T/gs_pids"; echo "" >"$T/ui_pid"; rm -f "$T/ui_active" "$T/rt/deckshell-ready"
    echo 0 >"$T/pd"; echo 0 >"$T/pd_since"; echo 769 >"$T/focus"
    rm -f "$T/rt/deck-session-restart-incomplete" "$T/journal_unavailable" "$T/state/deckshell/session-restart.last" "$T/rt/deckshell-power-claimed" "$T/game_app" "$T/helpers" "$T/reapers" "$T/conf/deckshell/disable"
    echo '{"event":"record_input.hid","ts_ms":1}' >"$T/state/deckshell/session.log"   # physical press long ago
    unset INPUT_QUIET
    printf '%s\n' 'systemctl --user --job-mode=fail start steamos-powerbuttond.service' >"$T/power-recover.sh"
    mkdir -p "$T/local/bin" "$T/conf/deckshell"
    touch "$T/local/bin/deckshell"; chmod +x "$T/local/bin/deckshell"
    echo stock >"$T/conf/deckshell/mode"
}
run() {  # rest = arguments of deck-session-restart.sh; sets RC. The plan for the fake switch is set by setplan.
    RC=0
    env -i PATH="$T/bin:/usr/bin:/bin" HOME="$T" XDG_RUNTIME_DIR="$T/rt" XDG_STATE_HOME="$T/state" XDG_CONFIG_HOME="$T/conf" \
        DECKSHELL_BIN="$T/local/bin/deckshell" \
        DISPLAY=:0 SHOT_FILE="$T/shot.png" T="$T" FAKE_NOW_MS="$FAKE_NOW_MS" OLD_PID="${OLD_PID:-}" \
        POWER_HOOK="$T/power-recover.sh" MIN_GAP=0 UI_MIN_AGE=0 INPUT_QUIET="${INPUT_QUIET:-0}" POST_WAIT=0 SESSION_WAIT=4 OLD_WAIT=4 KILL_GRACE=0 \
        bash "$SCRIPT" "$@" >"$T/out" 2>&1 || RC=$?
}
setplan() { printf '%s\n' "$@" >"$T/plan"; }

# 1. argument rules: no default action, the session name is required
reset; setplan ok
for args in "" "restart" "restart steam" "restart gamescope extra" "switch gamescope" "plasma"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" = 2 ] || fail "args '$args': exit $RC, want 2"
    [ "$(calls)" = 0 ] || fail "args '$args' still switched the session"
done
echo "ok: argument rules (exit 2, no switch)"

# 2. check: healthy state is READY and changes nothing
reset; setplan ok; run check
[ "$RC" = 0 ] || fail "check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "check switched the session"
grep -q 'RESULT: READY' "$T/out" || fail "check did not report READY"
echo "ok: check READY, no switch"

# 3. refusals: each must exit 3 and never call the switch
refuse() {  # $1 = label, rest = setup commands (eval)
    local label="$1"; shift; reset; setplan ok; eval "$*"
    run restart gamescope
    [ "$RC" = 3 ] || fail "$label: exit $RC, want 3"
    [ "$(calls)" = 0 ] || fail "$label: switched the session anyway"
    echo "ok: refused ($label)"
}
refuse "interrupted previous switch" 'touch "$T/rt/deck-session-restart-incomplete"'
refuse "unreadable journal" 'touch "$T/journal_unavailable"'
refuse "Permission denied this boot" 'echo 3 >"$T/pd"'
refuse "game on top" 'echo 2212 >"$T/focus"'
refuse "game window under the QAM (focus still on DeckShell)" 'echo 2379780 >"$T/game_app"'
refuse "physical press 60 s ago" 'echo "{\"event\":\"record_input.hid\",\"ts_ms\":$(( FAKE_NOW_MS - 60000 ))}" >"$T/state/deckshell/session.log"; INPUT_QUIET=600'
refuse "two gamescope processes" 'printf "%s\n" 1111 2222 >"$T/gs_pids"'
refuse "old power hook (cancels gamescope's stop job)" 'echo "systemctl --user start steamos-powerbuttond.service" >"$T/power-recover.sh"'
refuse "stale power marker" 'rm -f "$T/ui_active"; touch "$T/rt/deckshell-power-claimed"'
refuse "recording active" 'echo "{\"event\":\"native_recording.observation\",\"data\":{\"active_session_observed\":true}}" >"$T/state/deckshell/session.log"'
refuse "another ssh session" 'echo 2 >"$T/ssh_sessions"' 


# 4. the last switch was too recent
reset; setplan ok; echo "$(date +%s)" >"$T/state/deckshell/session-restart.last"
RC=0; env -i PATH="$T/bin:/usr/bin:/bin" HOME="$T" XDG_RUNTIME_DIR="$T/rt" XDG_STATE_HOME="$T/state" \
    DISPLAY=:0 SHOT_FILE="$T/shot.png" FAKE_NOW_MS="$FAKE_NOW_MS" MIN_GAP=120 UI_MIN_AGE=0 INPUT_QUIET=0 POST_WAIT=0 \
    bash "$SCRIPT" restart gamescope >"$T/out" 2>&1 || RC=$?
[ "$RC" = 3 ] || fail "switch 0 s after the last one: exit $RC, want 3"; [ "$(calls)" = 0 ] || fail "switched too soon"
echo "ok: refused (last switch too recent)"

# 5. happy path: one explicit switch, post-checks pass
reset; setplan ok; OLD_PID=""
run restart gamescope
[ "$RC" = 0 ] || fail "happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "happy path call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "happy path: no OK line"
echo "ok: happy path (1 explicit switch, OK)"

# 6. Incident ordering: old gamescope remains while logind/DRM errors appear.
# A tempting second switch would succeed in the fake, but must never be attempted.
sleep 300 & OLD_PID=$!
reset; setplan stuck ok
echo "$OLD_PID" >"$T/gs_pids"
run restart gamescope
[ "$RC" = 4 ] || fail "DRM failure: exit $RC, want 4"
[ "$(calls)" = 1 ] || fail "DRM failure: expected exactly one switch"
kill -0 "$OLD_PID" 2>/dev/null || fail "wrapper killed the old gamescope"
grep -q "Permission denied' since the switch" "$T/out" || fail "DRM failure was hidden"
grep -q 'No automatic recovery or second switch' "$T/out" || fail "no failure guidance"
# Even if journal and process probes later look healthy, the incomplete attempt blocks a retry.
echo 0 >"$T/pd_since"; echo 1111 >"$T/gs_pids"
run restart gamescope
[ "$RC" = 3 ] || fail "incomplete attempt: exit $RC, want 3"
[ "$(calls)" = 1 ] || fail "incomplete attempt permitted another switch"
grep -q 'previous switch failed or was interrupted' "$T/out" || fail "missing incomplete reason"
kill "$OLD_PID" 2>/dev/null || true; OLD_PID=""
echo "ok: DRM failure stops after one switch and blocks retry"

# 7. stock argument rules (exit 2, no switch)
reset_stock; setplan ok
for args in "from-stock" "from-stock restart" "from-stock restart steam" "from-stock restart gamescope extra" "check from-stock extra" "enter-hybrid" "enter-hybrid restart" "enter-hybrid gamescope extra"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" = 2 ] || fail "stock args '$args': exit $RC, want 2"
    [ "$(calls)" = 0 ] || fail "stock args '$args' switched session"
done
echo "ok: stock argument rules (exit 2, no switch)"

# 8. stock check on healthy state
reset_stock; setplan ok; run check from-stock
[ "$RC" = 0 ] || fail "check from-stock on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "check from-stock switched session"
grep -q 'RESULT: READY' "$T/out" || fail "check from-stock did not report READY"
reset_stock; setplan ok; run from-stock check
[ "$RC" = 0 ] || fail "from-stock check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "from-stock check switched session"
grep -q 'RESULT: READY' "$T/out" || fail "from-stock check did not report READY"
reset_stock; setplan ok; run enter-hybrid check
[ "$RC" = 0 ] || fail "enter-hybrid check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "enter-hybrid check switched session"
grep -q 'RESULT: READY' "$T/out" || fail "enter-hybrid check did not report READY"
echo "ok: check from-stock READY, no switch"

# 9. stock refusals: each must exit 3 and never call the switch
refuse_stock() {
    local label="$1"; shift; reset_stock; setplan ok; eval "$*"
    run from-stock restart gamescope
    [ "$RC" = 3 ] || fail "$label: exit $RC, want 3"
    [ "$(calls)" = 0 ] || fail "$label: switched the session anyway"
    echo "ok: stock refused ($label)"
}
refuse_stock "deckshell-ui already active" 'touch "$T/ui_active"'
refuse_stock "game focused in stock" 'echo 2379780 >"$T/focus"'
refuse_stock "game window under stock Steam" 'echo 2379780 >"$T/game_app"'
refuse_stock "Wine/Proton helper running" 'echo 1 >"$T/helpers"'
refuse_stock "reaper process running" 'echo 1 >"$T/reapers"'
refuse_stock "DeckShell binary missing" 'rm -f "$T/local/bin/deckshell"'
refuse_stock "DeckShell disabled marker present" 'touch "$T/conf/deckshell/disable"'
refuse_stock "stale ready marker" 'echo "5151 1 1" >"$T/rt/deckshell-ready"'
refuse_stock "stale power marker" 'touch "$T/rt/deckshell-power-claimed"'
refuse_stock "Permission denied this boot in stock" 'echo 3 >"$T/pd"'
refuse_stock "unreadable journal in stock" 'touch "$T/journal_unavailable"'
refuse_stock "interrupted previous switch in stock" 'touch "$T/rt/deck-session-restart-incomplete"'
refuse_stock "physical press 60 s ago in stock" 'echo "{\"event\":\"record_input.hid\",\"ts_ms\":$(( FAKE_NOW_MS - 60000 ))}" >"$T/state/deckshell/session.log"; INPUT_QUIET=600'
refuse_stock "another ssh session in stock" 'echo 2 >"$T/ssh_sessions"'

# 10. stock happy path: one explicit switch, mode written as hybrid, post-checks pass
reset_stock; setplan ok; OLD_PID=""
run from-stock restart gamescope
[ "$RC" = 0 ] || fail "stock happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "stock happy path call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "stock happy path: no OK line"
[ "$(cat "$T/conf/deckshell/mode")" = "hybrid" ] || fail "stock happy path did not set mode=hybrid"
echo "ok: stock happy path (from-stock restart gamescope, 1 switch, OK, mode=hybrid)"

reset_stock; setplan ok; OLD_PID=""
run enter-hybrid gamescope
[ "$RC" = 0 ] || fail "enter-hybrid happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "enter-hybrid call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "enter-hybrid: no OK line"
[ "$(cat "$T/conf/deckshell/mode")" = "hybrid" ] || fail "enter-hybrid did not set mode=hybrid"
echo "ok: stock happy path (enter-hybrid gamescope, 1 switch, OK, mode=hybrid)"

# 11. stock switch failure: DRM error stops after one switch and blocks retry
sleep 300 & OLD_PID=$!
reset_stock; setplan stuck ok
echo "$OLD_PID" >"$T/gs_pids"
run from-stock restart gamescope
[ "$RC" = 4 ] || fail "stock DRM failure: exit $RC, want 4"
[ "$(calls)" = 1 ] || fail "stock DRM failure: expected exactly one switch"
kill -0 "$OLD_PID" 2>/dev/null || fail "wrapper killed the old gamescope in stock"
grep -q "Permission denied' since the switch" "$T/out" || fail "stock DRM failure was hidden"
# Incomplete attempt blocks retry in stock
echo 0 >"$T/pd_since"; echo 1111 >"$T/gs_pids"
run from-stock restart gamescope
[ "$RC" = 3 ] || fail "stock incomplete attempt: exit $RC, want 3"
[ "$(calls)" = 1 ] || fail "stock incomplete attempt permitted another switch"
grep -q 'previous switch failed or was interrupted' "$T/out" || fail "stock missing incomplete reason"
kill "$OLD_PID" 2>/dev/null || true; OLD_PID=""
echo "ok: stock DRM failure stops after one switch and blocks retry"

# 12. to-stock argument rules (exit 2, no switch)
reset; setplan ok
for args in "to-stock" "to-stock restart" "to-stock restart steam" "to-stock restart gamescope extra" "check to-stock extra" "enter-stock" "enter-stock restart" "enter-stock gamescope extra" "leave-hybrid" "leave-hybrid restart" "leave-hybrid gamescope extra"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" = 2 ] || fail "to-stock args '$args': exit $RC, want 2"
    [ "$(calls)" = 0 ] || fail "to-stock args '$args' switched session"
done
echo "ok: to-stock argument rules (exit 2, no switch)"

# 13. to-stock check on healthy state
reset; setplan ok; run check to-stock
[ "$RC" = 0 ] || fail "check to-stock on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "check to-stock switched session"
grep -q 'RESULT: READY' "$T/out" || fail "check to-stock did not report READY"
reset; setplan ok; run to-stock check
[ "$RC" = 0 ] || fail "to-stock check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "to-stock check switched session"
grep -q 'RESULT: READY' "$T/out" || fail "to-stock check did not report READY"
reset; setplan ok; run enter-stock check
[ "$RC" = 0 ] || fail "enter-stock check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "enter-stock check switched session"
grep -q 'RESULT: READY' "$T/out" || fail "enter-stock check did not report READY"
reset; setplan ok; run leave-hybrid check
[ "$RC" = 0 ] || fail "leave-hybrid check on healthy state: exit $RC"; [ "$(calls)" = 0 ] || fail "leave-hybrid check switched session"
grep -q 'RESULT: READY' "$T/out" || fail "leave-hybrid check did not report READY"
echo "ok: check to-stock READY, no switch"

# 14. to-stock refusals: each must exit 3 and never call the switch
refuse_to_stock() {
    local label="$1"; shift; reset; setplan ok; eval "$*"
    run to-stock restart gamescope
    [ "$RC" = 3 ] || fail "$label: exit $RC, want 3"
    [ "$(calls)" = 0 ] || fail "$label: switched the session anyway"
    echo "ok: to-stock refused ($label)"
}
refuse_to_stock "already in stock session" 'echo stock >"$T/conf/deckshell/mode"; rm -f "$T/ui_active"'
refuse_to_stock "game focused in to-stock" 'echo 2379780 >"$T/focus"'
refuse_to_stock "game window under DeckShell" 'echo 2379780 >"$T/game_app"'
refuse_to_stock "DeckShell binary missing (cannot verify return)" 'rm -f "$T/local/bin/deckshell"'
refuse_to_stock "Wine/Proton helper running" 'echo 1 >"$T/helpers"'
refuse_to_stock "reaper process running" 'echo 1 >"$T/reapers"'
refuse_to_stock "physical press 60 s ago in to-stock" 'echo "{\"event\":\"record_input.hid\",\"ts_ms\":$(( FAKE_NOW_MS - 60000 ))}" >"$T/state/deckshell/session.log"; INPUT_QUIET=600'
refuse_to_stock "Permission denied this boot in to-stock" 'echo 3 >"$T/pd"'
refuse_to_stock "unreadable journal in to-stock" 'touch "$T/journal_unavailable"'
refuse_to_stock "interrupted previous switch in to-stock" 'touch "$T/rt/deck-session-restart-incomplete"'
refuse_to_stock "another ssh session in to-stock" 'echo 2 >"$T/ssh_sessions"'
refuse_to_stock "two gamescope processes in to-stock" 'printf "%s\n" 1111 2222 >"$T/gs_pids"'

# 15. to-stock happy path: one explicit switch, mode written as stock, backup written, post-checks pass
reset; setplan ok; OLD_PID=""
run to-stock restart gamescope
[ "$RC" = 0 ] || fail "to-stock happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "to-stock happy path call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "to-stock happy path: no OK line"
[ "$(cat "$T/conf/deckshell/mode")" = "stock" ] || fail "to-stock happy path did not set mode=stock"
[ -f "$T/conf/deckshell/mode.bak" ] || fail "to-stock happy path did not create mode.bak"
[ "$(cat "$T/conf/deckshell/mode.bak")" = "hybrid" ] || fail "to-stock happy path backup is not hybrid: $(cat "$T/conf/deckshell/mode.bak" 2>/dev/null)"
echo "ok: to-stock happy path (to-stock restart gamescope, 1 switch, OK, mode=stock, backup=hybrid)"

reset; setplan ok; OLD_PID=""
run enter-stock gamescope
[ "$RC" = 0 ] || fail "enter-stock happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "enter-stock call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "enter-stock: no OK line"
[ "$(cat "$T/conf/deckshell/mode")" = "stock" ] || fail "enter-stock did not set mode=stock"
echo "ok: to-stock happy path (enter-stock gamescope, 1 switch, OK, mode=stock)"

reset; setplan ok; OLD_PID=""
run leave-hybrid gamescope
[ "$RC" = 0 ] || fail "leave-hybrid happy path: exit $RC"
[ "$(cat "$T/calls")" = "steamos-session-select gamescope" ] || fail "leave-hybrid call: $(cat "$T/calls")"
grep -q 'RESULT: OK (checks passed' "$T/out" || fail "leave-hybrid: no OK line"
[ "$(cat "$T/conf/deckshell/mode")" = "stock" ] || fail "leave-hybrid did not set mode=stock"
echo "ok: to-stock happy path (leave-hybrid gamescope, 1 switch, OK, mode=stock)"

# 16. verifiable return roundtrip: hybrid -> to-stock -> stock -> from-stock -> hybrid
reset; setplan ok ok; OLD_PID=""
run to-stock restart gamescope
[ "$RC" = 0 ] || fail "roundtrip step 1 (to-stock): exit $RC"
[ "$(cat "$T/conf/deckshell/mode")" = "stock" ] || fail "roundtrip step 1 did not set mode=stock"

run check from-stock
[ "$RC" = 0 ] || fail "roundtrip step 2 (check from-stock): exit $RC"
grep -q 'RESULT: READY' "$T/out" || fail "roundtrip step 2 check from-stock did not report READY"

run from-stock restart gamescope
[ "$RC" = 0 ] || fail "roundtrip step 3 (from-stock restart): exit $RC"
[ "$(cat "$T/conf/deckshell/mode")" = "hybrid" ] || fail "roundtrip step 3 did not set mode=hybrid"

run check
[ "$RC" = 0 ] || fail "roundtrip step 4 (check hybrid): exit $RC"
grep -q 'RESULT: READY' "$T/out" || fail "roundtrip step 4 check hybrid did not report READY"
echo "ok: verifiable return roundtrip (hybrid -> to-stock -> stock -> from-stock -> hybrid)"

# 17. to-stock switch failure: DRM error stops after one switch and blocks retry
sleep 300 & OLD_PID=$!
reset; setplan stuck ok
echo "$OLD_PID" >"$T/gs_pids"
run to-stock restart gamescope
[ "$RC" = 4 ] || fail "to-stock DRM failure: exit $RC, want 4"
[ "$(calls)" = 1 ] || fail "to-stock DRM failure: expected exactly one switch"
kill -0 "$OLD_PID" 2>/dev/null || fail "wrapper killed the old gamescope in to-stock"
grep -q "Permission denied' since the switch" "$T/out" || fail "to-stock DRM failure was hidden"
# Incomplete attempt blocks retry in to-stock
echo 0 >"$T/pd_since"; echo 1111 >"$T/gs_pids"
run to-stock restart gamescope
[ "$RC" = 3 ] || fail "to-stock incomplete attempt: exit $RC, want 3"
[ "$(calls)" = 1 ] || fail "to-stock incomplete attempt permitted another switch"
grep -q 'previous switch failed or was interrupted' "$T/out" || fail "to-stock missing incomplete reason"
kill "$OLD_PID" 2>/dev/null || true; OLD_PID=""
echo "ok: to-stock DRM failure stops after one switch and blocks retry"

# ssh without XDG_RUNTIME_DIR / DBUS_SESSION_BUS_ADDRESS (run() passes neither DBUS): a live UI must still count as active,
# and gamescope is found by comm when its cmdline does not start with "gamescope ".
reset; touch "$T/gs_cmdline_odd"
run check
[ "$RC" = 0 ] || fail "check in a bare ssh environment: exit $RC, want 0"
grep -q 'deckshell-ui active' "$T/out" || fail "live UI reported not active without DBUS_SESSION_BUS_ADDRESS"
grep -q 'one gamescope process: 1111' "$T/out" || fail "gamescope not found by comm"
rm -f "$T/gs_cmdline_odd"
echo "ok: check works in a bare ssh environment"

echo "ALL PASSED: deck-session-restart tests"
