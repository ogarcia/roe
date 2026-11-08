# Architecture

## Components

```
 ┌──────────────── rnsd (Python, unprivileged) ────────────────┐
 │                                                             │
 │  Reticulum transport ── RtpInterface (RtpInterface.py)      │
 │                              │  starts and restarts rtp     │
 └──────────────────────────────┼──────────────────────────────┘
                                │  AF_UNIX / SOCK_SEQPACKET socket pair
                                │  (stdin and stdout of rtp)
 ┌──────────────────────────────┼──────────────────────────────┐
 │  rtp (Rust, CAP_NET_RAW)     │                              │
 │                    rx thread ┴ tx thread                     │
 │                              │                              │
 │                     AF_PACKET socket, EtherType 0x88B5       │
 └──────────────────────────────┼──────────────────────────────┘
                                │
                             Ethernet
```

There are three pieces:

- **`rtp`**, a small Rust binary that owns the raw socket and translates
  between Ethernet frames and packets on its standard input and output.
- **`RtpInterface`**, a Reticulum interface module written in Python that
  starts `rtp`, exchanges packets with it and restarts it when needed.
- **`rtp-node`**, a shell script that makes the test bundle self-contained.
  It is not needed to use `rtp` with an existing Reticulum setup.

## rtp

`rtp` has no configuration besides its command line:

```
rtp [-v] [--packet] <interface>
```

At startup it:

1. resolves the interface name to its index;
2. opens an `AF_PACKET`/`SOCK_RAW` socket for EtherType `0x88B5` and binds
   it to that interface, so it only ever sees frames of that type on that
   interface;
3. joins the `rtp` group address on the interface (`PACKET_ADD_MEMBERSHIP`);
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
`RtpInterface`. See [protocol.md](protocol.md#local-protocols-between-rtp-and-reticulum).

`rtp` logs to standard error, which ends up in the `rnsd` log. With `-v` it
logs every frame.

The code is split into a library and a thin binary:

| Module | Role |
|---|---|
| `src/frame.rs` | Frame layout: building, parsing, validation. |
| `src/hdlc.rs` | HDLC encoder and streaming decoder. |
| `src/packet.rs` | `SOCK_SEQPACKET` helpers for packet mode. |
| `src/link.rs` | `AF_PACKET` socket: open, send, receive, link state. Linux only. |
| `src/main.rs` | Command line, the two loops, logging. |

## RtpInterface

`RtpInterface.py` is an external interface module. Reticulum loads it from
the `interfaces` directory of its configuration directory when an interface
has `type = RtpInterface`.

When the interface is created, it:

1. creates an `AF_UNIX`/`SOCK_SEQPACKET` socket pair;
2. starts `rtp --packet <device>` with one end of the pair as both its
   standard input and output, and keeps the other end;
3. starts a reader thread that receives one packet per `recv()` and hands
   it to Reticulum.

Sending is a single `send()` per packet. There is no framing, no escaping
and no byte-by-byte loop on the Python side, which is what makes it much
faster than `PipeInterface` (see [testing.md](testing.md)).

It also tells Reticulum about the medium:

| Attribute | Value | Effect |
|---|---|---|
| `HW_MTU` | 1432 | Largest packet `rtp` can carry with any IFAC size; lower on interfaces with a smaller MTU, re-read on every `rtp` start. |
| `FIXED_MTU` | `True` | Lets Reticulum negotiate links up to `HW_MTU`. |
| `bitrate` | 100 Mbps | Estimate used for Reticulum's timeouts; override with `bitrate`. |
| `shared_medium` | `True` | Every node on the segment hears every packet. |

If `rtp` exits for any reason, the reader thread sees end-of-file, marks
the interface offline, waits `respawn_delay` seconds and starts a new
`rtp`, until it succeeds. When Reticulum shuts the interface down, it kills
`rtp` and stops restarting it.

## Privilege model

Opening a packet socket requires `CAP_NET_RAW`. Giving that to `rnsd` would
mean giving it to a whole Python interpreter, and with it to every
interface module and utility it loads.

Instead, only `rtp` gets the capability, as a file capability:

```sh
setcap cap_net_raw+ep /path/to/rtp
```

`rnsd` runs as a normal user and starts `rtp`, which gains the capability
on `exec`. `rtp` uses it to open its socket, and is small enough to be
audited in one sitting.

File capabilities are ignored on filesystems mounted `nosuid` (often
`/tmp`) and in processes running with `no_new_privs` (some sandboxes and
systemd units). `rtp-node` checks for both and explains the problem.

## Failure handling

| Event | What happens |
|---|---|
| `rtp` crashes or is killed | `RtpInterface` restarts it after `respawn_delay`. |
| Interface set down | `rtp` keeps its socket, drops outgoing packets and logs the outage once. Traffic resumes as soon as the interface is up, without a restart. |
| Carrier lost (cable unplugged) | Frames are silently lost, as on any Ethernet. Traffic resumes when the carrier comes back. |
| Interface removed (USB NIC unplugged) | `rtp` notices within a second that the name no longer maps to the index it is bound to, and exits. `RtpInterface` restarts it until the interface exists again, then traffic resumes on the new interface. |
| Interface MTU shrinks | Sending fails with `EMSGSIZE`; `rtp` exits, and on restart `RtpInterface` lowers its hardware MTU to match. |
| `rnsd` exits or crashes | Its end of the socket pair closes; `rtp` reads end-of-file and exits. No orphan is left behind. |

Two details make this work:

- **Down is not gone.** `ENETDOWN`, `ENXIO`, `ENODEV` and `ENOBUFS` are
  treated as a temporary lack of link, not as a fatal error.
- **A removed interface is detected by polling.** A packet socket bound to
  an interface that has been deleted gets no further events and would block
  forever. The one-second receive timeout gives `rtp` a chance to check
  `if_nametoindex()` regularly.

Reticulum itself copes with lost packets: links time out and are
re-established, and paths are re-learnt from announces.

## Platform support

`rtp` is Linux only: it relies on `AF_PACKET` sockets. Other systems would
need their own backend in `src/link.rs`: BPF on macOS and the BSDs, Npcap
on Windows. Android would need root.

Static musl builds are produced for x86_64 and aarch64. The aarch64 build
is tested under qemu user-mode emulation (see [testing.md](testing.md)).
qemu user mode does not translate `SOL_PACKET` socket options, so the group
membership cannot be set there; `rtp` warns and carries on, which is
harmless on the virtual interfaces the tests use. On real ARM hardware the
membership works as on x86_64.

## The test bundle and rtp-node

`scripts/pack.sh` builds one bundle per architecture:

```
rtp-<version>-<arch>/
├── rtp               static binary
├── RtpInterface.py   interface module
├── rtp-node          helper script
├── README.es.md      instructions (Spanish)
└── LICENSE           GPL-3.0 (RtpInterface.py carries its own MIT notice)
```

`rtp-node` keeps everything inside the unpacked directory: a venv with a
pinned Reticulum version, a node configuration with its own instance name
and ports (so it does not clash with a regular `rnsd` on the host), the
node's identities and received files. Besides the normal mode it can start
the node with `--pipe` (`rtp` under `PipeInterface`) or `--udp`
(Reticulum's `AutoInterface`, without `rtp`), to compare performance on
the same hosts.
