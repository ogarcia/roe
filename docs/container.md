# Container

The `Containerfile` builds an image with Reticulum (`rnsd` and its tools),
`roe` and `RoeInterface`, ready to run an RoE node.

## Building

```sh
podman build -t roe .
```

The image is built for the architecture of the machine building it
(x86_64 and aarch64 are both tested in CI). `roe` is compiled in a first
stage into a static binary; the final image is based on `python:alpine`,
with a pinned Reticulum version that can be changed with
`--build-arg RNS_VERSION=...`.

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
