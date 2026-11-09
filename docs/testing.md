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
| `cargo test` | Frame building and parsing, HDLC encoding and decoding, `SOCK_SEQPACKET` message boundaries and truncation, SHA-256 test vectors, and the neighbor table against packets made by Reticulum, with the packet hashes and link ids it computes for them. |
| `tests/netns.sh` | Two `rnsd` instances on a veth pair with no IP address, `roe` under `PipeInterface`; probe, and that both ends joined the group address. |
| `tests/bundle.sh` | A packaged bundle, unpacked twice as if on two machines, each node in its own network namespace; probe and a file transfer. `MODE=roe\|pipe\|udp` picks the interface, `SIZE_MB` the file size. |
| `tests/multihop.sh` | Two clients on separate segments joined only by a transport node with two `RoeInterface`s: 2-hop path, file transfer, and no connectivity once the transport node stops. |
| `tests/robustness.sh` | Recovery after the helper is killed, the interface goes down, the carrier is lost, the interface is removed and recreated; no orphan helper after `rnsd` dies. |
| `tests/mtu.sh` | Interfaces with an MTU below 1500: hardware MTU adapted at start, no oversized frames, adaptation when the MTU drops while running, refusal to start below 504 bytes. |
| `tests/ifac.sh` | Three nodes on a bridged segment with the largest IFAC (64 bytes): nodes on the same IFAC network reach each other, a node with another key does not, and full-size packets plus the IFAC fit a frame. |
| `tests/bridge.sh` | A transport node bridging RoE and TCP: a node with only RoE finds and exchanges files with a node with only TCP, 2 hops away. |
| `tests/vlan.sh` | Two clients on different 802.1Q VLANs of one trunk, and a transport node with an `RoeInterface` on each VLAN: reachability over a VLAN, a 2-hop path and a file transfer across VLANs, and no connectivity between VLANs without the transport node. |
| `tests/unicast.sh` | Two nodes and an empty third port on a learning bridge: probe, a file transfer whose frames almost never reach the third port, and, with `unicast = No`, the same transfer flooded to it. |
| `tests/container.sh` | Builds the container image and runs two nodes in containers on a bridge network; one probes the other over RoE. Uses `container/roe` if present (CI puts the build job's binary there), builds it otherwise. Needs a root container engine (`ENGINE="sudo podman"`). |
| `tests/qemu.sh` | `tests/bundle.sh` with the aarch64 build run under qemu user-mode emulation, to test it locally without ARM hardware. |

The tests use a veth pair because, unlike a real NIC, it delivers frames
for any multicast address, joined or not. `tests/netns.sh` therefore checks
the group membership explicitly with `ip maddr`.

## Continuous integration

GitHub Actions (`.github/workflows/ci.yml`) runs on every push and pull
request:

- rustfmt, clippy with warnings as errors, the unit tests and ruff;
- static musl builds for x86_64 and aarch64, uploaded as artifacts, both
  as bare binaries and as test bundles;
- the end-to-end tests (`netns`, `bundle`, `multihop`, `robustness`,
  `mtu`, `ifac`, `bridge`, `vlan`), natively on an x86_64 runner and on an arm64 runner. On arm64
  they run on a real aarch64 kernel, so unlike under qemu the group
  membership is set and checked.

A separate job builds the container image and runs `tests/container.sh`,
also on both architectures. On pushes, the tested images are published to
GitHub Container Registry (see [container.md](container.md)).

`tests/qemu.sh` is not run in CI, since the arm64 runner covers the same
ground natively.

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

### IFAC

`tests/ifac.sh`, three nodes on a Linux bridge, `ifac_size = 512`:

- A node on the same IFAC network: probe without loss.
- A node with another passphrase: its path request times out.
- 2 MB transfer: file identical; the largest packet on the wire is
  1479 bytes, a 1415-byte link packet plus the 64-byte IFAC, within the
  1496 bytes a frame carries; nothing dropped.

### Bridging RoE and TCP

`tests/bridge.sh`, A ─RoE─ T ─TCP─ B: A finds B's destination 2 hops away
via T, and 2 MB files cross the bridge identical in both directions.

### VLANs

`tests/vlan.sh`, VLANs 10 and 20 on one trunk: the client on VLAN 10 finds
the destination on VLAN 20 2 hops away via the transport node, a 2 MB file
crosses identical, the VLAN interfaces keep the full MTU, and with the
transport node stopped no link can be established between the VLANs.

### Unicast

`tests/unicast.sh`, A and B on a bridge with an empty third port: probe
without loss; a 2 MB transfer of 1459 frames, of which the third port
received 1; with `unicast = No` on A, 1461.

### Wi-Fi

A laptop on Wi-Fi and a wired desktop, through the access point, 1 hop:

| | Probe round trip | Loss |
|---|---|---|
| Everything to the group, laptop in power save | ~375 ms | some |
| Everything to the group, power save off | ~315 ms | none |
| Unicast to known neighbors | 5–6 ms | none |

A capture on the desktop showed it answering each probe in half a
millisecond: with multicast, the rest of the round trip was the access
point holding the replies to the laptop until its next DTIM beacon, as it
does while any of its stations is in power save. With unicast, a 262 KB
file went from the desktop to the laptop at 20.9 Mbps.

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

- **Other Wi-Fi hardware.** Only one access point and one laptop have been
  tried (see [Wi-Fi](#wi-fi)). Other access points and drivers may drop
  frames with EtherTypes they do not know.
- **Specific NIC drivers.** Both architectures are tested natively in CI,
  aarch64 included, but on veth interfaces, which accept any multicast
  address. Whether a physical NIC accepts the group address depends on its
  driver's multicast filter, not on the CPU architecture. It has been seen
  working on real x86_64 hosts on a wired LAN; other NICs, such as the USB
  Ethernet adapters common on ARM boards, have not been tried.
- **systemd units.** The capability behaviour in
  [deployment.md](deployment.md) was reproduced with `capsh` and `setpriv`,
  but the example units have not been run under systemd.
- **Larger segments.** All tests use two or three nodes.
- **Repeated measurements.** Each figure above comes from a single run.
