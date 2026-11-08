#!/usr/bin/env bash
# IFAC test: Interface Access Codes on a shared RoE segment.
#
# Three nodes on one segment (a Linux bridge with three ports, no IP). A and
# B share an IFAC network; C uses another key. All use the largest IFAC
# (ifac_size = 512 bits, 64 bytes), the case RoeInterface reserves room for.
#
#   1. B reaches A.
#   2. C cannot reach A: its packets carry the wrong access code.
#   3. A file transfer B -> A works with full-size packets plus the 64-byte
#      IFAC, without any packet dropped for exceeding the frame size.
#
# Usage: [SIZE_MB=n] [KEEP=1] tests/ifac.sh
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

# One shared segment: each node's veth end is plugged into a bridge.
ip link set lo up
ip link add br0 type bridge
ip link set br0 addrgenmode none
ip link set br0 up
for n in a b c; do
    ip link add "${n}0" type veth peer name "${n}1"
    for i in "${n}0" "${n}1"; do ip link set "$i" addrgenmode none; done
    ip link set "${n}1" master br0
    ip link set "${n}1" up
    ip link set "${n}0" up
done

write_config() { # name port transport device netkey
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
    verbose = Yes
    network_name = roe-test
    passphrase = $5
    ifac_size = 512
EOF
}

write_config a 47800 Yes a0 shared-secret
write_config b 47810 No  b0 shared-secret
write_config c 47820 No  c0 another-secret

for n in a b c; do
    "$VENV/rnsd" --config "$WORK/$n" > "$WORK/$n.log" 2>&1 &
    PID[$n]=$!
done

PA=""
for _ in $(seq 300); do
    PA="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/a.log" || true)"
    [[ -n "$PA" ]] && break
    sleep 0.2
done
[[ -n "$PA" ]] || { tail -n 20 "$WORK/a.log"; fail "node A did not start"; }
for n in b c; do
    for _ in $(seq 300); do
        grep -q 'Started rnsd' "$WORK/$n.log" && break
        sleep 0.2
    done
done
sleep 2

probe() { # from-node
    "$VENV/rnprobe" --config "$WORK/$1" -n 3 -t 10 rnstransport.probe "$PA" 2>&1 \
        | tr '\r' '\n' | grep -v '^\s*$' | tail -n 1 || true
}

echo "== 1. B, on the same IFAC network, reaches A"
out="$(probe b)"
echo "  $out"
[[ "$out" == *"packet loss 0.0%"* ]] || fail "B could not reach A"

echo "== 2. C, with another IFAC key, cannot reach A"
out="$(probe c)"
echo "  $out"
[[ "$out" != *"packet loss 0.0%"* ]] || fail "C reached A despite the wrong IFAC key"

echo "== 3. B sends $SIZE_MB MB to A with a 64-byte IFAC"
mkdir -p "$WORK/received"
"$VENV/rncp" --config "$WORK/a" -v -l -n -s "$WORK/received" > "$WORK/listen.log" 2>&1 &
PID[listen]=$!
DST=""
for _ in $(seq 300); do
    DST="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen.log" || true)"
    [[ -n "$DST" ]] && break
    sleep 0.2
done
[[ -n "$DST" ]] || { cat "$WORK/listen.log"; fail "listener on A did not start"; }
sleep 2
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/test.bin"
timeout 120 "$VENV/rncp" --config "$WORK/b" -w 30 "$WORK/test.bin" "$DST" > "$WORK/send.log" 2>&1 \
    || { tr '\r' '\n' < "$WORK/send.log" | tail -n 5; fail "transfer failed"; }
cmp -s "$WORK/test.bin" "$WORK/received/test.bin" || fail "received file differs"
echo "  received file is identical"

max="$(grep -ohP '\[roe\] tx \K\d+' "$WORK/b.log" | sort -n | tail -n 1)"
echo "  largest packet sent by B, IFAC included: $max bytes"
(( max > 1432 )) || fail "expected packets above the 1432-byte hardware MTU once the IFAC is added"
(( max <= 1496 )) || fail "packet of $max bytes does not fit a frame"
if grep -q 'dropped outbound' "$WORK/a.log" "$WORK/b.log"; then
    fail "packets were dropped"
fi

echo
echo "PASS"
