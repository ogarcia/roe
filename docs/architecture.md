# Architecture

## Components

```
 ┌──────────────── rnsd (Python, unprivileged) ────────────────┐
 │                                                             │
 │  Reticulum transport ── RoeInterface (RoeInterface.py)      │
 │                              │  starts and restarts roe     │
 └──────────────────────────────┼──────────────────────────────┘
                                │  AF_UNIX / SOCK_SEQPACKET socket pair
                                │  (stdin and stdout of roe)
 ┌──────────────────────────────┼──────────────────────────────┐
 │  roe (Rust, CAP_NET_RAW)     │                              │
 │                    rx thread ┴ tx thread                     │
 │                              │                              │
 │                     AF_PACKET socket, EtherType 0x88B5       │
 └──────────────────────────────┼──────────────────────────────┘
                                │
                             Ethernet
```

There are three pieces:

- **`roe`**, a small Rust binary that owns the raw socket and translates
  between Ethernet frames and packets on its standard input and output.
- **`RoeInterface`**, a Reticulum interface module written in Python that
  starts `roe`, exchanges packets with it and restarts it when needed.
- **`roe-node`**, a shell script that makes the test bundle self-contained.
  It is not needed to use `roe` with an existing Reticulum setup.

## roe

`roe` has no configuration besides its command line:

```
roe [-v] [--packet] <interface>
```

At startup it:

1. resolves the interface name to its index;
2. opens an `AF_PACKET`/`SOCK_RAW` socket for EtherType `0x88B5` and binds
   it to that interface, so it only ever sees frames of that type on that
   interface;
