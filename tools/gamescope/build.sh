#!/usr/bin/env bash
# glibc 2.34-compatible release build. Each canonical worktree gets its own Cargo cache.
set -eu
SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$SELF/../.." && pwd -P)"
# CARGO_TARGET_DIR is a cache root, never a directly shared Cargo target directory.
CACHE_ROOT="${CARGO_TARGET_DIR:-$HOME/.cache/deckshell/zig-target}"
case "$CACHE_ROOT" in /*) ;; *) CACHE_ROOT="$ROOT/$CACHE_ROOT" ;; esac
KEY="$(printf '%s' "$ROOT" | sha256sum | cut -d' ' -f1)"
export CARGO_TARGET_DIR="$CACHE_ROOT/worktrees/$KEY"
BIN="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/deckshell"
if [ "${1:-}" = --bin-path ] && [ "$#" -eq 1 ]; then
    printf '%s\n' "$BIN"
    exit 0
fi
# Cargo options such as --target-dir, --profile, --target or --bin could make the
# expected path refer to an old artifact. Do not accept an ambiguous output.
if [ "$#" -ne 0 ]; then
    echo 'build.sh accepts no cargo arguments (use --bin-path to print the output path)' >&2
    exit 2
fi
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export PATH="$HOME/.local/opt/zig:$PATH"
export PKG_CONFIG_ALLOW_CROSS=1
GLIBC_BASE="${GLIBC_BASE:-2.34}"
command -v zig >/dev/null && command -v cargo-zigbuild >/dev/null || { echo 'need zig and cargo-zigbuild' >&2; exit 1; }
cd "$ROOT"
cargo zigbuild --release -p deckshell --bin deckshell --target "x86_64-unknown-linux-gnu.$GLIBC_BASE"
[ -x "$BIN" ] || { echo "build did not produce $BIN" >&2; exit 1; }
"$SELF/check-glibc.sh" "$BIN" "$GLIBC_BASE"
echo "built $BIN"
