#!/usr/bin/env bash
# Build self-contained test bundles: dist/rtp-<version>-<arch>.tar.gz
#
# Each bundle holds a static (musl) rtp binary, the rtp-node helper and a
# Spanish README. Usage: scripts/pack.sh [x86_64|aarch64]...   (default: both)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
ARCHES=("$@")
[[ ${#ARCHES[@]} -gt 0 ]] || ARCHES=(x86_64 aarch64)

# rust-lld links static musl binaries for any arch, no cross gcc needed.
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld

mkdir -p dist
for arch in "${ARCHES[@]}"; do
    target="$arch-unknown-linux-musl"
    cargo build --quiet --release --target "$target"
    name="rtp-$VERSION-$arch"
    stage="$(mktemp -d)"
    mkdir "$stage/$name"
    install -m 755 "target/$target/release/rtp" "$stage/$name/rtp"
    install -m 755 dist-files/rtp-node "$stage/$name/rtp-node"
    install -m 644 dist-files/README.es.md "$stage/$name/README.es.md"
    install -m 644 dist-files/RtpInterface.py "$stage/$name/RtpInterface.py"
    install -m 644 LICENSE "$stage/$name/LICENSE"
    tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
    rm -rf "$stage"
    echo "dist/$name.tar.gz"
done
