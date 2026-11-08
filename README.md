# RoE: Reticulum over Ethernet

<p align="center"><strong>English</strong> · <a href="README.es.md">Español</a></p>

RoE (Reticulum over Ethernet) lets [Reticulum](https://reticulum.network)
nodes on the same Ethernet segment talk to each other directly at layer 2,
with their own EtherType, instead of tunnelling through UDP or TCP over IP.
It is unrelated to IEEE 1914.3, also known as RoE (Radio over Ethernet).

Reticulum is designed to be independent of IP: over LoRa, serial links or
packet radio it *is* the network. On Ethernet, however, its built-in
interfaces all ride on top of the host's IP stack. `roe` closes that gap:
Reticulum packets go straight into Ethernet frames, no IP address needed,
not even an IPv6 link-local one.

```
 Reticulum ── RoeInterface ══ socket pair ══ roe ── AF_PACKET ── Ethernet
 (rnsd, Python)                             (Rust, CAP_NET_RAW)    EtherType 0x88B5
```

> **Status: experimental.** It works, it is tested (see
> [docs/testing.md](docs/testing.md)), and on real hardware it is faster than
> Reticulum's own UDP transport, but the wire format may still change.

## Features

- **No IP between nodes.** Frames use the IEEE local experimental EtherType
  `0x88B5` and a locally administered multicast group address.
- **Faster than UDP.** Between two physical hosts, a 10 MB transfer ran at
  72.7 Mbps over `roe` against 53.2 Mbps over Reticulum's `AutoInterface`,
  thanks to 1432-byte packets (see [docs/testing.md](docs/testing.md)).
- **Privilege separation.** Only the small `roe` binary needs
  `CAP_NET_RAW`; `rnsd` runs unprivileged.
- **Works with Reticulum routing.** Transport nodes forward between `roe`
  segments, or between `roe` and any other interface, like with any other
  Reticulum interface.
- **Self-healing.** Survives the interface going down, cable unplugs, NICs
  disappearing and coming back, and helper crashes.
- **Static binaries** for x86_64 and aarch64 (musl), no runtime dependencies.

## Requirements

- Linux (`AF_PACKET` raw sockets).
- Reticulum 1.6.0 or later (tested with 1.6.0).
- All nodes on the same layer 2 segment (same switch, bridge or cable).
  Routers do not forward these frames; Reticulum transport nodes do.
- `CAP_NET_RAW` for the `roe` binary, or running it as root.

## Installation

### Arch Linux

The [AUR package](https://aur.archlinux.org/packages/reticulum-over-ethernet)
installs `/usr/bin/roe` with `CAP_NET_RAW` already set, and the
`RoeInterface` module in `/usr/share/roe/`:

```sh
yay -S reticulum-over-ethernet      # or any other AUR helper
```

### Prebuilt binaries

Every [release](https://github.com/ogarcia/roe/releases) includes static
binaries for x86_64 and aarch64, which run on any Linux distribution:

```sh
curl -LO https://github.com/ogarcia/roe/releases/latest/download/roe-x86_64-linux-musl
curl -LO https://github.com/ogarcia/roe/releases/latest/download/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
sudo install -m 755 roe-x86_64-linux-musl /usr/local/bin/roe
sudo setcap cap_net_raw+ep /usr/local/bin/roe
```

`RoeInterface.py` is included in the bundles of the same release.

### From source

```sh
cargo build --release
sudo install -m 755 target/release/roe /usr/local/bin/roe
sudo setcap cap_net_raw+ep /usr/local/bin/roe
```

`RoeInterface.py` is in `dist-files/`.

## Quick start with a bundle

Bundles are self-contained test kits: a static `roe`, the `RoeInterface`
module and the `roe-node` helper, which sets up Reticulum in a local venv
and runs a node without touching `~/.reticulum`.

```sh
scripts/pack.sh                     # builds dist/roe-<version>-{x86_64,aarch64}.tar.gz
```

On each machine:

```sh
tar xzf roe-0.1.0-x86_64.tar.gz && cd roe-0.1.0-x86_64/
./roe-node setup                    # installs Reticulum into ./.venv (uv or pip)
./roe-node start eth0               # runs the node; asks sudo once for setcap
```

Then, from another terminal on one machine:

```sh
./roe-node id                       # this node's probe hash
./roe-node probe <other-node-hash>  # Reticulum "ping"
./roe-node listen                   # receive files into ./received
./roe-node send <file> <hash>       # send a file to a listening node
```

Do not unpack a bundle under `/tmp`, or any other `nosuid` mount: file
capabilities are ignored there and `roe` cannot open the interface.
`roe-node` detects this and says so.

## Using RoE with your own Reticulum setup

Once `roe` is [installed](#installation), put the interface module in your
Reticulum config directory. With the AUR package, link it so that package
updates reach it (a system-wide `/etc/reticulum` already has the link):

```sh
mkdir -p ~/.reticulum/interfaces
ln -s /usr/share/roe/RoeInterface.py ~/.reticulum/interfaces/
```

Otherwise, copy it from a bundle or from `dist-files/`:

```sh
mkdir -p ~/.reticulum/interfaces
cp dist-files/RoeInterface.py ~/.reticulum/interfaces/
```

And add an interface to `~/.reticulum/config`:

```ini
[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = eth0
    binary = /usr/local/bin/roe
```

| Option | Default | Meaning |
|---|---|---|
| `device` | (required) | Network interface to use. |
| `binary` | `roe` from `PATH` | Path to the `roe` binary (`/usr/bin/roe` with the AUR package). |
| `respawn_delay` | `5` | Seconds before restarting `roe` if it exits. |
| `verbose` | `No` | Make `roe` log every frame it sends or receives. |

Standard Reticulum interface options (`mode`, `network_name`,
`passphrase`, `ifac_size`, `bitrate`, ...) work as usual.

`roe` can also run under Reticulum's built-in `PipeInterface`
(`command = roe eth0`). That needs no extra module, but it is about six
times slower; see [docs/testing.md](docs/testing.md).

## As a library

The crate also exposes the pieces `roe` is built from: the frame format,
the raw socket and the framings towards Reticulum. They are meant for
projects that want to speak RoE directly, such as a Reticulum
implementation in Rust. See the
[API documentation](https://docs.rs/reticulum-over-ethernet).

```sh
cargo add reticulum-over-ethernet
```

## Building

```sh
cargo build --release                                   # native
cargo build --release --target x86_64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build --release --target aarch64-unknown-linux-musl
```

The tests need unprivileged user namespaces and a Python venv with
Reticulum; see [docs/testing.md](docs/testing.md).

To run the same format and lint checks as CI before every commit
(rustfmt, clippy and, if installed, ruff), enable the repository hooks
once per clone:

```sh
git config core.hooksPath .githooks
```

## Documentation

- [Motivation](docs/motivation.md): why Reticulum on Ethernet should not
  need IP, goals and non-goals.
- [Protocol](docs/protocol.md): wire format, addressing, MTU, the two
  protocols between `roe` and Reticulum, versioning and security.
- [Architecture](docs/architecture.md): components, privilege model,
  failure handling, platform support.
- [Testing](docs/testing.md): test suite and measured results.
