#!/usr/bin/env bash
set -euo pipefail
if [[ $# -lt 5 || ! $2 =~ ^[1-9][0-9]*$ || ! $3 =~ ^[1-9][0-9]*$ || $4 != -- ]]; then
    echo "usage: $0 NEW_OUTPUT_DIR WARMUP_SECONDS LOG_SECONDS -- COMMAND [ARG...]" >&2
    exit 2
fi
umask 077
out=$(realpath -m -- "$1")
if [[ $out =~ [[:space:],:=] ]]; then
    echo 'choose an output path without whitespace, commas, colons or equals signs' >&2
    exit 2
fi
mkdir -- "$out"
export MANGOHUD_CONFIG="output_folder=$out,autostart_log=$2,log_duration=$3,log_interval=0,permit_upload=0,fps,frametime,cpu_stats,gpu_stats,cpu_temp,gpu_temp,cpu_power,gpu_power"
unset MANGOHUD_CONFIGFILE
shift 4
# Use gamescope --mangoapp as COMMAND under gamescope; use mangohud COMMAND for direct runs.
exec "$@"
