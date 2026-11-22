#!/usr/bin/env bash
# hold-файл перезапуска Steam (задача 430): свежий — Steam не поднимать; устаревший — игнорировать и удалить;
# без файла — поднимать. Проверяется hold_active из session/deckshell-session.sh.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
log() { :; }
HOLD="$T/hold"; HOLD_MAX=240
eval "$(sed -n '/^hold_active() {/,/^}/p' "$HERE/../deckshell-session.sh")"
hold_active && { echo "FAIL: без файла hold активен" >&2; exit 1; }
: >"$HOLD"; hold_active || { echo "FAIL: свежий hold не активен" >&2; exit 1; }
touch -d '10 minutes ago' "$HOLD"
hold_active && { echo "FAIL: устаревший hold активен" >&2; exit 1; }
[ ! -e "$HOLD" ] || { echo "FAIL: устаревший hold не удалён" >&2; exit 1; }
echo "ok: hold-файл: свежий держит, устаревший снимается"
