#!/usr/bin/env bash
# Bridge test: a transport node forwarding between RoE and another
# Reticulum interface.
#
#   A ──RoE (veth, no IP)── T ──TCP (loopback)── B
#
# A only has an RoeInterface, B only a TCPClientInterface, and T, a
# transport node, has both. The test checks that A finds a destination on B
# 2 hops away via T, and that files cross the bridge in both directions.
#
# Usage: [SIZE_MB=n] [KEEP=1] tests/bridge.sh
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
TCP_PORT=4250
declare -A PID

cleanup() {
    for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

# The RoE segment, without IP; the TCP side uses the namespace's loopback.
ip link set lo up
ip link add a0 type veth peer name t0
for i in a0 t0; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done

write_config() { # name port transport interfaces-block
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
$4
EOF
}

write_config a 47900 No "  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = a0
    binary = $BIN"
write_config t 47910 Yes "  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = t0
    binary = $BIN
  [[TCP Server]]
    type = TCPServerInterface
    interface_enabled = Yes
    listen_ip = 127.0.0.1
    listen_port = $TCP_PORT"
write_config b 47920 No "  [[TCP to T]]
    type = TCPClientInterface
    interface_enabled = Yes
    target_host = 127.0.0.1
    target_port = $TCP_PORT"

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

# T first, so that B's TCP client finds its server listening.
for n in t a b; do start_node "$n"; done
T_ID="$(grep -oP 'probe requests on <rnstransport\.probe\.\K[0-9a-f]{32}' "$WORK/t.log")"
echo "T transport identity: <$T_ID>"
sleep 2

listen() { # node -> prints the listener destination
    mkdir -p "$WORK/received-$1"
    "$VENV/rncp" --config "$WORK/$1" -v -l -n -s "$WORK/received-$1" > "$WORK/listen-$1.log" 2>&1 &
    PID[listen-$1]=$!
    local dst=""
    for _ in $(seq 300); do
        dst="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen-$1.log" || true)"
        [[ -n "$dst" ]] && break
        sleep 0.2
    done
    [[ -n "$dst" ]] || { cat "$WORK/listen-$1.log" >&2; fail "listener on $1 did not start"; }
    echo "$dst"
}

send() { # from to-destination to-node file
    timeout 120 "$VENV/rncp" --config "$WORK/$1" -w 30 "$WORK/$4" "$2" > "$WORK/send-$4.log" 2>&1 \
        || { tr '\r' '\n' < "$WORK/send-$4.log" | tail -n 5; fail "transfer of $4 failed"; }
    cmp -s "$WORK/$4" "$WORK/received-$3/$4" || fail "$4 arrived corrupted"
    echo "  $4 ($SIZE_MB MB) arrived identical"
}

DB="$(listen b)"
DA="$(listen a)"
sleep 2

echo "== 1. A (RoE only) finds B's listener (TCP only) 2 hops away via T"
out="$("$VENV/rnpath" --config "$WORK/a" -w 15 "$DB" 2>&1 | tr '\r' '\n' | grep -v '^\s*$' | tail -n 1 || true)"
echo "  $out"
[[ "$out" == *"is 2 hops away via <$T_ID>"* ]] || fail "expected a 2-hop path via T"

echo "== 2. Files cross the bridge both ways"
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/a-to-b.bin"
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/b-to-a.bin"
send a "$DB" b a-to-b.bin
send b "$DA" a b-to-a.bin

echo
echo "PASS"
