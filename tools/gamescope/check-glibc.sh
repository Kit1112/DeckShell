#!/usr/bin/env bash
# check-glibc.sh BIN [MAX]  - highest GLIBC symbol version BIN needs; fails if > MAX (default 2.34) or the host cannot load it.
# Run in the build container:  tools/gamescope/build.sh   (or: check-glibc.sh target/release/deckshell 2.43 for a plain build)
set -eu
HX=""; command -v distrobox-host-exec >/dev/null && HX=distrobox-host-exec   # empty when already on the host
BIN="${1:?binary}"; MAX="${2:-2.34}"
need="$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1 | cut -d_ -f2)"
host="$($HX ldd --version 2>/dev/null | head -n1 | grep -o '[0-9]\+\.[0-9]\+$' || true)"
echo "max GLIBC symbol version: GLIBC_$need   (limit $MAX; host glibc ${host:-unknown})"
echo "all versions: $(objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tr '\n' ' ')"
echo "libstdc++/gcc: $(objdump -T "$BIN" | grep -oE 'GLIBCXX_[0-9.]+|CXXABI_[0-9.]+' | sort -uV | tail -n2 | tr '\n' ' ')"
[ "$(printf '%s\n%s\n' "$need" "$MAX" | sort -V | tail -n1)" != "$MAX" ] && { echo "FAIL: needs GLIBC_$need > $MAX"; exit 1; }
missing="$($HX ldd "$(readlink -f "$BIN")" 2>&1 | grep -E 'not found|version' || true)"
[ -n "$missing" ] && { echo "HOST CANNOT LOAD:"; echo "$missing"; exit 1; }
echo OK
