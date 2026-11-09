#!/usr/bin/env bash
# Unicast test: packets for a known neighbor go to its MAC address, not to
# the RoE multicast group.
#
#   A (a0) ─┐
#   B (b0) ─┼── br0, a learning bridge, as a switch would be
#   C (c0) ─┘
#
# A and B are RoE nodes; c0 is a third port with nothing behind it, which
# only receives what the bridge floods: multicast and unknown unicast. The
# test checks that:
#
#   1. A reaches B by probe, and the probes and their proofs do not reach
#      c0;
#   2. a file crosses from A to B, and almost none of its frames reach c0;
#   3. with unicast = No on A, its frames reach c0 again.
#
# Usage: [SIZE_MB=n] [KEEP=1] tests/unicast.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${ROE_IN_NS:-}" != 1 ]]; then
    cargo build --quiet --release --manifest-path "$ROOT/Cargo.toml"
    exec env ROE_IN_NS=1 unshare --user --map-root-user --net -- "$0" "$@"
fi

BIN="$ROOT/target/release/roe"
VENV="$ROOT/.venv/bin"
export PYTHONUNBUFFERED=1
WORK="$(mktemp -d)"
SIZE_MB="${SIZE_MB:-2}"
declare -A PID

cleanup() {
    for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

# A bridge with three ports, without IP.
ip link set lo up
ip link add br0 type bridge
ip link set br0 addrgenmode none
ip link set br0 up
for n in a b c; do
    ip link add "${n}0" type veth peer name "${n}p"
    for i in "${n}0" "${n}p"; do
        ip link set "$i" addrgenmode none
        ip link set "$i" up
    done
    ip link set "${n}p" master br0
done

# Packet counters from /proc/net/dev, which belongs to this namespace (sysfs
# would show the host's). The colon is split off: large byte counts touch it.
counter() { sed 's/:/ /' /proc/net/dev | awk -v i="$1" -v f="$2" '$1 == i { print $f }'; }
rx() { counter "$1" 3; }
tx() { counter "$1" 11; }

write_config() { # name port transport device unicast
    mkdir -p "$WORK/$1/interfaces"
    cp "$ROOT/dist-files/RoeInterface.py" "$WORK/$1/interfaces/"
    cat > "$WORK/$1/config" <<EOF
[reticulum]
  enable_transport = $3
  share_instance = Yes
  instance_name = roe-$1
  shared_instance_port = $2
  instance_control_port = $(($2 + 1))
  respond_to_probes = Yes

[logging]
  loglevel = 4

[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = $4
    binary = $BIN
    respawn_delay = 2
    unicast = $5
EOF
}

start_node() { # name
    "$VENV/rnsd" --config "$WORK/$1" > "$WORK/$1.log" 2>&1 &
    PID[$1]=$!
    for _ in $(seq 300); do
        grep -q 'Started rnsd' "$WORK/$1.log" && return 0
        sleep 0.2
    done
    tail -n 20 "$WORK/$1.log"
    fail "node $1 did not start"
}

# B is a transport node only so that it answers probes.
write_config a 48300 No a0 Yes
write_config b 48310 Yes b0 Yes
# B starts first, so that A misses B's announce and learns the way to B from
# the response to its path request, as a node joining a running network.
start_node b
PB="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/b.log")"
sleep 3
start_node a
sleep 2

echo
echo "== 1. A reaches B, by unicast"
c_rx="$(rx c0)"
out="$("$VENV/rnprobe" --config "$WORK/a" -n 10 -t 15 rnstransport.probe "$PB" 2>&1 | tr '\r' '\n')"
echo "$out" | grep 'packet loss'
echo "$out" | grep -q 'packet loss 0.0%' || fail "probes to B were lost"
# The path request and its response go to the group; the 10 probes and
# their 10 proofs should not.
C_RX=$(($(rx c0) - c_rx))
echo "c0 received $C_RX frames"
((C_RX < 10)) || fail "probes or proofs reached c0"

mkdir -p "$WORK/received"
"$VENV/rncp" --config "$WORK/b" -v -l -n -s "$WORK/received" > "$WORK/listen.log" 2>&1 &
PID[listen]=$!
DST=""
for _ in $(seq 300); do
    DST="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen.log" || true)"
    [[ -n "$DST" ]] && break
    sleep 0.2
done
[[ -n "$DST" ]] || { cat "$WORK/listen.log"; fail "listener on B did not start"; }
sleep 2

transfer() { # name
    head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/$1"
    local a_tx b_rx c_rx
    a_tx="$(tx a0)" b_rx="$(rx b0)" c_rx="$(rx c0)"
    timeout 120 "$VENV/rncp" --config "$WORK/a" -w 30 "$WORK/$1" "$DST" > "$WORK/send-$1.log" 2>&1 \
        || { tr '\r' '\n' < "$WORK/send-$1.log" | tail -n 5; fail "transfer failed"; }
    cmp -s "$WORK/$1" "$WORK/received/$1" || fail "received file differs"
    A_TX=$(($(tx a0) - a_tx)) B_RX=$(($(rx b0) - b_rx)) C_RX=$(($(rx c0) - c_rx))
    echo "received file is identical; frames: A sent $A_TX, B received $B_RX, c0 received $C_RX"
}

echo
echo "== 2. A sends $SIZE_MB MB to B, by unicast"
transfer unicast.bin
((C_RX * 20 < B_RX)) || fail "c0 received $C_RX frames, too many for unicast"
echo "c0 received under 5% of the frames"

echo
echo "== 3. With unicast = No on A, its frames reach c0"
kill "${PID[a]}"; wait "${PID[a]}" 2>/dev/null || true
write_config a 48300 No a0 No
start_node a
sleep 2
transfer multicast.bin
((C_RX * 2 > A_TX)) || fail "c0 received only $C_RX of the $A_TX frames A sent to the group"
echo "c0 received A's frames"

echo
echo "PASS"
