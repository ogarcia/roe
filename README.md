# rtp: Reticulum Transmission Protocol

`rtp` (Reticulum Transmission Protocol) lets
[Reticulum](https://reticulum.network) nodes on the same Ethernet segment
talk to each other directly at layer 2, with their own EtherType, instead of
tunnelling through UDP or TCP over IP. It is unrelated to RTP, the Real-time
Transport Protocol used for audio and video.

Reticulum is designed to be independent of IP: over LoRa, serial links or
packet radio it *is* the network. On Ethernet, however, its built-in
interfaces all ride on top of the host's IP stack. `rtp` closes that gap:
Reticulum packets go straight into Ethernet frames, no IP address needed,
not even an IPv6 link-local one.

```
 Reticulum ── RtpInterface ══ socket pair ══ rtp ── AF_PACKET ── Ethernet
 (rnsd, Python)                             (Rust, CAP_NET_RAW)    EtherType 0x88B5
```

> **Status: experimental.** It works, it is tested (see
> [docs/testing.md](docs/testing.md)), and on real hardware it is faster than
> Reticulum's own UDP transport, but the wire format may still change.

## Features

- **No IP between nodes.** Frames use the IEEE local experimental EtherType
  `0x88B5` and a locally administered multicast group address.
- **Faster than UDP.** Between two physical hosts, a 10 MB transfer ran at
  72.7 Mbps over `rtp` against 53.2 Mbps over Reticulum's `AutoInterface`,
  thanks to 1432-byte packets (see [docs/testing.md](docs/testing.md)).
- **Privilege separation.** Only the small `rtp` binary needs
  `CAP_NET_RAW`; `rnsd` runs unprivileged.
- **Works with Reticulum routing.** Transport nodes forward between `rtp`
  segments, or between `rtp` and any other interface, like with any other
  Reticulum interface.
- **Self-healing.** Survives the interface going down, cable unplugs, NICs
  disappearing and coming back, and helper crashes.
- **Static binaries** for x86_64 and aarch64 (musl), no runtime dependencies.

## Requirements

- Linux (`AF_PACKET` raw sockets).
- Reticulum 1.6.0 or later (tested with 1.6.0).
- All nodes on the same layer 2 segment (same switch, bridge or cable).
  Routers do not forward these frames; Reticulum transport nodes do.
- `CAP_NET_RAW` for the `rtp` binary, or running it as root.

## Quick start with a bundle

Bundles are self-contained test kits: a static `rtp`, the `RtpInterface`
module and the `rtp-node` helper, which sets up Reticulum in a local venv
and runs a node without touching `~/.reticulum`.

```sh
scripts/pack.sh                     # builds dist/rtp-<version>-{x86_64,aarch64}.tar.gz
```

On each machine:

```sh
tar xzf rtp-0.1.0-x86_64.tar.gz && cd rtp-0.1.0-x86_64/
./rtp-node setup                    # installs Reticulum into ./.venv (uv or pip)
./rtp-node start eth0               # runs the node; asks sudo once for setcap
```

Then, from another terminal on one machine:

```sh
./rtp-node id                       # this node's probe hash
./rtp-node probe <other-node-hash>  # Reticulum "ping"
./rtp-node listen                   # receive files into ./received
./rtp-node send <file> <hash>       # send a file to a listening node
```

Do not unpack a bundle under `/tmp`, or any other `nosuid` mount: file
capabilities are ignored there and `rtp` cannot open the interface.
`rtp-node` detects this and says so.

## Using rtp with your own Reticulum setup

Build the binary and give it the capability it needs:

```sh
cargo build --release
sudo install -m 755 target/release/rtp /usr/local/bin/rtp
sudo setcap cap_net_raw+ep /usr/local/bin/rtp
```

Install the interface module in your Reticulum config directory:

```sh
mkdir -p ~/.reticulum/interfaces
cp dist-files/RtpInterface.py ~/.reticulum/interfaces/
```

And add an interface to `~/.reticulum/config`:

```ini
[interfaces]
  [[Ethernet]]
    type = RtpInterface
    interface_enabled = Yes
    device = eth0
    binary = /usr/local/bin/rtp
```

| Option | Default | Meaning |
|---|---|---|
| `device` | (required) | Network interface to use. |
| `binary` | `rtp` from `PATH` | Path to the `rtp` binary. |
| `respawn_delay` | `5` | Seconds before restarting `rtp` if it exits. |
| `verbose` | `No` | Make `rtp` log every frame it sends or receives. |

Standard Reticulum interface options (`mode`, `ifac_netname`,
`ifac_netkey`, `bitrate`, ...) work as usual.

`rtp` can also run under Reticulum's built-in `PipeInterface`
(`command = rtp eth0`). That needs no extra module, but it is about six
times slower; see [docs/architecture.md](docs/architecture.md).

## Building

```sh
cargo build --release                                   # native
cargo build --release --target x86_64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build --release --target aarch64-unknown-linux-musl
```

The tests need unprivileged user namespaces and a Python venv with
Reticulum; see [docs/testing.md](docs/testing.md).

## Documentation

- [Motivation](docs/motivation.md): why Reticulum on Ethernet should not
  need IP, goals and non-goals.
- [Protocol](docs/protocol.md): wire format, addressing, MTU, the two
  protocols between `rtp` and Reticulum, versioning and security.
- [Architecture](docs/architecture.md): components, privilege model,
  failure handling, platform support.
- [Testing](docs/testing.md): test suite and measured results.
