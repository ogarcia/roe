#!/usr/bin/env bash
# MTU test: roe and RoeInterface on interfaces with an MTU below 1500.
#
#   1. Nodes start on a veth pair with MTU 1000: RoeInterface lowers its
#      hardware MTU, a file transfer works, and no frame exceeds the MTU.
#   2. The MTU drops to 800 while running: roe notices, is restarted, and
#      transfers work again with the new value.
#   3. With an MTU of 400, too small for Reticulum's 500-byte packets, roe
#      refuses to start and says why.
#
# Usage: [KEEP=1] tests/mtu.sh
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
declare -A PID

cleanup() {
    for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

set_mtu() {
    for i in a0 b0; do ip link set "$i" mtu "$1"; done
}

ip link set lo up
ip link add a0 type veth peer name b0
for i in a0 b0; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done
set_mtu 1000

write_config() { # name port device
    mkdir -p "$WORK/$1/interfaces"
    cp "$ROOT/dist-files/RoeInterface.py" "$WORK/$1/interfaces/"
    cat > "$WORK/$1/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = Yes
  instance_name = roe-$1
  shared_instance_port = $2
  instance_control_port = $(($2 + 1))

[logging]
  loglevel = 4

[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = $3
    binary = $BIN
    respawn_delay = 1
    verbose = Yes
EOF
}

write_config a 47700 a0
write_config b 47710 b0
"$VENV/rnsd" --config "$WORK/a" > "$WORK/a.log" 2>&1 & PID[a]=$!
"$VENV/rnsd" --config "$WORK/b" > "$WORK/b.log" 2>&1 & PID[b]=$!

mkdir -p "$WORK/received"
"$VENV/rncp" --config "$WORK/b" -v -l -n -s "$WORK/received" > "$WORK/listen.log" 2>&1 &
PID[listen]=$!
DST=""
for _ in $(seq 300); do
    DST="$(grep -oP 'listening on <\K[0-9a-f]{32}' "$WORK/listen.log" || true)"
    [[ -n "$DST" ]] && break
    sleep 0.2
done
[[ -n "$DST" ]] || { echo "--- rncp listener log"; cat "$WORK/listen.log"; echo "--- node B log (tail)"; tail -n 20 "$WORK/b.log"; fail "listener on B did not start"; }
sleep 2

# Sends a file from A to B, retrying a few times: right after an MTU change
# the first attempt may be lost while roe restarts.
send_file() { # name size-kb
    head -c "$(($2 * 1000))" /dev/urandom > "$WORK/$1"
    for attempt in 1 2 3; do
        if timeout 60 "$VENV/rncp" --config "$WORK/a" -w 15 "$WORK/$1" "$DST" \
            > "$WORK/send-$1.log" 2>&1 && cmp -s "$WORK/$1" "$WORK/received/$1"; then
            echo "  $1 ($2 kB) transferred on attempt $attempt"
            return 0
        fi
        sleep 2
    done
    fail "could not transfer $1"
}

largest_tx() { # node
    grep -oP '\[roe\] tx \K\d+' "$WORK/$1.log" | sort -n | tail -n 1
}

echo "== 1. interface MTU 1000 from the start"
grep -h 'hardware MTU set to' "$WORK/a.log" "$WORK/b.log" | sed 's/^/  /' | head -n 2
grep -q 'hardware MTU set to 932 for an interface MTU of 1000' "$WORK/a.log" \
    || fail "RoeInterface did not adapt to MTU 1000"
send_file first.bin 500
max="$(largest_tx a)"
echo "  largest packet sent by A: $max bytes"
(( max <= 996 )) || fail "A sent a $max-byte packet on a 1000-byte MTU"
grep -q 'dropped outbound' "$WORK/a.log" "$WORK/b.log" && fail "packets were dropped"

echo "== 2. MTU drops to 800 while running"
set_mtu 800
send_file second.bin 500
grep -h 'frame too large' "$WORK/a.log" "$WORK/b.log" | sed 's/^/  /' | head -n 2
grep -q 'hardware MTU set to 732 for an interface MTU of 800' "$WORK/a.log" \
    || fail "RoeInterface did not adapt to MTU 800"
echo "  RoeInterface adapted to MTU 800"

echo "== 3. MTU too small for Reticulum"
for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
wait 2>/dev/null || true
PID=()
set_mtu 400
if out="$("$BIN" a0 </dev/null 2>&1)"; then
    fail "roe started on a 400-byte MTU"
fi
echo "$out" | sed 's/^/  /'
[[ "$out" == *"too small"* ]] || fail "roe did not explain why it refused to start"

echo
echo "PASS"
