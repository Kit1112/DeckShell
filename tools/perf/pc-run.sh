#!/usr/bin/env bash
# PC part of the DeckShell-vs-Steam performance comparison. Everything runs in a *headless nested gamescope*:
# no DRM, no real session, no Deck, no Steam, no writes outside $OUT. Results stay local.
#   pc-run.sh ui-idle     DECKSHELL_BIN: UI alone (idle home screen), CPU/RSS/GPU of deckshell + gamescope
#   pc-run.sh synthetic   vkcube alone ("game" without UI): baseline for the compositor
#   pc-run.sh game+ui     vkcube + deckshell beside it (UI under load from a game), plus framestats
#   pc-run.sh all
# Env: DECKSHELL_BIN (release build), OUT (default ~/.cache/deckshell-perf/runs/<stamp>), DUR (s, default 60), WARM (s, default 10)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${DECKSHELL_BIN:-$HOME/.cache/deckshell-perf/target/release/deckshell}
OUT=${OUT:-$HOME/.cache/deckshell-perf/runs/$(date +%Y%m%d-%H%M%S)}; DUR=${DUR:-60}; WARM=${WARM:-10}
W=1280; H=800; mkdir -p "$OUT"
export DECKSHELL_DRYRUN=1 DECKSHELL_INPUT=none DECKSHELL_FRAMESTATS=1   # never writes to Steam/Deck state, no pads

run_case() {  # name, with_game(0/1), with_ui(0/1)
  local name=$1 game=$2 ui=$3 ctl="$OUT/$1.ctl" inner="$OUT/$1.inner.sh"
  {
    echo '#!/usr/bin/env bash'
    [ "$ui" = 1 ] && echo "DECKSHELL_CONTROL='$ctl' '$BIN' >'$OUT/$name.ui.log' 2>&1 &"
    [ "$game" = 1 ] && echo "vkcube --suppress_popups --c $(( (DUR+WARM+15)*60 )) >/dev/null 2>&1 &"
    echo "sleep $((DUR+WARM+10))"
  } >"$inner"; chmod +x "$inner"
  gamescope --backend headless -W $W -H $H -w $W -h $H -r 60 -- "$inner" >"$OUT/$name.gamescope.log" 2>&1 &
  local gs=$!
  sleep "$WARM"
  [ "$ui" = 1 ] && [ -S "$ctl" ] && (echo "framestats reset"; sleep 1) | timeout 3 socat - "UNIX-CONNECT:$ctl" >/dev/null 2>&1 || true
  # sampler reads /proc + sysfs/nvidia-smi only
  "$HERE/proc-sampler.sh" "$DUR" 1 "deckshell,gamescope,gamescope-wl,vkcube,Xwayland" >"$OUT/$name.proc.csv"
  [ "$ui" = 1 ] && [ -S "$ctl" ] && (echo framestats; sleep 1) | timeout 3 socat - "UNIX-CONNECT:$ctl" >"$OUT/$name.framestats.txt" 2>/dev/null || true
  kill "$gs" 2>/dev/null || true; wait "$gs" 2>/dev/null || true
  echo "== $name ==" | tee -a "$OUT/summary.txt"
  "$HERE/summarize.py" proc "$OUT/$name.proc.csv" | tee -a "$OUT/summary.txt"
  if [ -s "$OUT/$name.framestats.txt" ]; then { echo "-- deckshell framestats --"; cat "$OUT/$name.framestats.txt"; } | tee -a "$OUT/summary.txt"; fi
}
case ${1:-all} in
  ui-idle) run_case ui-idle 0 1;;
  synthetic) run_case synthetic 1 0;;
  game+ui) run_case game+ui 1 1;;
  all) run_case synthetic 1 0; run_case ui-idle 0 1; run_case game+ui 1 1;;
  *) sed -n 2,9p "$0"; exit 2;;
esac
echo "results: $OUT"
