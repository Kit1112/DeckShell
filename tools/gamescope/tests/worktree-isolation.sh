#!/usr/bin/env bash
# Exercise the actual build and Desktop selection paths with a small Rust workspace.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd -P)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
REAL_CARGO="$(command -v cargo)"
export REAL_CARGO
mkdir -p "$TMP/bin" "$TMP/home" "$TMP/a/tools/gamescope" "$TMP/a/app/src" "$TMP/a/dep/src"
cp "$ROOT/tools/gamescope/"{build.sh,desktop-build-install.sh} "$TMP/a/tools/gamescope/"
cat > "$TMP/bin/cargo" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = zigbuild ]; then
    shift
    exec "$REAL_CARGO" build "$@" # strip the glibc suffix, retaining the target directory layout
fi
exec "$REAL_CARGO" "$@"
SH
# The native target is enough for this cache regression; no Zig toolchain is required.
python3 - "$TMP/bin/cargo" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace('    exec "$REAL_CARGO" build "$@" # strip the glibc suffix, retaining the target directory layout', '''    args=()
    for arg in "$@"; do args+=("${arg/x86_64-unknown-linux-gnu.2.34/x86_64-unknown-linux-gnu}"); done
    exec "$REAL_CARGO" build "${args[@]}"''')
open(p, 'w').write(s)
PY
printf '#!/bin/sh\nexit 0\n' > "$TMP/bin/zig"
printf '#!/bin/sh\nexit 0\n' > "$TMP/bin/cargo-zigbuild"
printf '#!/bin/sh\nexit 0\n' > "$TMP/a/tools/gamescope/check-glibc.sh"
cat > "$TMP/bin/ssh" <<'SH'
#!/bin/sh
exit 0
SH
cat > "$TMP/a/tools/gamescope/install-package.sh" <<'SH'
#!/bin/sh
[ "$1" = --deck ]
"$3" > "$INSTALL_RESULT"
SH
chmod +x "$TMP/bin/"* "$TMP/a/tools/gamescope/"*.sh
cat > "$TMP/a/Cargo.toml" <<'TOML'
[workspace]
members = ["app", "dep"]
resolver = "2"
TOML
cat > "$TMP/a/app/Cargo.toml" <<'TOML'
[package]
name = "deckshell"
version = "0.1.0"
edition = "2021"
[dependencies]
dep = { path = "../dep" }
TOML
printf 'fn main() { println!("{}", dep::value()); }\n' > "$TMP/a/app/src/main.rs"
cat > "$TMP/a/dep/Cargo.toml" <<'TOML'
[package]
name = "dep"
version = "0.1.0"
edition = "2021"
TOML
printf 'pub fn value() -> &\x27static str { "A" }\n' > "$TMP/a/dep/src/lib.rs"
git -C "$TMP/a" init -q
git -C "$TMP/a" add .
git -C "$TMP/a" -c user.name=test -c user.email=test@example.invalid commit -qm fixture
git -C "$TMP/a" worktree add -q --detach "$TMP/b"
printf 'pub fn value() -> &\x27static str { "B" }\n' > "$TMP/b/dep/src/lib.rs"
# Reproduce checkout timestamps that can fool a shared Cargo cache.
touch -r "$TMP/a/dep/src/lib.rs" "$TMP/b/dep/src/lib.rs"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$TMP/home" PATH="$TMP/bin:$PATH" CARGO_TARGET_DIR="$TMP/cache" INSTALL_RESULT="$TMP/result"
for tree in a b a; do
    REPO="$TMP/$tree" "$TMP/$tree/tools/gamescope/desktop-build-install.sh"
    expected="${tree^^}"
    [ "$(cat "$INSTALL_RESULT")" = "$expected" ] || { echo "wrong binary for $tree" >&2; exit 1; }
done
A="$($TMP/a/tools/gamescope/build.sh --bin-path)"
B="$($TMP/b/tools/gamescope/build.sh --bin-path)"
[ "$A" != "$B" ]
if "$TMP/a/tools/gamescope/build.sh" --target-dir "$TMP/other"; then
    echo 'unsafe Cargo argument accepted' >&2; exit 1
fi
echo 'worktree isolation passed'
