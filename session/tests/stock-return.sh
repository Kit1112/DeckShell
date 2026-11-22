#!/usr/bin/env bash
# Returning to stock Steam must leave no DeckShell UI behind and no stale base layer.
# Runs the real session script against fake systemctl/xprop/steam; touches nothing outside a temp dir.
set -eu
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; SCRIPT="$HERE/../deckshell-session.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/conf/deckshell" "$T/state"
cat >"$T/bin/systemctl" <<'F'
#!/usr/bin/env bash
echo "systemctl $*" >>"$T_LOG"
case "$*" in
    *is-active*) [ -e "$T_UI" ] ;;
    *" stop "*)  rm -f "$T_UI" ;;
esac
F
cat >"$T/bin/xprop" <<'F'
#!/usr/bin/env bash
echo "xprop $*" >>"$T_LOG"
F
cat >"$T/bin/steam" <<'F'
#!/usr/bin/env bash
echo "steam $*" >>"$T_LOG"
F
cat >"$T/bin/stock" <<'F'
#!/usr/bin/env bash
if [ -e "$T_UI" ]; then echo "stock ui=alive" >>"$T_LOG"; else echo "stock ui=gone" >>"$T_LOG"; fi
F
chmod +x "$T/bin/"*
fail() { echo "FAIL: $1" >&2; sed 's/^/  /' "$T/log" >&2; exit 1; }
run() {   # $1 = mode
    : >"$T/log"; : >"$T/ui"; echo "$1" >"$T/conf/deckshell/mode"
    T_LOG="$T/log" T_UI="$T/ui" PATH="$T/bin:$PATH" DISPLAY=:99 XDG_RUNTIME_DIR="$T" \
        XDG_CONFIG_HOME="$T/conf" XDG_STATE_HOME="$T/state" DECKSHELL_SYS_DIR="$T/none" \
        DECKSHELL_STOCK_BIN="$T/bin/stock" STEAM_BIN="$T/bin/steam" bash "$SCRIPT"
}
# 1. mode=stock while a UI unit from an earlier launcher is still active.
run stock
grep -q 'systemctl --user stop deckshell-ui.service' "$T/log" || fail "leftover UI unit was not stopped"
grep -q 'xprop -root -remove GAMESCOPECTRL_BASELAYER_APPID' "$T/log" || fail "base layer was not released"
[ "$(tail -n1 "$T/log")" = "stock ui=gone" ] || fail "stock Steam started while the UI was alive"
stop_line="$(grep -n 'stop deckshell-ui' "$T/log" | head -n1 | cut -d: -f1)"
atom_line="$(grep -n 'BASELAYER_APPID' "$T/log" | head -n1 | cut -d: -f1)"
[ "$stop_line" -lt "$atom_line" ] || fail "base layer released before the UI stopped (it would re-claim it)"
# 2. The disable marker takes the same path.
: >"$T/conf/deckshell/disable"
run hybrid
[ "$(tail -n1 "$T/log")" = "stock ui=gone" ] || fail "disable marker: stock started while the UI was alive"
rm -f "$T/conf/deckshell/disable"
# 3. The UI unit must stop with the launcher/session but never be restarted with them.
grep -q 'StopPropagatedFrom=gamescope-session.service steam-launcher.service' "$SCRIPT" || fail "UI unit does not stop with the session"
if grep -q -- '--property="PartOf=' "$SCRIPT"; then fail "PartOf restarts the UI unit together with the launcher"; fi
echo "ok: stock return leaves one UI"
