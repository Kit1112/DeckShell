#!/usr/bin/env bash
# Read-only sampler: CPU%/RSS/threads of named processes + system load + (optional) GPU/power/temp, 1 line/interval, CSV.
# Works locally, or on the Deck via:  ssh deck@HOST 'bash -s' -- <args> < tools/perf/proc-sampler.sh   (no sudo, nothing written on the target)
# Usage: proc-sampler.sh SECONDS INTERVAL "comm1,comm2,..."   -> CSV on stdout
# CPU% is in percent of ONE core (100 = one full core), from /proc/PID/stat utime+stime deltas.
set -u
DUR=${1:-60}; IV=${2:-1}; NAMES=${3:-deckshell,gamescope,gamescope-wl,steam,steamwebhelper,Xwayland,mangoapp}
CLK=$(getconf CLK_TCK)
declare -A PREV
now_ticks() { awk '{print $14+$15}' "/proc/$1/stat" 2>/dev/null; }
pids_of() { for n in ${NAMES//,/ }; do pgrep -x "$n" 2>/dev/null | sed "s/^/$n /"; done; }
read_gpu() {  # AMD (Deck, sysfs) or NVIDIA (nvidia-smi); empty fields if unavailable
  local g="" p="" t="" f
  for f in /sys/class/drm/card*/device/gpu_busy_percent; do [ -r "$f" ] && { g=$(<"$f"); break; }; done
  for f in /sys/class/hwmon/hwmon*/power1_average; do [ -r "$f" ] && [ "$(<"${f%/*}/name")" = amdgpu ] && { p=$(awk '{printf "%.2f",$1/1e6}' "$f"); break; }; done
  for f in /sys/class/hwmon/hwmon*/temp1_input; do [ -r "$f" ] && [ "$(<"${f%/*}/name")" = amdgpu ] && { t=$(awk '{printf "%.1f",$1/1000}' "$f"); break; }; done
  if [ -z "$g" ] && command -v nvidia-smi >/dev/null; then
    IFS=, read -r g p t < <(nvidia-smi --query-gpu=utilization.gpu,power.draw,temperature.gpu --format=csv,noheader,nounits 2>/dev/null | tr -d ' ')
  fi
  echo "${g:-},${p:-},${t:-}"
}
echo "t,proc,pid,cpu_pct,rss_mb,threads,gpu_busy_pct,gpu_power_w,gpu_temp_c,mem_used_mb,loadavg1"
end=$((SECONDS+DUR)); t0=$SECONDS; last=$(date +%s.%N)
while [ $SECONDS -lt $end ]; do
  sleep "$IV"
  cur=$(date +%s.%N); dt=$(awk "BEGIN{print $cur-$last}"); last=$cur
  gpu=$(read_gpu)
  mem=$(awk '/MemTotal/{t=$2}/MemAvailable/{a=$2}END{printf "%.0f",(t-a)/1024}' /proc/meminfo)
  la=$(cut -d' ' -f1 /proc/loadavg)
  while read -r name pid; do
    [ -z "${pid:-}" ] && continue
    tk=$(now_ticks "$pid") || continue; [ -z "$tk" ] && continue
    pv=${PREV[$pid]:-$tk}; PREV[$pid]=$tk
    cpu=$(awk "BEGIN{printf \"%.1f\",($tk-$pv)/$CLK/$dt*100}")
    rss=$(awk '/VmRSS/{printf "%.1f",$2/1024}' "/proc/$pid/status" 2>/dev/null)
    thr=$(awk '/Threads/{print $2}' "/proc/$pid/status" 2>/dev/null)
    echo "$((SECONDS-t0)),$name,$pid,$cpu,$rss,$thr,$gpu,$mem,$la"
  done < <(pids_of)
done
