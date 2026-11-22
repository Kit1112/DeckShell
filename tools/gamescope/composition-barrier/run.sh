#!/usr/bin/env bash
# PC only. Never accepts an existing DISPLAY: it creates and owns a private Xvfb.
# XVFB=/path/to/unpacked/Xvfb dispatch-test -- tools/gamescope/composition-barrier/run.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
XVFB_PID=""
cleanup() {
    [ -z "$XVFB_PID" ] || { kill "$XVFB_PID" 2>/dev/null || true; wait "$XVFB_PID" 2>/dev/null || true; }
    rm -rf "$TMP"
}
trap cleanup EXIT
umask 077
OUT="${BARRIER_RESULTS:-$HOME/.cache/deckshell/composition-barrier/$(date +%Y%m%d-%H%M%S)-$$}"
mkdir -p "$OUT"
cc -shared -fPIC -O2 -Wall -Wextra -Werror -o "$TMP/guard.so" "$HERE/xlib-guard.c" -lX11 -ldl -pthread
cc -O2 -Wall -Wextra -Werror -o "$TMP/fake-steam" "$HERE/fake-steam.c" -lX11 -lxcb
# An isolated server with a random MIT cookie. FamilyWild auth works with the
# dynamically allocated display number; never disable access control with -ac.
python3 - "$TMP/auth" <<'PY'
import secrets, struct, sys
from pathlib import Path
fields = (b"", b"", b"MIT-MAGIC-COOKIE-1", secrets.token_bytes(16))
Path(sys.argv[1]).write_bytes(struct.pack(">H", 65535) + b"".join(
    struct.pack(">H", len(f)) + f for f in fields))
PY
export XAUTHORITY="$TMP/auth"
"${XVFB:-Xvfb}" -displayfd 3 -screen 0 320x200x24 -nolisten tcp -auth "$XAUTHORITY" \
    3>"$TMP/display" >"$OUT/xvfb.log" 2>&1 &
XVFB_PID=$!
for _ in {1..100}; do
    [ ! -s "$TMP/display" ] || break
    kill -0 "$XVFB_PID" 2>/dev/null || { cat "$OUT/xvfb.log"; exit 1; }
    sleep 0.05
done
[ -s "$TMP/display" ] || { echo 'Xvfb readiness timeout' >&2; exit 1; }
export DISPLAY=":$(cat "$TMP/display")"
unset LD_PRELOAD
timeout 600s python3 "$HERE/rig.py" --writer "$TMP/fake-steam" --library "$TMP/guard.so" \
    --trace "$OUT/frames.jsonl" "$@" | tee "$OUT/summary.jsonl"
echo "results: $OUT"
