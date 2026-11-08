#!/usr/bin/env bash
# Runs the bundle test with the aarch64 build under qemu user-mode emulation.
#
# Repacks the aarch64 bundle so that `roe` is a small wrapper running the
# real ARM binary through qemu-aarch64-static, then hands it to
# tests/bundle.sh. Everything else in the bundle (roe-node, RoeInterface)
# is used unchanged.
#
# qemu user mode cannot set SOL_PACKET socket options, so roe runs without
# its multicast group membership here (it warns about it). That is harmless
# on a veth pair, which delivers group frames anyway.
#
# Usage: [MODE=roe|pipe] [SIZE_MB=n] tests/qemu.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
QEMU="$(command -v qemu-aarch64-static || command -v qemu-aarch64 || true)"
[[ -n "$QEMU" ]] || { echo "qemu-aarch64(-static) not found"; exit 1; }
[[ "${MODE:-roe}" != udp ]] || { echo "MODE=udp does not run roe"; exit 2; }

"$ROOT/scripts/pack.sh" aarch64 >/dev/null
src="$(ls -t "$ROOT"/dist/roe-*-aarch64.tar.gz | head -n 1)"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
tar -C "$stage" -xzf "$src"
dir="$(echo "$stage"/roe-*-aarch64)"
mv "$dir/roe" "$dir/roe.aarch64"
cat > "$dir/roe" <<EOF
#!/bin/sh
exec "$QEMU" "\$(dirname "\$0")/roe.aarch64" "\$@"
EOF
chmod 755 "$dir/roe"
tar -C "$stage" -czf "$stage/qemu-bundle.tar.gz" "$(basename "$dir")"

file -b "$dir/roe.aarch64" | cut -d, -f1-2
"$ROOT/tests/bundle.sh" "$stage/qemu-bundle.tar.gz"
