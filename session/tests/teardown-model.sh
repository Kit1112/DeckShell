#!/usr/bin/env bash
# Session teardown model (task 363): does stopping the Gaming session really stop gamescope when the
# DeckShell UI's ExecStopPost hook (session/power-recover.sh) runs in the middle of it?
#
# Runs on the PC against the real `systemd --user` manager with throw-away units (prefix ds363-) that
# copy the dependency lines of the Deck's units (read 2026-11-16, SteamOS, systemd 261):
#   gamescope-session.target/.service, graphical-session(-pre).target, steam-launcher.service,
#   steamos-powerbuttond.service (Requisite=gamescope-session.service), deckshell-ui.service (systemd-run).
# The processes are `sleep`; nothing graphical is touched. Units live in $XDG_RUNTIME_DIR/systemd/user
# and are removed at exit. Skips (exit 0) when no user manager is reachable.
#
#   teardown-model.sh            control (old hook must reproduce the defect) + the real hook must pass
#   teardown-model.sh --explain  also print the job/journal detail of each run
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$HERE/../power-recover.sh"
EXPLAIN=0; [ "${1:-}" = --explain ] && EXPLAIN=1
P=ds363
sc() { systemctl --user "$@"; }
if ! sc show-environment >/dev/null 2>&1 || [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    echo "skip: no systemd --user manager"; exit 0
fi
U="$XDG_RUNTIME_DIR/systemd/user"; T="$(mktemp -d)"
mkdir -p "$U"
UNITS=("$P-gpre.target" "$P-graph.target" "$P-gs.service" "$P-session.target" "$P-launcher.service" "$P-pb.service")
cleanup() {
    local u
    for u in "$P-ui.service" "$P-session.target" "$P-launcher.service" "$P-gs.service" "$P-pb.service" "$P-graph.target" "$P-gpre.target"; do
        sc stop "$u" >/dev/null 2>&1
    done
    sc reset-failed "$P-*" >/dev/null 2>&1
    for u in "${UNITS[@]}"; do rm -f "$U/$u"; done
    sc daemon-reload >/dev/null 2>&1
    rm -rf "$T"
}
trap cleanup EXIT

# --- the Deck's dependency lines, renamed -----------------------------------------------------------
cat >"$U/$P-gpre.target" <<F
[Unit]
Description=model graphical-session-pre.target
RefuseManualStart=yes
StopWhenUnneeded=yes
Before=$P-graph.target
F
cat >"$U/$P-graph.target" <<F
[Unit]
Description=model graphical-session.target
RefuseManualStart=yes
StopWhenUnneeded=yes
F
cat >"$U/$P-gs.service" <<F
[Unit]
Description=model gamescope-session.service
Before=$P-graph.target
PartOf=$P-graph.target
Wants=$P-gpre.target $P-pb.service
After=$P-gpre.target
RefuseManualStart=yes
[Service]
Type=exec
TimeoutStopSec=10
ExecStart=/usr/bin/sleep infinity
F
cat >"$U/$P-session.target" <<F
[Unit]
Description=model gamescope-session.target
Requires=$P-graph.target
BindsTo=$P-graph.target
After=$P-graph.target
PropagatesStopTo=$P-graph.target
Requires=$P-gs.service
BindsTo=$P-gs.service
Upholds=$P-launcher.service
F
# The launcher takes seconds to stop (Steam shutdown: 3-12 s on the Deck); gamescope's stop job waits behind it.
cat >"$U/$P-launcher.service" <<F
[Unit]
Description=model steam-launcher.service
After=$P-graph.target
PartOf=$P-graph.target
[Service]
Type=exec
ExecStart=/usr/bin/sleep infinity
ExecStop=/usr/bin/sleep 2
TimeoutStopSec=20
F
cat >"$U/$P-pb.service" <<F
[Unit]
Description=model steamos-powerbuttond.service
Requisite=$P-gs.service
[Service]
ExecStart=/usr/bin/sleep infinity
F
# The defect as installed on the Deck until this task (session/power-recover.sh at 57cb9f1).
cat >"$T/old-hook.sh" <<'F'
#!/usr/bin/env bash
set -eu
marker="$DECKSHELL_POWER_MARKER"
[ -e "$marker" ] || exit 0
if systemctl --user is-active --quiet "$DECKSHELL_GS_SERVICE"; then
    timeout 5s systemctl --user start "$DECKSHELL_STOCK_POWER"
fi
rm -f -- "$marker"
F
# Only the last guard of the real hook: no state checks, just --job-mode=fail.
sed 's/systemctl --user start/systemctl --user --job-mode=fail start/; s/^set -eu$/set -u/' "$T/old-hook.sh" >"$T/jobmode-hook.sh"
chmod +x "$T/old-hook.sh" "$T/jobmode-hook.sh"
sc daemon-reload

FAILED=0
fail() { echo "FAIL: $*" >&2; FAILED=1; }
pid_of() { sc show -p MainPID --value "$1"; }
state_of() { sc show -p ActiveState --value "$1"; }

# Brings the model session up as the Deck does: target, then the UI unit, then the UI claims Power
# (stock daemon stopped, marker written). $1 = hook path or "none".
session_up() {
    local hook="$1" props=()
    sc reset-failed "$P-*" >/dev/null 2>&1
    sc start "$P-session.target" || { fail "model session did not start"; return 1; }
    [ "$hook" = none ] || props=(--property="ExecStopPost=$hook")
    systemd-run --user --quiet --unit="$P-ui.service" --service-type=exec --collect \
        --property="StopPropagatedFrom=$P-gs.service $P-launcher.service" \
        --property="Requisite=$P-gs.service" --property="After=$P-gs.service" --property="TimeoutStopSec=5s" \
        "${props[@]}" \
        --setenv="DECKSHELL_POWER_MARKER=$T/claimed" --setenv="DECKSHELL_GS_SERVICE=$P-gs.service" \
        --setenv="DECKSHELL_GS_TARGET=$P-session.target" --setenv="DECKSHELL_STOCK_POWER=$P-pb.service" \
        /usr/bin/sleep infinity || { fail "model UI did not start"; return 1; }
    : >"$T/claimed"; sc stop "$P-pb.service"
}
# The session switch as /usr/bin/start-gamescope-session sees it: the target stops, the script then
# blocks on graphical-session-pre.target and returns ("Cleanup Complete"); sddm ends the logind session.
teardown() {
    sc stop "$P-session.target"
    sc stop "$P-gpre.target"
}
detail() {
    [ "$EXPLAIN" = 1 ] || return 0
    journalctl --user --no-pager -o short-precise --since "@$1" 2>/dev/null \
        | grep -E "model |$P-" | sed 's/^/    /'
}

# $1 label, $2 hook, $3 expected state of the gamescope model after teardown (active = defect)
scenario_teardown() {
    local label="$1" hook="$2" want="$3" t0 gs got
    t0=$(date +%s)
    session_up "$hook" || return
    gs=$(pid_of "$P-gs.service")
    teardown
    if kill -0 "$gs" 2>/dev/null; then got=active; else got=inactive; fi
    if [ "$got" = "$want" ]; then echo "ok: $label: gamescope model $got after teardown"
    else fail "$label: gamescope model is $got after teardown, expected $want"; fi
    detail "$t0"
    LAST_GS="$gs"
}

# 1. Control: the old hook must reproduce the Deck's defect, otherwise this model proves nothing.
scenario_teardown "old hook (control)" "$T/old-hook.sh" active
# 1b. What the next session (sddm relogin -> start-gamescope-session) does with that leftover.
if kill -0 "${LAST_GS:-0}" 2>/dev/null; then
    sc stop "$P-session.target"; sc stop "$P-gpre.target"; sc reset-failed
    sc start "$P-session.target"
    if [ "$(pid_of "$P-gs.service")" = "$LAST_GS" ]; then
        echo "ok: next session start reuses the leftover gamescope model (pid $LAST_GS), as on the Deck"
    else fail "next session start replaced the leftover gamescope model (the Deck did not)"; fi
fi
sc stop "$P-gs.service" "$P-session.target" >/dev/null 2>&1; rm -f "$T/claimed"

# 2. No hook at all (stock SteamOS, and the Deck's teardowns where the UI was not up yet).
scenario_teardown "no hook" none inactive

# 2b. --job-mode=fail alone is enough: the start is refused instead of cancelling the stop job.
scenario_teardown "old hook + --job-mode=fail only" "$T/jobmode-hook.sh" inactive

# 3. The real hook: the session must go down, and the marker must be consumed.
scenario_teardown "session/power-recover.sh" "$HOOK" inactive
[ ! -e "$T/claimed" ] || fail "real hook left the power marker behind after a session teardown"

# 4. The real hook when only the UI stops (crash, UI restart): the stock daemon must come back.
session_up "$HOOK" && {
    sc stop "$P-ui.service"
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ "$(state_of "$P-pb.service")" = active ] && break; sleep 0.2; done
    if [ "$(state_of "$P-pb.service")" = active ] && [ "$(state_of "$P-gs.service")" = active ] && [ ! -e "$T/claimed" ]; then
        echo "ok: UI-only stop: stock power daemon restored, session untouched"
    else fail "UI-only stop: stock daemon '$(state_of "$P-pb.service")', gamescope model '$(state_of "$P-gs.service")', marker $([ -e "$T/claimed" ] && echo present || echo gone)"; fi
}

[ "$FAILED" = 0 ] && echo "ok: teardown model ($(systemctl --version | head -n1))"
exit "$FAILED"
