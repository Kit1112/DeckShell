#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 2 || ! $1 =~ ^[1-9][0-9]*$ || ! $2 =~ ^[1-9][0-9]*(,[1-9][0-9]*)*$ ]]; then
    echo "usage: $0 SAMPLE_COUNT PID[,PID...]" >&2
    exit 2
fi
# Retain only selected process rows. A missing row or "-" is unavailable.
LC_ALL=C nvidia-smi pmon -s um -d 1 -c "$1" -o DT |
    awk -v pids="$2" '
        BEGIN { count = split(pids, list, ","); for (i = 1; i <= count; i++) selected[list[i]] = 1 }
        /^#/ || $4 in selected
    '
