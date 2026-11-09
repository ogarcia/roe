# Examples: RoE from Rust Reticulum implementations

These examples show how a Reticulum implementation in Rust can use RoE,
without Python. Each one is the same small program, an echo server: it
announces the destination `roe_example.echo`, accepts links to it and sends
every message it receives on a link back.

| Example | Implementation | How it reaches RoE |
|---|---|---|
| [`reticulum-rs-process`](reticulum-rs-process) | [Reticulum-rs](https://github.com/BeechatNetworkSystemsLtd/Reticulum-rs) | Starts the `roe` helper, through a small interface of its own. |
| [`reticulum-rs-linked`](reticulum-rs-linked) | Reticulum-rs | Links the `reticulum-over-ethernet` crate and sends the frames itself. |
| [`rsreticulum-process`](rsreticulum-process) | [rsReticulum](https://github.com/ratspeak/rsReticulum) | Starts the `roe` helper through rsReticulum's own `PipeInterface`, with no interface code. |

They are not part of the `reticulum-over-ethernet` crate: each is a
standalone Cargo package, pinned to a known commit of the implementation
it uses.

## Two ways to use RoE

### Through the roe helper

The program starts `roe` and exchanges packets with it, as `RoeInterface`
does for Python Reticulum. Two modes are available (see
[docs/protocol.md](../docs/protocol.md#local-protocols-between-roe-and-reticulum)):

- **Packet mode**, `roe --packet <device>`: a `SOCK_SEQPACKET` socket pair,
  one packet per message. `reticulum-rs-process` implements it in about 200
  lines, [`roe_interface.rs`](reticulum-rs-process/src/roe_interface.rs),
  which can be copied into another implementation.
- **HDLC mode**, `roe <device>`: HDLC framing over a pipe, as Reticulum's
  `PipeInterface` expects. An implementation with a `PipeInterface` needs
  no code at all, only this in its configuration, as `rsreticulum-process`
  shows:

  ```ini
  [[RoE]]
    type = PipeInterface
    enabled = yes
    command = roe eth0
  ```

  Packet mode is faster, but HDLC needs nothing new.

Either way:

- **Any license.** Two programs talking over a socket or a pipe are
  separate programs, so the implementation does not become a derivative
  of RoE and keeps its own license. That is what makes RoE usable from
  rsReticulum, whose license, the Reticulum License, adds restrictions the
  GPL does not allow in a combined work.
- **Privilege separation.** Only `roe` needs `CAP_NET_RAW`; the
  implementation runs unprivileged.
- **Unicast included.** `roe` keeps the neighbor table, so packets for
  known neighbors go to their MAC address without anything on the other
  side.

### Linking the crate

The implementation uses the crate directly:
[`RawLink`](https://docs.rs/reticulum-over-ethernet/latest/reticulum_over_ethernet/link/struct.RawLink.html)
for the raw socket,
[`frame`](https://docs.rs/reticulum-over-ethernet/latest/reticulum_over_ethernet/frame/index.html)
to build and parse frames and
[`Neighbors`](https://docs.rs/reticulum-over-ethernet/latest/reticulum_over_ethernet/neighbors/struct.Neighbors.html)
to choose the destination MAC. `reticulum-rs-linked` does it in about 130
lines, [`roe_interface.rs`](reticulum-rs-linked/src/roe_interface.rs).

There is no helper process, but:

- **The whole program needs `CAP_NET_RAW`.**
- **The binary is GPL-3.0-or-later.** The crate is, so a program linking
  it is a combined work under the GPL. Reticulum-rs is MIT, which allows
  that. rsReticulum's license does not, so there is no linked example for
  it.

## Running them

`tests/examples.sh` builds an example and runs it on one end of a veth
pair, without IP, against a plain Python Reticulum node with
`RoeInterface` on the other end. The Python side,
[`echo_client.py`](echo_client.py), finds the echo destination, opens a link
and checks that a message comes back:

```sh
tests/examples.sh reticulum-rs-process
tests/examples.sh reticulum-rs-linked
tests/examples.sh rsreticulum-process
```

It needs the Python environment of the other tests (see
[docs/testing.md](../docs/testing.md#setup)). The examples are not built in
CI: they pull in two Reticulum implementations and their dependencies.

To run one on a real network instead, with Python Reticulum on another
host:

```sh
cd examples/reticulum-rs-process
cargo build --release
ROE=/usr/bin/roe target/release/roe-example-reticulum-rs-process eth0
```

`roe` needs `CAP_NET_RAW` as usual. The linked example needs it itself,
for instance with
`sudo setcap cap_net_raw+ep target/release/roe-example-reticulum-rs-linked`.
Then, on the Python side, with `RoeInterface` in its configuration:

```sh
python examples/echo_client.py <configdir> <echo destination hash>
```

## Results

With Python Reticulum 1.6.0 on the other end, all three find the path (1
hop), bring the link up and echo the message back:

| Example | Link round trip | Echo |
|---|---|---|
| `reticulum-rs-process` | 1.1 ms | 0.9 ms |
| `reticulum-rs-linked` | 1.0 ms | 0.9 ms |
| `rsreticulum-process` | 12.7 ms | 0.8 ms |

The link round trip is measured by Python Reticulum while the link comes
up, which includes the key exchange on the other end; the echo is one
message on the established link, there and back.