3. joins the `roe` group address on the interface (`PACKET_ADD_MEMBERSHIP`);
4. sets a one-second receive timeout (see [failure handling](#failure-handling));
5. reads the interface's MAC address, used as the source of every frame;
6. reads the interface MTU, which sets the largest packet it will send
   ([protocol.md](protocol.md#interfaces-with-a-smaller-mtu)).

Then it runs two loops:

- the **receive thread** reads frames from the socket, validates them
  ([protocol.md](protocol.md#length-and-padding)) and writes each payload
  upstream, to standard output;
- the **main thread** reads packets from standard input, wraps each one in
  a frame and sends it.

The upstream framing is either HDLC, for Reticulum's `PipeInterface`, or
one packet per `SOCK_SEQPACKET` message with `--packet`, for
`RoeInterface`. See [protocol.md](protocol.md#local-protocols-between-roe-and-reticulum).

`roe` logs to standard error, which ends up in the `rnsd` log. With `-v` it
logs every frame.

The code is split into a library and a thin binary:

| Module | Role |
|---|---|
| `src/frame.rs` | Frame layout: building, parsing, validation. |
| `src/hdlc.rs` | HDLC encoder and streaming decoder. |
| `src/packet.rs` | `SOCK_SEQPACKET` helpers for packet mode. |
| `src/link.rs` | `AF_PACKET` socket: open, send, receive, link state. Linux only. |
| `src/main.rs` | Command line, the two loops, logging. |

## RoeInterface

`RoeInterface.py` is an external interface module. Reticulum loads it from
the `interfaces` directory of its configuration directory when an interface
has `type = RoeInterface`.

When the interface is created, it:

1. creates an `AF_UNIX`/`SOCK_SEQPACKET` socket pair;
2. starts `roe --packet <device>` with one end of the pair as both its
   standard input and output, and keeps the other end;
3. starts a reader thread that receives one packet per `recv()` and hands
   it to Reticulum.

Sending is a single `send()` per packet. There is no framing, no escaping
and no byte-by-byte loop on the Python side, which is what makes it much
faster than `PipeInterface` (see [testing.md](testing.md)).

It also tells Reticulum about the medium:

| Attribute | Value | Effect |
|---|---|---|
| `HW_MTU` | 1432 | Largest packet `roe` can carry with any IFAC size; lower on interfaces with a smaller MTU, re-read on every `roe` start. |
| `FIXED_MTU` | `True` | Lets Reticulum negotiate links up to `HW_MTU`. |
| `bitrate` | 100 Mbps | Estimate used for Reticulum's timeouts; override with `bitrate`. |
| `shared_medium` | `True` | Every node on the segment hears every packet. |

If `roe` exits for any reason, the reader thread sees end-of-file, marks
the interface offline, waits `respawn_delay` seconds and starts a new
`roe`, until it succeeds. When Reticulum shuts the interface down, it kills
`roe` and stops restarting it.

## Privilege model

Opening a packet socket requires `CAP_NET_RAW`. Giving that to `rnsd` would
mean giving it to a whole Python interpreter, and with it to every
interface module and utility it loads.

Instead, only `roe` gets the capability, as a file capability:

```sh
setcap cap_net_raw+ep /path/to/roe
```

`rnsd` runs as a normal user and starts `roe`, which gains the capability
on `exec`. `roe` uses it to open its socket, and is small enough to be
audited in one sitting.

File capabilities are ignored on filesystems mounted `nosuid` (often
`/tmp`) and in processes running with `no_new_privs` (some sandboxes and
systemd units). `roe-node` checks for both and explains the problem. Under
systemd, an ambient capability granted to `rnsd` works instead; see
[deployment.md](deployment.md).

## Failure handling

| Event | What happens |
|---|---|
| `roe` crashes or is killed | `RoeInterface` restarts it after `respawn_delay`. |
| Interface set down | `roe` keeps its socket, drops outgoing packets and logs the outage once. Traffic resumes as soon as the interface is up, without a restart. |
| Carrier lost (cable unplugged) | Frames are silently lost, as on any Ethernet. Traffic resumes when the carrier comes back. |
| Interface removed (USB NIC unplugged) | `roe` notices within a second that the name no longer maps to the index it is bound to, and exits. `RoeInterface` restarts it until the interface exists again, then traffic resumes on the new interface. |
| Interface MTU shrinks | Sending fails with `EMSGSIZE`; `roe` exits, and on restart `RoeInterface` lowers its hardware MTU to match. |
| `rnsd` exits or crashes | Its end of the socket pair closes; `roe` reads end-of-file and exits. No orphan is left behind. |

Two details make this work:

- **Down is not gone.** `ENETDOWN`, `ENXIO`, `ENODEV` and `ENOBUFS` are
  treated as a temporary lack of link, not as a fatal error.
- **A removed interface is detected by polling.** A packet socket bound to
  an interface that has been deleted gets no further events and would block
  forever. The one-second receive timeout gives `roe` a chance to check
  `if_nametoindex()` regularly.

Reticulum itself copes with lost packets: links time out and are
re-established, and paths are re-learnt from announces.

## Platform support

`roe` is Linux only: it relies on `AF_PACKET` sockets. Other systems would
need their own backend in `src/link.rs`: BPF on macOS and the BSDs, Npcap
on Windows. Android would need root.

Static musl builds are produced for x86_64 and aarch64. CI runs the whole
end-to-end suite natively on both architectures (see
[testing.md](testing.md#continuous-integration)).

For local testing without ARM hardware, `tests/qemu.sh` runs the aarch64
build under qemu user-mode emulation. qemu user mode does not translate
`SOL_PACKET` socket options, so the group membership cannot be set there;
`roe` warns and carries on, which is harmless on the virtual interfaces the
tests use.

## The test bundle and roe-node

`scripts/pack.sh` builds one bundle per architecture:

```
roe-<version>-<arch>/
├── roe               static binary
├── RoeInterface.py   interface module
├── roe-node          helper script
├── README.md         instructions (English)
├── README.es.md      instructions (Spanish)
└── LICENSE           GPL-3.0 (RoeInterface.py carries its own MIT notice)
```

`roe-node` keeps everything inside the unpacked directory: a venv with a
pinned Reticulum version, a node configuration with its own instance name
and ports (so it does not clash with a regular `rnsd` on the host), the
node's identities and received files. Besides the normal mode it can start
the node with `--pipe` (`roe` under `PipeInterface`) or `--udp`
(Reticulum's `AutoInterface`, without `roe`), to compare performance on
the same hosts.
