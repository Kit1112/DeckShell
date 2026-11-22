#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 2 || ! $1 =~ ^[0-9]+([.][0-9]+)?$ || ! $2 =~ ^[1-9][0-9]*(,[1-9][0-9]*)*$ ]]; then
    echo "usage: $0 SECONDS PID[,PID...]" >&2
    exit 2
fi
# Read-only, user-space software events. Keep failures visible when perf is restricted.
exec env LC_ALL=C perf stat -x ';' -e task-clock:u,context-switches:u,page-faults:u \
    -p "$2" -- sleep "$1"
