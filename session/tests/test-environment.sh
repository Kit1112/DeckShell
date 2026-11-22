#!/usr/bin/env bash
set -eu
SCRIPT="$(cd "$(dirname "$0")/.." && pwd)/deckshell-session.sh"
check() {
    env DECKSHELL_DRYRUN=1 DECKSHELL_CONTROL=/tmp/control DECKSHELL_TEST_HID_SOCKET=/tmp/hid DECKSHELL_TEST_FUTURE=yes "$@" bash "$SCRIPT" --ui-exec env
}
clean="$(check)"
if printf '%s\n' "$clean" | grep -Eq '^DECKSHELL_(DRYRUN|CONTROL|TEST_)'; then echo 'test environment leaked' >&2; exit 1; fi
allowed="$(check DECKSHELL_ALLOW_TEST=1)"
for entry in DECKSHELL_DRYRUN=1 DECKSHELL_CONTROL=/tmp/control DECKSHELL_TEST_HID_SOCKET=/tmp/hid DECKSHELL_TEST_FUTURE=yes; do
    printf '%s\n' "$allowed" | grep -Fxq "$entry" || { echo "missing $entry" >&2; exit 1; }
done
echo 'ok: test environment is opt-in'
