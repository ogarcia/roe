#!/usr/bin/env bash
# Runs one of the Rust examples in examples/ against a Python Reticulum
# node over RoE, on a veth pair without IP:
#
#   example (Rust, echo server) ── a0 ═══ b0 ── echo_client.py (Python)
#
# The Python side finds the example's echo destination, opens a link to it
# and checks that a message comes back. Not run in CI: the examples pull in
# third-party Reticulum implementations and take a while to build.
#
# Usage: [KEEP=1] tests/examples.sh <example directory name>
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="${1:?usage: tests/examples.sh <example>, one of: $(cd "$ROOT/examples" && ls -d */ | tr -d / | tr '\n' ' ')}"
EXAMPLE="$ROOT/examples/$NAME"

if [[ "${ROE_IN_NS:-}" != 1 ]]; then
    [[ -f "$EXAMPLE/Cargo.toml" ]] || { echo "no example $NAME"; exit 2; }
    cargo build --quiet --release --manifest-path "$ROOT/Cargo.toml"
    cargo build --quiet --release --manifest-path "$EXAMPLE/Cargo.toml"
    exec env ROE_IN_NS=1 unshare --user --map-root-user --net -- "$0" "$@"
fi

BIN="$ROOT/target/release/roe"
EXAMPLE_BIN="$(find "$EXAMPLE/target/release" -maxdepth 1 -type f -executable -name 'roe-example-*' | head -n 1)"
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

ip link set lo up
ip link add a0 type veth peer name b0
for i in a0 b0; do
    ip link set "$i" addrgenmode none
    ip link set "$i" up
done

# The example takes the device; those that start roe find it in $ROE.
ROE="$BIN" "$EXAMPLE_BIN" a0 > "$WORK/example.log" 2>&1 &
PID[example]=$!
DST=""
for _ in $(seq 100); do
    DST="$(grep -oP 'echo destination /?\K[0-9a-f]{32}' "$WORK/example.log" || true)"
    [[ -n "$DST" ]] && break
    sleep 0.2
done
[[ -n "$DST" ]] || { cat "$WORK/example.log"; fail "the example did not start"; }
echo "$NAME: echo destination <$DST>"

# The Python side: a standalone Reticulum instance with an RoeInterface.
mkdir -p "$WORK/py/interfaces"
cp "$ROOT/dist-files/RoeInterface.py" "$WORK/py/interfaces/"
cat > "$WORK/py/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = No

[logging]
  loglevel = 4

[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = b0
    binary = $BIN
EOF

if ! timeout 90 "$VENV/python" "$ROOT/examples/echo_client.py" "$WORK/py" "$DST" > "$WORK/client.log" 2>&1; then
    tail -n 20 "$WORK/client.log"
    echo "--- example log"
    tail -n 30 "$WORK/example.log"
    fail "no echo from $NAME"
fi
grep -E 'path found|link active|echo received' "$WORK/client.log"
echo
echo "PASS"
