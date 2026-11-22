#!/usr/bin/env bash
# Uses an existing, owner-authorized SSH account. The remote Python source goes via stdin.
set -euo pipefail
if [[ $# -lt 1 || ! $1 =~ ^([A-Za-z0-9_.-]+@)?[A-Za-z0-9][A-Za-z0-9_.-]*$ ]]; then
    echo "usage: $0 USER@HOST [--seconds N] [--interval N] [--pid LABEL=PID ...]" >&2
    exit 2
fi
host=$1
shift
# ssh joins the remote arguments with spaces. Validate every token first.
args=()
while (( $# )); do
    if (( $# < 2 )); then exit 2; fi
    case $1 in
        --seconds|--interval) [[ $2 =~ ^[0-9]+([.][0-9]+)?$ ]] || exit 2 ;;
        --pid) [[ $2 =~ ^[A-Za-z][A-Za-z0-9_-]*=[0-9]+$ ]] || exit 2 ;;
        *) echo "unknown sampler argument: $1" >&2; exit 2 ;;
    esac
    args+=("$1" "$2")
    shift 2
done
exec ssh -T -o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=5 \
    -o UpdateHostKeys=no -o ClearAllForwardings=yes -o ForwardAgent=no -o ForwardX11=no \
    -o PermitLocalCommand=no -o ServerAliveInterval=5 -o ServerAliveCountMax=2 -- "$host" \
    python3 -u - "${args[@]}" < "$(dirname -- "$0")/sample.py"
