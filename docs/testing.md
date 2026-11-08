# Testing

## Setup

The unit tests only need Rust:

```sh
cargo test
```

The end-to-end tests run real Reticulum nodes. They need:

- a Python venv with Reticulum in `.venv` at the repository root:

  ```sh
  uv venv && uv pip install rns==1.6.0
  ```

- unprivileged user namespaces (`unshare --user --map-root-user --net`
  must work). Each test re-executes itself in a new user and network
  namespace, where it is root and can create virtual interfaces and open
  packet sockets, without touching the host network or needing `sudo`.

All tests build what they need, clean up after themselves, and keep their
work directory and logs with `KEEP=1`.

## Test suite

| Test | What it checks |
|---|---|
| `cargo test` | Frame building and parsing, HDLC encoding and decoding, `SOCK_SEQPACKET` message boundaries and truncation. |
| `tests/netns.sh` | Two `rnsd` instances on a veth pair with no IP address, `roe` under `PipeInterface`; probe, and that both ends joined the group address. |
| `tests/bundle.sh` | A packaged bundle, unpacked twice as if on two machines, each node in its own network namespace; probe and a file transfer. `MODE=roe\|pipe\|udp` picks the interface, `SIZE_MB` the file size. |
| `tests/multihop.sh` | Two clients on separate segments joined only by a transport node with two `RoeInterface`s: 2-hop path, file transfer, and no connectivity once the transport node stops. |
| `tests/robustness.sh` | Recovery after the helper is killed, the interface goes down, the carrier is lost, the interface is removed and recreated; no orphan helper after `rnsd` dies. |
| `tests/mtu.sh` | Interfaces with an MTU below 1500: hardware MTU adapted at start, no oversized frames, adaptation when the MTU drops while running, refusal to start below 504 bytes. |
| `tests/qemu.sh` | `tests/bundle.sh` with the aarch64 build run under `qemu-aarch64-static`. |

The tests use a veth pair because, unlike a real NIC, it delivers frames
for any multicast address, joined or not. `tests/netns.sh` therefore checks
the group membership explicitly with `ip maddr`.

## Results

All throughput figures are for a 10 MB file of random data sent with
`rncp`, as reported by `rncp`, in a single run. They compare transports on
the same hosts; they are not a benchmark of Reticulum.

### Between two physical hosts

Two Linux hosts on the same wired LAN, both running the same bundle, with
Reticulum 1.6.0.

| Transport | Throughput | Relative |
|---|---|---|
| `roe` under `PipeInterface` | 12.1 Mbps | 1× |
| Reticulum's `AutoInterface` (UDP over IPv6 link-local) | 53.2 Mbps | 4.4× |
| `roe` under `RoeInterface` | **72.7 Mbps** | **6.0×** |

A packet capture during the `RoeInterface` transfer showed every frame sent
to `03:52:4e:53:00:01` with EtherType `0x88b5`, including the resource
proofs coming back.

`AutoInterface` needed the hosts' `nftables` rules to allow UDP ports 29716
and 42671; `roe` needed no firewall change.

### Between network namespaces

Two nodes on a veth pair on one host (`tests/bundle.sh`):

| Mode | Throughput | Total time |
|---|---|---|
| `pipe` | 25.0 Mbps | 4.2 s |
| `roe` | 88.8 Mbps | 1.9 s |
| `udp` | 88.8 Mbps | 1.9 s |

`roe` and `udp` give exactly the same figures, to the millisecond, across
repeated runs. On a veth pair neither transport is the bottleneck: the
transfer time is set by Reticulum and `rncp` themselves. Differences
between them only show on real hardware.

### Multi-hop

Through a transport node, A → T → B (`tests/multihop.sh`):

- A finds B's destination **2 hops away via T**.
- 10 MB transfer at **99.8 Mbps** (2.6 s including link setup), file
  identical. T forwards frames with about 1415 bytes of payload: the large
  link MTU holds across the transport node.
- With T stopped, A cannot establish a link to B.

### Smaller MTU

`tests/mtu.sh`, two nodes on a veth pair:

| Situation | Result |
|---|---|
| MTU 1000 from the start | `RoeInterface` hardware MTU 932; 500 kB transferred on the first attempt; largest packet 915 bytes; nothing dropped. |
| MTU lowered to 800 while running | `roe` exits on `EMSGSIZE` and restarts; hardware MTU 732; transfer succeeds on the second attempt. |
| MTU 400 | `roe` refuses to start: Reticulum needs an MTU of at least 504. |

### aarch64 under qemu

`tests/qemu.sh`: both nodes run the aarch64 `roe` under
`qemu-aarch64-static` on an x86_64 host. Probe without loss, 10 MB transfer
at **79.9 Mbps**, file identical, despite the emulation.

### Robustness

`tests/robustness.sh`, two nodes, `respawn_delay = 2`:

| Event | `roe` restarts | Traffic back after |
|---|---|---|
| `roe` killed with `SIGKILL` | 1 (expected) | ~4 s |
| Interface down for 3 s, then up | 0 | ~1 s |
| Carrier lost (peer down), then back | 0 | ~1 s |
| Interface removed and recreated (new index) | 2 (expected) | ~5 s |
| `rnsd` killed with `SIGKILL` | `roe` exits with it | n/a |

Before the link-state handling was added, an interface going down made
`roe` restart every `respawn_delay` for the whole outage, and traffic took
up to `respawn_delay` to resume.

## Not covered yet

- **Wi-Fi.** Some access points and drivers drop frames with EtherTypes they
  do not know. Untested.
- **Real ARM hardware.** The aarch64 build is only tested under emulation,
  where the group membership cannot be set.
- **Larger segments.** All tests use two or three nodes.
- **Repeated measurements.** Each figure above comes from a single run.
