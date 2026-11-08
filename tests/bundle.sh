#!/usr/bin/env bash
# End-to-end test of a packaged bundle, as if unpacked on two machines.
#
# Unpacks the bundle twice, runs each node in its own network namespace
# (joined only by a veth pair, without IP), probes one from the other and
# transfers a 1 MB file. Needs Internet for 'rtp-node setup'.
#
# Usage: [MODE=udp] tests/bundle.sh [dist/rtp-<version>-x86_64.tar.gz]
#
# MODE=udp runs the same test over Reticulum's AutoInterface (UDP over IPv6
# link-local) instead of rtp, as a performance baseline.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${RTP_IN_NS:-}" != 1 ]]; then
    tarball="${1:-}"
    if [[ -z "$tarball" ]]; then
        "$ROOT/scripts/pack.sh" x86_64 >/dev/null
        tarball="$(ls -t "$ROOT"/dist/rtp-*-x86_64.tar.gz | head -n 1)"
    fi
    tarball="$(readlink -f "$tarball")"
    work="$(mktemp -d)"
    for m in a b; do
        mkdir "$work/$m"
        tar -C "$work/$m" -xzf "$tarball"
        "$work/$m"/rtp-*/rtp-node setup >/dev/null
    done
    status=0
    env RTP_IN_NS=1 unshare --user --map-root-user --net -- "$0" "$work" || status=$?
    if [[ "${KEEP:-}" == 1 ]]; then echo "work dir kept in $work"; else rm -rf "$work"; fi
    exit "$status"
fi

WORK="$1"
MODE="${MODE:-rtp}"
START_ARGS=()
[[ "$MODE" == udp ]] && START_ARGS=(--udp)
A="$(echo "$WORK"/a/rtp-*)"
B="$(echo "$WORK"/b/rtp-*)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
}
trap cleanup EXIT

# Node B lives in a nested network namespace, held open by a sleeper.
unshare --net sleep infinity & NS=$!; PIDS+=("$NS")
sleep 0.3
inB() { nsenter -t "$NS" -n "$@"; }

ip link add veth0 type veth peer name veth1
ip link set veth1 netns "$NS"
if [[ "$MODE" == rtp ]]; then
    # No IP at all, not even IPv6 link-local.
    ip link set veth0 addrgenmode none
    inB ip link set veth1 addrgenmode none
fi
ip link set veth0 up
inB ip link set veth1 up
# AutoInterface needs the link-local addresses to finish DAD first.
[[ "$MODE" == udp ]] && sleep 3

"$A/rtp-node" start "${START_ARGS[@]}" veth0 > "$WORK/a.log" 2>&1 & PIDS+=($!)
inB "$B/rtp-node" start "${START_ARGS[@]}" veth1 > "$WORK/b.log" 2>&1 & PIDS+=($!)

ida=""
for _ in $(seq 50); do
    ida="$("$A/rtp-node" id 2>/dev/null || true)"
    [[ -n "$ida" ]] && break
    sleep 0.2
done
[[ -n "$ida" ]] || { echo "node A did not start"; cat "$WORK/a.log"; exit 1; }
sleep 2
echo "node A: $ida"
inB "$B/rtp-node" probe "$ida" 3 | tail -n 1

"$A/rtp-node" listen > "$WORK/listen.log" 2>&1 & PIDS+=($!)
dst=""
for _ in $(seq 50); do
    dst="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen.log" || true)"
    [[ -n "$dst" ]] && break
    sleep 0.2
done
[[ -n "$dst" ]] || { echo "listener did not start"; cat "$WORK/listen.log"; exit 1; }
sleep 2

SIZE_MB="${SIZE_MB:-1}"
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/test.bin"
start=$(date +%s%N)
inB timeout 120 "$B/rtp-node" send "$WORK/test.bin" "$dst" > "$WORK/send.log" 2>&1
end=$(date +%s%N)
tr '\r' '\n' < "$WORK/send.log" | grep 'Transfer complete' || true
echo "[$MODE] $SIZE_MB MB sent in $(( (end - start) / 1000000 )) ms"
cmp "$WORK/test.bin" "$A/received/test.bin"
echo "received file is identical"
