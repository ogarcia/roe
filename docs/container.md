# Container

The `Containerfile` builds an image with Reticulum (`rnsd` and its tools),
`roe` and `RoeInterface`, ready to run an RoE node.

## Published images

CI publishes the image to GitHub Container Registry for x86_64 and
aarch64, as a single multi-architecture image:

| Image | Contents |
|---|---|
| `ghcr.io/ogarcia/roe:master` | The latest commit on `master`. |
| `ghcr.io/ogarcia/roe:latest` | The highest release. A fix released for an older series does not move it, and pre-releases never get it. |
| `ghcr.io/ogarcia/roe:<version>` | A release, such as `ghcr.io/ogarcia/roe:0.1.1`. |

Every image has passed the container test of its own CI run, and carries
the same `roe` binary as that run's build artifacts and release.

## Building

The image does not compile `roe`: it takes a prebuilt static binary from
`container/roe`. In CI that is the binary the build job produces and the
tests use, the same one attached to each release. To build the image
locally, put a static musl build there first:

```sh
cargo build --release --target "$(uname -m)-unknown-linux-musl"
cp "target/$(uname -m)-unknown-linux-musl/release/roe" container/roe
podman build -t roe .
```

A binary downloaded from a release works too (`roe-x86_64-linux-musl` or
`roe-aarch64-linux-musl`, renamed to `container/roe`). It must match the
architecture of the image, which is that of the machine building it.

The image is based on `python:alpine`, with a pinned Reticulum version that
can be changed with `--build-arg RNS_VERSION=...`.

## Running

```sh
podman run -d --name roe --network host --cap-add NET_RAW \
    -e ROE_DEVICE=eth0 -v roe-data:/data roe
```

- `--network host` gives the container the host's network interfaces, so
  RoE frames reach the real LAN.
- `--cap-add NET_RAW` lets `roe` open its raw socket. Docker grants this
  capability by default; Podman does not.
- The container engine must run as root. Rootless containers, even with
  `--network host`, have no access to the host's link layer: their user
  namespace does not own the host's network namespace.

On first start, the container writes `/data/config` with an RoE interface
on `$ROE_DEVICE`. Change it as you like; it is never overwritten while it
exists. `RoeInterface.py` is linked into `/data/interfaces/` on every
start.

| Variable | Default | Meaning |
|---|---|---|
| `ROE_DEVICE` | `eth0` | Network interface for RoE, used when writing the first config. |
| `ROE_TRANSPORT` | `No` | `Yes` makes the node a transport node, used when writing the first config. |

The `/data` volume keeps the configuration, the node's identities and its
paths across restarts and upgrades.

### Alongside an rnsd on the host

With `--network host` the container shares the host's network namespace,
including the abstract Unix sockets Reticulum uses for its shared instance.
If the host also runs `rnsd`, both would try to use the same shared
instance. Give the container's instance its own name in `/data/config`:

```ini
[reticulum]
  instance_name = roe
```

## Using Reticulum's tools

`rnsd` logs to standard output (`podman logs roe`). Any other command given
to the container runs instead of `rnsd`, with the same configuration, so
Reticulum's tools work through `exec`:

```sh
podman exec roe rnstatus
podman exec roe rnprobe rnstransport.probe <hash>
```

## Containers on a bridge network

Containers on the same bridge network (`podman network create`) share a
layer 2 segment, so RoE works between them without `--network host`, with
`ROE_DEVICE=eth0`. `tests/container.sh` uses this to test the image:

```sh
ENGINE="sudo podman" tests/container.sh
```
