#!/usr/bin/env bash
# Container test: builds the image and runs two RoE nodes in containers on
# a bridge network, which joins them at layer 2.
#
# Node A is a transport node and answers probes; node B probes it. This
# checks the image end to end: the static roe binary, RoeInterface, the
# entrypoint's generated config, and CAP_NET_RAW granted with --cap-add.
#
# Needs root for the container engine (rootless networking gives no layer 2
# access). Usage: [ENGINE="sudo podman"] [KEEP=1] tests/container.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
read -r -a ENGINE <<< "${ENGINE:-podman}"
IMAGE=roe-test
NET=roe-test-net

fail() { echo "FAIL: $*"; exit 1; }

cleanup() {
    if [[ "${KEEP:-}" != 1 ]]; then
        "${ENGINE[@]}" rm -f roe-a roe-b >/dev/null 2>&1 || true
        "${ENGINE[@]}" network rm "$NET" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "== Building the image"
"${ENGINE[@]}" build -q -t "$IMAGE" -f "$ROOT/Containerfile" "$ROOT" >/dev/null
"${ENGINE[@]}" run --rm --entrypoint roe "$IMAGE" --help 2>&1 | head -n 1
"${ENGINE[@]}" run --rm --entrypoint rnsd "$IMAGE" --version

cleanup
"${ENGINE[@]}" network create "$NET" >/dev/null

run_node() { # name transport
    "${ENGINE[@]}" run -d --name "roe-$1" --network "$NET" --cap-add NET_RAW \
        -e ROE_DEVICE=eth0 -e ROE_TRANSPORT="$2" "$IMAGE" >/dev/null
}
run_node a Yes
run_node b No

wait_log() { # container pattern
    for _ in $(seq 300); do
        "${ENGINE[@]}" logs "$1" 2>&1 | grep -q "$2" && return 0
        sleep 0.2
    done
    "${ENGINE[@]}" logs "$1" 2>&1 | tail -n 20
    fail "$1: '$2' never appeared in its log"
}

echo "== Starting two nodes"
wait_log roe-a 'Started rnsd'
wait_log roe-b 'Started rnsd'
for c in roe-a roe-b; do
    "${ENGINE[@]}" logs "$c" 2>&1 | grep '\[roe\] up on' | sed "s/^/  $c: /"
done
PA="$("${ENGINE[@]}" logs roe-a 2>&1 | grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}')"
sleep 2

echo "== B probes A over RoE"
out="$("${ENGINE[@]}" exec roe-b rnprobe -n 3 -t 15 rnstransport.probe "$PA" 2>&1 \
    | tr '\r' '\n' | grep -v '^\s*$' | tail -n 1 || true)"
echo "  $out"
[[ "$out" == *"packet loss 0.0%"* ]] || {
    "${ENGINE[@]}" logs roe-b 2>&1 | tail -n 20
    fail "B could not reach A"
}

echo
echo "PASS"
