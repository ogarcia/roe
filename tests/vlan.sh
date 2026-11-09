#!/usr/bin/env bash
# VLAN test: RoE on 802.1Q VLAN interfaces, routed between VLANs by a
# transport node with one RoeInterface per VLAN ("router on a stick").
#
#   A (t0.10) ─┐                       ┌─ t1.10 ─┐
#              ├── t0 ═══ trunk ═══ t1 ┤         ├─ T, transport node
#   B (t0.20) ─┘                       └─ t1.20 ─┘
#
# One veth pair is the trunk, carrying VLANs 10 and 20 tagged; the kernel
# adds and strips the tags, RoE just uses the VLAN interfaces. A and B are
# plain clients on different VLANs. The test checks that:
#
#   1. A reaches T over VLAN 10;
#   2. A finds a destination on B, on VLAN 20, 2 hops away via T;
#   3. a file crosses from A to B through T;
#   4. with T stopped, A cannot reach B: the VLANs really are separate.
#
# Usage: [SIZE_MB=n] [KEEP=1] tests/vlan.sh
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

# The trunk, and a VLAN interface for each VLAN on each end. No IP at all.
ip link set lo up
ip link add t0 type veth peer name t1
for i in t0 t1; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done
for end in t0 t1; do
    for vid in 10 20; do
        ip link add link "$end" name "$end.$vid" type vlan id "$vid"
        ip link set "$end.$vid" addrgenmode none
        ip link set "$end.$vid" up
    done
done

roe_iface() { # section-name device
    cat <<EOF
  [[$1]]
    type = RoeInterface
    interface_enabled = Yes
    device = $2
    binary = $BIN
    respawn_delay = 2
EOF
}

write_config() { # name port transport iface-blocks...
    local name=$1 port=$2 transport=$3
    shift 3
    mkdir -p "$WORK/$name/interfaces"
    cp "$ROOT/dist-files/RoeInterface.py" "$WORK/$name/interfaces/"
    {
        cat <<EOF
[reticulum]
  enable_transport = $transport
  share_instance = Yes
  instance_name = roe-$name
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

write_config a 48200 No  "$(roe_iface VLAN10 t0.10)"
write_config b 48210 No  "$(roe_iface VLAN20 t0.20)"
write_config t 48220 Yes "$(roe_iface VLAN10 t1.10)" "$(roe_iface VLAN20 t1.20)"

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

for n in t a b; do start_node "$n"; done
PT="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/t.log")"
T_ID="$(grep -oP 'probe requests on <rnstransport\.probe\.\K[0-9a-f]{32}' "$WORK/t.log")"
echo "T transport identity: <$T_ID>"
ip -d link show t0.10 | grep -o 'vlan protocol 802.1Q id [0-9]*' | sed 's/^/t0.10: /'
sleep 2

echo
echo "== 1. A reaches T over VLAN 10"
out="$("$VENV/rnprobe" --config "$WORK/a" -n 3 -t 15 rnstransport.probe "$PT" 2>&1 | tr '\r' '\n')"
echo "$out" | grep 'packet loss'
echo "$out" | grep -q 'over 1 hop' || fail "expected T 1 hop away"
echo "$out" | grep -q 'packet loss 0.0%' || fail "probes to T were lost"

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

echo
echo "== 2. A (VLAN 10) finds B's listener (VLAN 20) 2 hops away via T"
out="$("$VENV/rnpath" --config "$WORK/a" -w 15 "$DST" 2>&1 | tr '\r' '\n' | grep -v '^\s*$' | tail -n 1 || true)"
echo "$out"
[[ "$out" == *"is 2 hops away via <$T_ID>"* ]] || fail "expected a 2-hop path via T"

echo
echo "== 3. A sends $SIZE_MB MB to B through T"
head -c "$((SIZE_MB * 1000000))" /dev/urandom > "$WORK/test.bin"
timeout 120 "$VENV/rncp" --config "$WORK/a" -w 30 "$WORK/test.bin" "$DST" > "$WORK/send.log" 2>&1 \
    || { tr '\r' '\n' < "$WORK/send.log" | tail -n 5; fail "transfer failed"; }
cmp -s "$WORK/test.bin" "$WORK/received/test.bin" || fail "received file differs"
echo "received file is identical"
if grep -q 'interface MTU limits' "$WORK/a.log" "$WORK/b.log" "$WORK/t.log"; then
    fail "a VLAN interface reduced the MTU"
fi

echo
echo "== 4. With T stopped, A cannot reach B"
kill "${PID[t]}"; wait "${PID[t]}" 2>/dev/null || true
unset 'PID[t]'
echo "should not arrive" > "$WORK/small.txt"
if timeout 60 "$VENV/rncp" --config "$WORK/a" -w 10 "$WORK/small.txt" "$DST" > "$WORK/send2.log" 2>&1; then
    fail "A still reaches B without T: VLANs 10 and 20 are not separate"
fi
[[ ! -e "$WORK/received/small.txt" ]] || fail "file reached B without T"
echo "no link to B without T: the VLANs are separate segments"

echo
echo "PASS"
