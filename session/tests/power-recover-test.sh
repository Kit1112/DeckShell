#!/usr/bin/env bash
# power-recover.sh (ExecStopPost of the UI) against a fake systemctl: it may start the stock Power daemon
# only while the Gaming session stays up, and never with a job-replacing start.
# Why: docs/drm-root-cause-2026-11-15.md. The real-systemd proof is session/tests/teardown-model.sh.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; HOOK="$HERE/../power-recover.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/rt"
# State files: target_active, gs_active (present = active), gs_job (content = queued job id), start_fails.
cat >"$T/bin/systemctl" <<'F'
#!/usr/bin/env bash
echo "systemctl $*" >>"$T/log"
case "$*" in
    *"is-active --quiet gamescope-session.target"*)  [ -e "$T/target_active" ] ;;
    *"is-active --quiet gamescope-session.service"*) [ -e "$T/gs_active" ] ;;
    *"show -p Job --value gamescope-session.service"*) cat "$T/gs_job" 2>/dev/null || true ;;
    *" start "*) [ ! -e "$T/start_fails" ] ;;
esac
F
chmod +x "$T/bin/systemctl"
fail() { echo "FAIL: $1" >&2; sed 's/^/  | /' "$T/log" "$T/out" >&2 2>/dev/null; exit 1; }
MARK="$T/rt/deckshell-power-claimed"
run() {  # $1 = marker (yes|no); the other state is set by the caller
    : >"$T/log"; rm -f "$MARK"; [ "$1" = no ] || : >"$MARK"
    RC=0; env -i PATH="$T/bin:/usr/bin:/bin" T="$T" XDG_RUNTIME_DIR="$T/rt" bash "$HOOK" >"$T/out" 2>&1 || RC=$?
}
state() { rm -f "$T/target_active" "$T/gs_active" "$T/gs_job" "$T/start_fails"; local f; for f in "$@"; do : >"$T/$f"; done; }
started() { grep -q ' start ' "$T/log"; }

# 1. No marker: the UI never claimed Power. Nothing is asked of systemd.
state target_active gs_active; run no
[ "$RC" = 0 ] && [ ! -s "$T/log" ] || fail "no marker: systemctl was called"
echo "ok: no marker, no systemctl call"

# 2. UI-only stop (crash, UI restart): the stock daemon comes back, without replacing queued jobs.
state target_active gs_active; run yes
[ "$RC" = 0 ] || fail "UI-only stop: exit $RC"
grep -qx 'systemctl --user --job-mode=fail start steamos-powerbuttond.service' "$T/log" || fail "UI-only stop: stock daemon not started with --job-mode=fail"
[ ! -e "$MARK" ] || fail "UI-only stop: marker left behind"
echo "ok: UI-only stop restores the stock daemon (--job-mode=fail)"

# 3. Session teardown as on the Deck 2026-11-15 21:51:14: the target has stopped, gamescope still reads
#    active because its stop job waits behind steam-launcher. No start at all.
state gs_active; echo 4711 >"$T/gs_job"; run yes
[ "$RC" = 0 ] || fail "teardown: exit $RC"
started && fail "teardown: stock daemon started while the session was stopping"
[ ! -e "$MARK" ] || fail "teardown: marker left behind (the next UI start would refuse the handoff)"
echo "ok: session teardown, nothing started"

# 4. Stop job queued for gamescope while the target still reads active.
state target_active gs_active; echo 4711 >"$T/gs_job"; run yes
started && fail "queued gamescope job: stock daemon started"
[ "$RC" = 0 ] && [ ! -e "$MARK" ] || fail "queued gamescope job: exit $RC or marker left"
echo "ok: queued gamescope job, nothing started"

# 5. gamescope already gone (Desktop switch, killed compositor).
state; run yes
started && fail "gamescope gone: stock daemon started"
[ "$RC" = 0 ] && [ ! -e "$MARK" ] || fail "gamescope gone: exit $RC or marker left"
echo "ok: gamescope gone, nothing started"

# 6. systemd refuses the start (a stop job appeared after the checks): no error exit, marker consumed.
state target_active gs_active start_fails; run yes
[ "$RC" = 0 ] || fail "refused start: exit $RC (ExecStopPost failure would mark the UI unit failed)"
[ ! -e "$MARK" ] || fail "refused start: marker left behind"
grep -q 'not started' "$T/out" || fail "refused start: not reported"
echo "ok: refused start is reported, not fatal"

# 7. No plain (job-replacing) start of the stock daemon anywhere in the hook.
if grep -n 'systemctl' "$HOOK" | grep ' start ' | grep -v -- '--job-mode=fail' >/dev/null; then fail "hook contains a plain systemctl start"; fi
echo "ok: power-recover hook"
