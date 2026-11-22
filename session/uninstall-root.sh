#!/usr/bin/env bash
# Reverse install-root.sh.  ONE command:   sudo bash ~/PATH/TO/session/uninstall-root.sh  [--purge] [--user NAME]
# Removes everything it created, restores backed-up originals byte-identically (verified with cmp),
# logs to /var/log/deckshell-install.log.  Works from desktop mode, SSH or a TTY.  Idempotent.
# --purge also deletes /etc/deckshell (kill-switch/mode files) and /var/lib/deckshell.
set -u
[ "$(id -u)" = 0 ] || { echo "run as root:  sudo bash $0" >&2; exit 1; }
PURGE=0; U="${SUDO_USER:-deck}"
while [ $# -gt 0 ]; do case "$1" in --purge) PURGE=1 ;; --user) U="$2"; shift ;; *) echo "unknown option $1" >&2; exit 2 ;; esac; shift; done
VAR=/var/lib/deckshell; ETC=/etc/deckshell; LOGF=/var/log/deckshell-install.log; MAN=$VAR/install.manifest; BK=$VAR/backup
DROPDIR=/etc/systemd/user/steam-launcher.service.d; DROPIN=$DROPDIR/deckshell.conf
log() { printf '%s [uninstall-root %s] %s\n' "$(date '+%F %T')" "$$" "$*" | tee -a "$LOGF" >&2; }
log "begin"
RO=0
if ! touch "$VAR/.w" 2>/dev/null; then
    command -v steamos-readonly >/dev/null && { steamos-readonly disable; RO=1; log "readonly disabled temporarily"; }
fi
[ "$RO" = 1 ] && trap 'steamos-readonly enable' EXIT
rm -f "$VAR/.w"

if [ -f "$MAN" ]; then
    while IFS='=' read -r kind path; do
        case "$kind" in
            created) [ -e "$path" ] && rm -f "$path" && log "removed $path" ;;
            backup)  if [ -e "$BK$path" ]; then cp -a "$BK$path" "$path" && cmp -s "$BK$path" "$path" \
                        && log "restored $path (byte-identical)" || log "ERROR restoring $path"; fi ;;
        esac
    done <"$MAN"
fi
# belt and braces: the hook must be gone even if the manifest is lost
if [ -e "$DROPIN" ] && grep -q deckshell-managed "$DROPIN"; then rm -f "$DROPIN"; log "removed $DROPIN (no manifest entry)"; fi
rmdir "$DROPDIR" 2>/dev/null || true
rm -rf "$VAR/session" "$VAR/bin" "$VAR/lib" "$MAN"
if [ "$PURGE" = 1 ]; then rm -rf "$ETC" "$VAR"; log "purged $ETC $VAR"
else rmdir "$VAR/backup" "$VAR" 2>/dev/null || true; fi
runuser -u "$U" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$U")" systemctl --user daemon-reload 2>/dev/null || true
log "done"
echo "Uninstalled. Stock Steam UI returns at next gaming-mode start (or: systemctl --user restart steam-launcher from the live session)."
[ -d "$ETC" ] && echo "Left $ETC (kill-switch/mode files, harmless). --purge removes it."
exit 0
