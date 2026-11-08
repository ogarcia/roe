#!/usr/bin/env bash
# Multi-hop test: two clients on separate Ethernet segments, joined only by a
# transport node, all talking through RtpInterface + rtp with no IP.
#
#   A ──veth── T ──veth── B
#
# A and B are plain clients (transport disabled). T is a transport node with
# one RtpInterface per segment. The test checks that A finds a 2-hop path to
# a destination on B via T, transfers a file across, and that A cannot reach
# B any more once T is stopped.
#
# Reticulum only answers probes on transport nodes, so the target on B is
# the destination of an `rncp --listen` running there.
#
# Usage: [SIZE_MB=n] [KEEP=1] [RTP_VERBOSE=1] tests/multihop.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${RTP_IN_NS:-}" != 1 ]]; then
    cargo build --quiet --release --manifest-path "$ROOT/Cargo.toml"
    exec env RTP_IN_NS=1 unshare --user --map-root-user --net -- "$0" "$@"
fi

BIN="$ROOT/target/release/rtp"
VENV="$ROOT/.venv/bin"
export PYTHONUNBUFFERED=1
WORK="$(mktemp -d)"
SIZE_MB="${SIZE_MB:-10}"
declare -A PID

cleanup() {
    for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

# Two separate L2 segments, without any IP address.
ip link set lo up
ip link add a0 type veth peer name t0
ip link add t1 type veth peer name b0
for i in a0 t0 t1 b0; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done

rtp_iface() { # section-name device
    cat <<EOF
  [[$1]]
    type = RtpInterface
    interface_enabled = Yes
    device = $2
    binary = $BIN
    respawn_delay = 2
    verbose = $([[ "${RTP_VERBOSE:-}" == 1 ]] && echo Yes || echo No)
EOF
}

write_config() { # name port transport iface-blocks...
    local name=$1 port=$2 transport=$3
    shift 3
    mkdir -p "$WORK/$name/interfaces"
    cp "$ROOT/dist-files/RtpInterface.py" "$WORK/$name/interfaces/"
    {
        cat <<EOF
[reticulum]
  enable_transport = $transport
  share_instance = Yes
  instance_name = rtp-$name
  shared_instance_port = $port
  instance_control_port = $((port + 1))
  respond_to_probes = Yes

[logging]
  loglevel = 4

[interfaces]
EOF
        printf '%s\n' "$@"
    } > "$WORK/$name/config"
}

write_config a 47500 No  "$(rtp_iface Ethernet a0)"
write_config t 47510 Yes "$(rtp_iface SegmentA t0)" "$(rtp_iface SegmentB t1)"
write_config b 47520 No  "$(rtp_iface Ethernet b0)"

start_node() { # name
    "$VENV/rnsd" --config "$WORK/$1" > "$WORK/$1.log" 2>&1 &
    PID[$1]=$!
}

wait_log() { # name pattern
    for _ in $(seq 50); do
        grep -q "$2" "$WORK/$1.log" 2>/dev/null && return 0
        sleep 0.2
    done
    fail "node $1: '$2' never appeared in its log"
}

probe_hash() { # name
    local h=""
    for _ in $(seq 50); do
        h="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/$1.log" || true)"
        [[ -n "$h" ]] && break
        sleep 0.2
    done
    [[ -n "$h" ]] || fail "node $1 did not start"
    echo "$h"
}

for n in t a b; do start_node "$n"; done
PT="$(probe_hash t)"
T_ID="$(grep -oP 'probe requests on <rnstransport\.probe\.\K[0-9a-f]{32}' "$WORK/t.log")"
wait_log a 'Started rnsd'
wait_log b 'Started rnsd'
sleep 2
echo "T transport identity: <$T_ID>"

echo
echo "== 1. A probes T (one hop, segment A)"
out="$("$VENV/rnprobe" --config "$WORK/a" -n 3 -t 15 rnstransport.probe "$PT" 2>&1 | tr '\r' '\n')"
echo "$out" | grep 'packet loss'
echo "$out" | grep -q 'over 1 hop' || fail "expected T 1 hop away"
echo "$out" | grep -q 'packet loss 0.0%' || fail "probes to T were lost"

mkdir -p "$WORK/received"
"$VENV/rncp" --config "$WORK/b" -v -l -n -s "$WORK/received" > "$WORK/listen.log" 2>&1 &
PID[listen]=$!
DST=""
for _ in $(seq 50); do
    DST="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen.log" || true)"
    [[ -n "$DST" ]] && break
    sleep 0.2
done
[[ -n "$DST" ]] || fail "listener on B did not start"
sleep 2

echo
echo "== 2. A finds B's listener <$DST> 2 hops away via T"
out="$("$VENV/rnpath" --config "$WORK/a" -w 15 "$DST" 2>&1 | tr '\r' '\n' | grep -v '^\s*$' | tail -n 1)"
echo "$out"
[[ "$out" == *"is 2 hops away via <$T_ID>"* ]] || fail "expected a 2-hop path via T"

echo
echo "== 3. A sends $SIZE_MB MB to B through T"
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/test.bin"
start=$(date +%s%N)
timeout 180 "$VENV/rncp" --config "$WORK/a" -w 30 "$WORK/test.bin" "$DST" > "$WORK/send.log" 2>&1 \
    || { tr '\r' '\n' < "$WORK/send.log" | tail -n 5; fail "transfer failed"; }
end=$(date +%s%N)
tr '\r' '\n' < "$WORK/send.log" | grep 'Transfer complete' || true
echo "$SIZE_MB MB sent in $(( (end - start) / 1000000 )) ms"
cmp -s "$WORK/test.bin" "$WORK/received/test.bin" || fail "received file differs"
echo "received file is identical"

echo
echo "== 4. With T stopped, A cannot reach B"
kill "${PID[t]}"; wait "${PID[t]}" 2>/dev/null || true
unset 'PID[t]'
# A may still hold the cached path, so test the data path itself: a link
# to B cannot be established without T.
echo "should not arrive" > "$WORK/small.txt"
if timeout 60 "$VENV/rncp" --config "$WORK/a" -w 10 "$WORK/small.txt" "$DST" > "$WORK/send2.log" 2>&1; then
    fail "A still sends to B without T"
fi
tr '\r' '\n' < "$WORK/send2.log" | grep -v '^\s*$' | tail -n 1
[[ ! -e "$WORK/received/small.txt" ]] || fail "file reached B without T"
echo "transfer fails without T, as expected"

echo
echo "PASS"
