#!/usr/bin/env bash
# End-to-end test: two rnsd instances talking only through `rtp` over a veth
# pair, inside an unprivileged user + network namespace with no IP at all.
#
# Usage: tests/netns.sh            (from the repository root)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${RTP_IN_NS:-}" != 1 ]]; then
    cargo build --quiet --manifest-path "$ROOT/Cargo.toml"
    exec env RTP_IN_NS=1 unshare --user --map-root-user --net -- "$0" "$@"
fi

BIN="$ROOT/target/debug/rtp"
VENV="$ROOT/.venv/bin"
export PYTHONUNBUFFERED=1
WORK="$(mktemp -d)"
PIDS=()

cleanup() {
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

# Link layer only: no IPv4, no IPv6 (not even link-local).
ip link set lo up
ip link add veth0 type veth peer name veth1
for i in veth0 veth1; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done

write_config() { # name iface port transport
    mkdir -p "$WORK/$1"
    cat > "$WORK/$1/config" <<EOF
[reticulum]
  enable_transport = $4
  share_instance = Yes
  instance_name = rtp-$1
  shared_instance_port = $3
  instance_control_port = $(($3 + 1))
  respond_to_probes = Yes

[logging]
  loglevel = 5

[interfaces]
  [[Ethernet]]
    type = PipeInterface
    interface_enabled = Yes
    command = $BIN -v $2
    respawn_delay = 2
EOF
}

write_config a veth0 47428 Yes
write_config b veth1 47438 No

"$VENV/rnsd" --config "$WORK/a" > "$WORK/a.log" 2>&1 & PIDS+=($!)
"$VENV/rnsd" --config "$WORK/b" > "$WORK/b.log" 2>&1 & PIDS+=($!)

# Wait for node A to publish its probe destination.
probe=""
for _ in $(seq 50); do
    probe="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/a.log" || true)"
    [[ -n "$probe" ]] && break
    sleep 0.2
done
if [[ -z "$probe" ]]; then
    echo "node A never announced its probe destination"; cat "$WORK/a.log"; exit 1
fi
echo "node A probe destination: $probe"

echo "addresses in the namespace (veths must have none):"
ip -brief addr show

status=0
"$VENV/rnprobe" --config "$WORK/b" -n 3 -t 15 rnstransport.probe "$probe" || status=$?

echo "--- rtp on A ---"; grep '\[rtp\]' "$WORK/a.log" | head -n 8 || true
echo "--- rtp on B ---"; grep '\[rtp\]' "$WORK/b.log" | head -n 8 || true
exit "$status"
