#!/usr/bin/env bash
# Robustness test: two nodes over a veth pair (RoeInterface + roe, no IP),
# put through the failures a real deployment sees, checking after each one
# that traffic recovers on its own.
#
#   1. the roe helper is killed
#   2. the interface is set administratively down and up again
#   3. the carrier is lost and comes back (the peer end goes down: cable)
#   4. the interface disappears and reappears with a new ifindex (USB NIC)
#   5. rnsd dies: its roe helper must not be left behind
#
# Usage: [KEEP=1] tests/robustness.sh
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
RESPAWN=2
declare -A PID

cleanup() {
    for p in "${PID[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
    if [[ "${KEEP:-}" == 1 ]]; then echo "logs kept in $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT

fail() { echo "FAIL: $*"; exit 1; }

make_link() {
    ip link add a0 type veth peer name b0
    for i in a0 b0; do
        ip link set "$i" addrgenmode none
        ip link set "$i" up
    done
}

ip link set lo up
make_link

write_config() { # name port transport device
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
    respawn_delay = $RESPAWN
EOF
}

write_config a 47600 Yes a0
write_config b 47610 No b0

"$VENV/rnsd" --config "$WORK/a" > "$WORK/a.log" 2>&1 & PID[a]=$!
"$VENV/rnsd" --config "$WORK/b" > "$WORK/b.log" 2>&1 & PID[b]=$!

PA=""
for _ in $(seq 50); do
    PA="$(grep -oP 'probe requests on <[^:>]*:\K[0-9a-f]{32}' "$WORK/a.log" || true)"
    [[ -n "$PA" ]] && break
    sleep 0.2
done
[[ -n "$PA" ]] || fail "node A did not start"
sleep 2

roe_pid() { # device
    pgrep -f -- "--packet $1\$" || true
}

# B probes A until a reply arrives, or gives up after $1 seconds.
expect_reachable() { # timeout what
    local deadline=$((SECONDS + $1))
    while (( SECONDS < deadline )); do
        if "$VENV/rnprobe" --config "$WORK/b" -n 1 -t 3 rnstransport.probe "$PA" 2>&1 \
            | tr '\r' '\n' | grep -q 'packet loss 0.0%'; then
            echo "  A reachable again after $(( $1 - (deadline - SECONDS) )) s"
            return 0
        fi
        sleep 0.5
    done
    fail "$2: A not reachable after $1 s"
}

expect_unreachable() { # what
    if "$VENV/rnprobe" --config "$WORK/b" -n 1 -t 3 rnstransport.probe "$PA" 2>&1 \
        | tr '\r' '\n' | grep -q 'packet loss 0.0%'; then
        fail "$1: A still reachable"
    fi
    echo "  A unreachable, as expected"
}

restarts() { # node
    grep -c 'Restarted the roe helper' "$WORK/$1.log" || true
}

echo "== 0. baseline"
expect_reachable 10 "baseline"

echo "== 1. roe helper killed"
old="$(roe_pid a0)"
[[ -n "$old" ]] || fail "no roe helper running for a0"
kill -9 "$old"
expect_reachable 20 "after killing roe"
new="$(roe_pid a0)"
[[ -n "$new" && "$new" != "$old" ]] || fail "roe helper was not restarted"
echo "  roe restarted: pid $old -> $new"

echo "== 2. interface set down and up"
before="$(restarts a)"
ip link set a0 down
sleep 3
expect_unreachable "a0 down"
ip link set a0 up
expect_reachable 20 "after a0 up"
n=$(( $(restarts a) - before ))
echo "  roe restarts during the outage: $n"
(( n == 0 )) || fail "roe should ride out an interface that is only down"

echo "== 3. carrier lost and back (peer end down)"
before="$(restarts a)"
ip link set b0 down
sleep 1
ip -brief link show a0 | grep -q 'LOWER_UP' && fail "a0 still has carrier"
expect_unreachable "carrier lost"
ip link set b0 up
expect_reachable 20 "after carrier back"
n=$(( $(restarts a) - before ))
echo "  roe restarts during the outage: $n"
(( n == 0 )) || fail "roe should ride out a carrier loss"

echo "== 4. interface removed and recreated"
before="$(restarts a)"
old_idx="$(cat /sys/class/net/a0/ifindex 2>/dev/null || ip -o link show a0 | cut -d: -f1)"
ip link del a0
sleep 3
make_link
new_idx="$(ip -o link show a0 | cut -d: -f1)"
echo "  a0 ifindex $old_idx -> $new_idx"
expect_reachable 30 "after recreating a0"
n=$(( $(restarts a) - before ))
echo "  roe restarts: $n"
(( n >= 1 )) || fail "roe should restart to rebind the new interface"

echo "== 5. rnsd killed"
helper="$(roe_pid a0)"
[[ -n "$helper" ]] || fail "no roe helper running for a0"
kill -9 "${PID[a]}"; wait "${PID[a]}" 2>/dev/null || true
unset 'PID[a]'
for _ in $(seq 20); do
    kill -0 "$helper" 2>/dev/null || break
    sleep 0.1
done
kill -0 "$helper" 2>/dev/null && fail "roe helper $helper outlived rnsd"
echo "  roe helper exited with rnsd"

echo
echo "PASS"
