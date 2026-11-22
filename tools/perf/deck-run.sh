#!/usr/bin/env bash
# DECK part of the comparison. NOT RUN until the owner grants access. Read-only over ssh: no sudo, no restarts,
# no mode switch, no file written on the Deck (the sampler is fed through `bash -s` on stdin).
# The owner switches modes and launches the same game by hand; this script only observes one window per call.
#   deck-run.sh preflight                       reachability + what mode/game is running (read-only), refuses if owner is active
#   deck-run.sh sample LABEL [DUR] [WARM]       LABEL e.g. deckshell-balatro / stock-balatro; samples CPU/RSS/GPU/power/temp
# Env: DECK_HOST (default deck@192.168.1.50), OUT (default ~/.cache/deckshell-perf/deck/<date>), IDLE_MIN (owner-idle guard)
# MangoHud frametime/FPS: the owner enables the stock Steam performance overlay level "full" with logging, OR launch option
#   MANGOHUD=1 MANGOHUD_CONFIG=output_folder=/tmp/mh,autostart_log=15,log_duration=120  %command%   (same for both modes)
# and this script copies /tmp/mh/*.csv back with `fetch-mangohud`. Without MangoHud only CPU/GPU/power/temp are collected.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
HOST=${DECK_HOST:-deck@192.168.1.50}; OUT=${OUT:-$HOME/.cache/deckshell-perf/deck/$(date +%Y%m%d)}
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=5 "$HOST")
mkdir -p "$OUT"
case ${1:-} in
preflight)
  "${SSH[@]}" 'echo host=$(hostname); uptime; echo -n "session: "; pgrep -a -x gamescope | cut -c1-120 | head -2; pgrep -x steam >/dev/null && echo steam=running;
    for f in /sys/class/power_supply/BAT1/status /sys/class/power_supply/BAT1/capacity /sys/class/drm/card0/device/power_dpm_force_performance_level \
      /sys/class/hwmon/hwmon*/power1_average; do [ -r $f ] && echo "$f=$(cat $f)"; done; echo -n "owner idle s (logind): "; loginctl show-session $(loginctl | awk "/deck/{print \$1;exit}") -p IdleSinceHint 2>/dev/null' ;;
sample)
  LABEL=${2:?label}; DUR=${3:-120}; WARM=${4:-20}
  echo "owner: game must already be running at the same scene/settings; warmup ${WARM}s then ${DUR}s"; sleep "$WARM"
  "${SSH[@]}" "bash -s -- $DUR 1 deckshell,gamescope,gamescope-wl,steam,steamwebhelper,Xwayland,mangoapp" <"$HERE/proc-sampler.sh" >"$OUT/$LABEL.proc.csv"
  "${SSH[@]}" 'cat /sys/class/power_supply/BAT1/status /sys/class/power_supply/BAT1/power_now 2>/dev/null' >"$OUT/$LABEL.battery.txt" || true
  "$HERE/summarize.py" proc "$OUT/$LABEL.proc.csv" | tee "$OUT/$LABEL.summary.txt" ;;
fetch-mangohud)
  LABEL=${2:?label}; mkdir -p "$OUT/$LABEL.mh"; scp -q "$HOST:/tmp/mh/*.csv" "$OUT/$LABEL.mh/" && ls "$OUT/$LABEL.mh"
  for f in "$OUT/$LABEL.mh"/*.csv; do echo "-- $f"; "$HERE/summarize.py" mango "$f"; done ;;
*) sed -n 2,13p "$0"; exit 2;;
esac
