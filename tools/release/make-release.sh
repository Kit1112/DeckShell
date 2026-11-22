#!/usr/bin/env bash
# make-release.sh VERSION BINARY  -> dist/deckshell-VERSION-linux-x86_64.tar.gz + SHA256SUMS (+ install.sh copy)
# BINARY: release build from tools/gamescope/build.sh (glibc <= 2.34, so it runs on SteamOS).
set -eu
V="${1:?version, e.g. v0.1.0}"; BIN="${2:?path to built deckshell binary}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; OUT="$ROOT/dist"; S="$(mktemp -d)"; trap 'rm -rf "$S"' EXIT
"$ROOT/tools/gamescope/check-glibc.sh" "$BIN" 2.34
mkdir -p "$S/bin" "$S/session" "$S/docs" "$OUT"
install -m 0755 "$BIN" "$S/bin/deckshell"
install -m 0755 "$ROOT/session/deckshell-run" "$S/bin/deckshell-run"
for f in deckshell-session.sh session-log.py power-recover.sh held-button.py install.sh uninstall.sh install-root.sh uninstall-root.sh rollback.sh dropin.conf.in; do install -m 0755 "$ROOT/session/$f" "$S/session/$f"; done
for f in gsctl.sh stage1.py deck-session-restart.sh; do install -m 0755 "$ROOT/tools/gamescope/$f" "$S/session/$f"; done
install -m 0644 "$ROOT/docs/ROLLBACK.md" "$S/session/ROLLBACK.md"
install -m 0644 "$ROOT/LICENSE" "$ROOT/NOTICE" "$S/"
echo "$V" >"$S/VERSION"
P="deckshell-$V-linux-x86_64.tar.gz"
tar -C "$S" --owner=0 --group=0 -czf "$OUT/$P" bin session VERSION LICENSE NOTICE
cp "$ROOT/install.sh" "$OUT/install.sh"
(cd "$OUT" && sha256sum "$P" > SHA256SUMS && cat SHA256SUMS)
