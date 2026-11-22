#!/usr/bin/env bash
# DeckShell installer for Steam Deck (SteamOS 3.x). Run it in Desktop Mode (Konsole) or over SSH, as the normal user "deck".
#
#   curl -fsSL https://github.com/Kit1112/DeckShell/releases/latest/download/install.sh | bash
#   curl -fsSL https://github.com/Kit1112/DeckShell/releases/latest/download/install.sh | bash -s -- --enable
#
# Options:
#   --enable        also switch DeckShell on for the next Gaming Mode start (default: installed but OFF = stock Steam UI)
#   --version vX.Y.Z   install a specific release instead of the latest
#   --uninstall     remove the hook and (with --purge) all files; Steam is never modified
#   --purge         together with --uninstall: also delete ~/.local/share/deckshell
#
# Everything is user-level: no sudo, nothing in /usr, the SteamOS image and Steam itself are not modified.
set -eu
REPO="Kit1112/DeckShell"; VERSION=latest; ENABLE=0; UNINSTALL=0; PURGE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --enable) ENABLE=1 ;;
        --version) VERSION="${2:?--version needs a value}"; shift ;;
        --uninstall) UNINSTALL=1 ;;
        --purge) PURGE=1 ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac; shift
done
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/deckshell"; CONF="${XDG_CONFIG_HOME:-$HOME/.config}/deckshell"
say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }

if [ "$UNINSTALL" = 1 ]; then
    [ -x "$DATA/session/uninstall.sh" ] || die "DeckShell is not installed ($DATA/session/uninstall.sh missing)"
    bash "$DATA/session/rollback.sh" || true
    bash "$DATA/session/uninstall.sh" ${PURGE:+--purge} || true
    [ "$PURGE" = 1 ] && rm -rf "$DATA" "$CONF" && say "removed $DATA and $CONF"
    say "done: the next Gaming Mode start is stock Steam"; exit 0
fi

[ "$(id -u)" != 0 ] || die "run as the normal user (deck), not root"
[ "$(uname -m)" = x86_64 ] || die "x86_64 only (Steam Deck)"
[ -f /usr/lib/systemd/user/steam-launcher.service ] || die "this does not look like SteamOS Gaming Mode (steam-launcher.service not found)"
for t in curl tar sha256sum; do command -v "$t" >/dev/null || die "need $t"; done
[ -d "$HOME/.local/share/Steam/steamui" ] || die "Steam client files not found in ~/.local/share/Steam (start Steam once first)"

if [ -n "${DECKSHELL_BASE_URL:-}" ]; then BASE="$DECKSHELL_BASE_URL"   # testing override
elif [ "$VERSION" = latest ]; then BASE="https://github.com/$REPO/releases/latest/download"; else BASE="https://github.com/$REPO/releases/download/$VERSION"; fi
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
say "downloading DeckShell ($VERSION)"
curl -fSL --progress-bar -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS"
PKG="$(awk '/deckshell-.*-linux-x86_64\.tar\.gz/ {print $2}' "$TMP/SHA256SUMS" | head -n1)"
[ -n "$PKG" ] || die "no package listed in SHA256SUMS"
curl -fSL --progress-bar -o "$TMP/$PKG" "$BASE/$PKG"
say "verifying checksum"
(cd "$TMP" && grep " $PKG\$" SHA256SUMS | sha256sum -c -) || die "checksum mismatch, aborting"

say "installing into $DATA"
mkdir -p "$DATA"
if [ -x "$DATA/bin/deckshell" ]; then cp -f "$DATA/bin/deckshell" "$DATA/bin/deckshell.before-update"; say "previous binary kept as bin/deckshell.before-update"; fi
tar -xzf "$TMP/$PKG" -C "$DATA" --no-same-owner
chmod 0755 "$DATA/bin/deckshell" "$DATA/bin/deckshell-run" "$DATA"/session/*.sh "$DATA"/session/*.py 2>/dev/null || true
bash "$DATA/session/install.sh"
if [ "$ENABLE" = 1 ]; then echo hybrid >"$CONF/mode"; say "DeckShell ENABLED for the next Gaming Mode start"; fi

cat <<EOT

Installed DeckShell $(cat "$DATA/VERSION" 2>/dev/null).  Mode: $(cat "$CONF/mode")   (stock = off, hybrid = DeckShell on)

  Turn ON :   echo hybrid > $CONF/mode      then restart Gaming Mode (or reboot)
  Turn OFF:   echo stock  > $CONF/mode      (back to the stock Steam UI)
  Emergency:  touch $CONF/disable, or hold Volume-Down (or B) for 2 s while Gaming Mode starts
  Rollback:   bash $DATA/session/rollback.sh
  Remove all: curl -fsSL https://github.com/$REPO/releases/latest/download/install.sh | bash -s -- --uninstall --purge
EOT
