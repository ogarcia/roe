#!/usr/bin/env bash
# Build self-contained test bundles: dist/roe-<version>-<arch>.tar.gz
#
# Each bundle holds a static (musl) roe binary, RoeInterface.py, the
# roe-node helper, READMEs in English and Spanish, and the license.
# Usage: scripts/pack.sh [x86_64|aarch64]...   (default: both)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
ARCHES=("$@")
[[ ${#ARCHES[@]} -gt 0 ]] || ARCHES=(x86_64 aarch64)

HOST_ARCH="$(uname -m)"
[[ "$HOST_ARCH" == arm64 ]] && HOST_ARCH=aarch64

mkdir -p dist
for arch in "${ARCHES[@]}"; do
    target="$arch-unknown-linux-musl"
    if [[ "$arch" != "$HOST_ARCH" ]]; then
        # Cross build: rust-lld links static musl binaries for any
        # architecture, with no cross gcc needed. Native builds use the
        # default linker.
        var="CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER"
        export "$var=rust-lld"
    fi
    cargo build --quiet --release --target "$target"
    name="roe-$VERSION-$arch"
    stage="$(mktemp -d)"
    mkdir "$stage/$name"
    install -m 755 "target/$target/release/roe" "$stage/$name/roe"
    install -m 755 dist-files/roe-node "$stage/$name/roe-node"
    install -m 644 dist-files/README.md "$stage/$name/README.md"
    install -m 644 dist-files/README.es.md "$stage/$name/README.es.md"
    install -m 644 dist-files/RoeInterface.py "$stage/$name/RoeInterface.py"
    install -m 644 LICENSE "$stage/$name/LICENSE"
    tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
    rm -rf "$stage"
    echo "dist/$name.tar.gz"
done
