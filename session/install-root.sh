#!/usr/bin/env bash
# SYSTEM-LEVEL install of the DeckShell gaming-mode hook.  Run by the USER as ONE command:
#     sudo bash ~/PATH/TO/session/install-root.sh            (options: --user NAME  --from DIR  --dry-run)
# Never run by an AI agent.  Idempotent.  Undo: sudo bash .../session/uninstall-root.sh
#
# Writes ONLY to /etc and /var (these survive SteamOS A/B updates; /usr is wiped by them):
#   /etc/systemd/user/steam-launcher.service.d/deckshell.conf   the hook (drop-in over the OS unit)
#   /etc/deckshell/                                              kill switches: disable, mode
#   /var/lib/deckshell/{bin,session,backup,install.manifest}     binary, launcher, scripts, backups
#   /var/log/deckshell-install.log                               every change logged here
# Nothing under /usr is touched, so steamos-readonly is normally NOT needed; if a target turns
# out read-only the script disables it and re-enables it on exit.  Pre-existing files at any
# target path are saved to /var/lib/deckshell/backup and restored byte-identically on uninstall.
# The default mode is "stock" (no behaviour change) until:  echo hybrid | sudo tee /etc/deckshell/mode
# (or ~/.config/deckshell/mode, which wins).  Nothing is restarted; takes effect at next gaming-mode start.
set -eu
[ "$(id -u)" = 0 ] || { echo "run as root:  sudo bash $0" >&2; exit 1; }
DRY=0; U="${SUDO_USER:-}"; FROM=""
while [ $# -gt 0 ]; do case "$1" in
    --dry-run) DRY=1 ;; --user) U="$2"; shift ;; --from) FROM="$2"; shift ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;; *) echo "unknown option $1" >&2; exit 2 ;; esac; shift; done
[ -n "$U" ] && [ "$U" != root ] || { echo "cannot tell target user; use --user deck" >&2; exit 1; }
UHOME="$(getent passwd "$U" | cut -d: -f6)"
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$SELF/.." && pwd)"
FROM="${FROM:-$UHOME/.local/share/deckshell}"
VAR=/var/lib/deckshell; ETC=/etc/deckshell; LOGF=/var/log/deckshell-install.log
DROPDIR=/etc/systemd/user/steam-launcher.service.d; DROPIN=$DROPDIR/deckshell.conf
MAN=$VAR/install.manifest; BK=$VAR/backup

log() { printf '%s [install-root %s] %s\n' "$(date '+%F %T')" "$$" "$*" | tee -a "$LOGF" >&2; }
do_() { if [ "$DRY" = 1 ]; then echo "[dry-run] $*"; else "$@"; fi; }
fail() { log "ERROR: $*"; exit 1; }

[ -f /usr/lib/systemd/user/steam-launcher.service ] || fail "steam-launcher.service missing: not a SteamOS gaming image"
[ -x "$FROM/bin/deckshell" ] && [ -x "$FROM/bin/deckshell-run" ] || fail "no package in $FROM/bin (run tools/gamescope/install-package.sh first, or --from DIR)"
[ -e "$UHOME/.config/systemd/user/steam-launcher.service.d/deckshell.conf" ] && \
    fail "user-level install present: a same-named drop-in in ~ would shadow the /etc one. Run session/uninstall.sh as $U first"
log "begin: user=$U from=$FROM dry=$DRY"

# readonly handling (only if needed)
RO_CHANGED=0
if [ "$DRY" = 0 ] && ! { mkdir -p "$ETC" "$VAR" && touch "$ETC/.w" "$VAR/.w" && rm -f "$ETC/.w" "$VAR/.w"; } 2>/dev/null; then
    command -v steamos-readonly >/dev/null || fail "/etc or /var not writable and no steamos-readonly"
    log "targets read-only: steamos-readonly disable (will re-enable on exit)"
    steamos-readonly disable; RO_CHANGED=1
    trap 'steamos-readonly enable; echo "$(date "+%F %T") steamos-readonly re-enabled" >>"$LOGF"' EXIT
fi

do_ mkdir -p "$ETC" "$VAR/bin" "$VAR/lib" "$VAR/session" "$BK" "$DROPDIR"
touch "$MAN" 2>/dev/null || true

# put (src, dst, mode): back up a foreign pre-existing dst once, then install; record in manifest
put() {
    local src="$1" dst="$2" mode="$3"
    if [ -e "$dst" ] && ! grep -qxF "created=$dst" "$MAN" 2>/dev/null && ! grep -qxF "backup=$dst" "$MAN" 2>/dev/null; then
        do_ mkdir -p "$BK$(dirname "$dst")"; do_ cp -a "$dst" "$BK$dst"
        [ "$DRY" = 1 ] || echo "backup=$dst" >>"$MAN"; log "backed up existing $dst -> $BK$dst"
    elif [ ! -e "$dst" ]; then
        [ "$DRY" = 1 ] || echo "created=$dst" >>"$MAN"
    fi
    do_ install -m "$mode" "$src" "$dst.tmp.$$" && do_ mv -f "$dst.tmp.$$" "$dst"; log "installed $dst"
}

put "$FROM/bin/deckshell"        "$VAR/bin/deckshell"        0755
put "$FROM/bin/deckshell-run"    "$VAR/bin/deckshell-run"    0755
for f in "$FROM"/lib/*.so*; do [ -e "$f" ] && put "$f" "$VAR/lib/$(basename "$f")" 0755; done
put "$SELF/deckshell-session.sh" "$VAR/session/deckshell-session.sh" 0755
put "$SELF/session-log.py"      "$VAR/session/session-log.py"      0755
GS="$SELF/gsctl.sh"; [ -f "$GS" ] || GS="$ROOT/tools/gamescope/gsctl.sh"
put "$GS" "$VAR/session/gsctl.sh" 0755
put "$SELF/held-button.py"       "$VAR/session/held-button.py" 0755
[ -e "$ETC/mode" ] || { [ "$DRY" = 1 ] || { echo stock >"$ETC/mode"; echo "created=$ETC/mode" >>"$MAN"; }; log "wrote $ETC/mode = stock"; }

TMP="$(mktemp)"; sed -e "s|@RUN@|$VAR/bin/deckshell-run|" -e "s|@SESSION@|$VAR/session/deckshell-session.sh|" "$SELF/dropin.conf.in" >"$TMP"
put "$TMP" "$DROPIN" 0644; rm -f "$TMP"

if [ "$DRY" = 0 ]; then
    systemd-analyze verify "$DROPIN" >/dev/null 2>&1 || true
    runuser -u "$U" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$U")" systemctl --user daemon-reload 2>/dev/null \
        && log "user manager reloaded" || log "note: could not reload user manager (not logged in?) - it is read at next start"
fi
log "done. mode=$(cat "$ETC/mode" 2>/dev/null) ; readonly_changed=$RO_CHANGED"
echo
echo "Installed. Nothing changes until you opt in:   echo hybrid | sudo tee $ETC/mode"
echo "Kill switches: sudo touch $ETC/disable | touch ~/.config/deckshell/disable | hold volume-down (or B) 2 s at boot"
echo "Uninstall:     sudo bash $SELF/uninstall-root.sh"
