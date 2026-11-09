# Protocol

This document specifies how `roe` carries Reticulum packets over Ethernet
(version 1 of the frame format), and the two local protocols between `roe`
and Reticulum.

## Overview

Every Reticulum packet travels in exactly one Ethernet frame. There is no
fragmentation, no acknowledgement and no state: `roe` is a best-effort link
layer, like Ethernet itself. Anything above that (proofs, retransmission,
ordering, encryption) is Reticulum's job and works unchanged.

## Frame format

```
 0                   6                   12      14  15  16      18
 +-------------------+-------------------+-------+---+---+-------+--------------
 |  destination MAC  |    source MAC     | type  | M | V |  len  | payload ...
 +-------------------+-------------------+-------+---+---+-------+--------------
   03:52:4e:53:00:01   sender's NIC MAC   0x88B5  'R'  1   u16 BE
                                                  \______________/
                                                     roe header
```

All multi-byte fields are big-endian (network byte order).

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 6 | Destination MAC | The group address `03:52:4e:53:00:01`, or a neighbor's MAC (see [Addressing](#addressing)). |
| 6 | 6 | Source MAC | MAC address of the sending interface. |
| 12 | 2 | EtherType | `0x88B5`. |
| 14 | 1 | Magic | `0x52` (ASCII `R`). |
| 15 | 1 | Version | `0x01` for this specification. |
| 16 | 2 | Length | Length of the payload in bytes. |
| 18 | *len* | Payload | One Reticulum packet, byte for byte. |

The frame check sequence is added and stripped by the NIC, as usual.

### EtherType

`0x88B5` is the first of the two EtherTypes that IEEE reserves for local
experimental use ("Local Experimental Ethertype 1"). It needs no
registration, but anyone may use it, so it says nothing about the content on
its own. Hence the magic and version bytes: a receiver drops any frame whose
magic is not `0x52` or whose version it does not support.

A registered EtherType would remove that ambiguity. It is worth requesting
if the format becomes stable and used outside experiments.

### Length and padding

Ethernet frames have a minimum size of 60 bytes without the FCS, and NICs
pad shorter frames with zeros. A small Reticulum packet can therefore arrive
with trailing bytes that are not part of it, and the frame size alone does
not tell the payload size. The length field does.

A receiver:

- drops frames shorter than the 18-byte header;
- drops frames whose length field is larger than the bytes actually present
  after the header;
- ignores any bytes after the payload.

### Payload

The payload is the Reticulum packet exactly as Reticulum hands it to any
interface, including the IFAC field when the interface has an
[Interface Access Code](#security-considerations) configured. `roe` only
reads its header, to choose the destination MAC, and never changes it.

## Addressing

### The group address

Frames that every node should receive go to the group address
`03:52:4e:53:00:01`:

- The first octet, `0x03`, has both the individual/group bit and the
  universal/local bit set: it is a **locally administered multicast**
  address, which no vendor will ever assign.
- The next three octets spell `RNS` in ASCII.

Each `roe` instance joins this group on its interface
(`PACKET_ADD_MEMBERSHIP`), which programs the NIC's multicast filter. Hosts
that do not run `roe` drop these frames in the NIC hardware, so they never
reach their kernel. Broadcast would interrupt every host on the segment for
every Reticulum packet.

Switches do not snoop group membership for non-IP multicast, so they flood
these frames to every port, as they do with broadcast.

### Unicast to known neighbors

Reticulum is designed for shared media such as radio, where every node
hears every packet, and `roe` presents the Ethernet segment to it in the
same way. Most packets are meant for a single neighbor, though, and on some
media multicast costs much more than unicast. Wi-Fi is the clearest case:
an access point holds multicast frames until the next DTIM beacon whenever
any of its stations is in power save, which adds hundreds of milliseconds,
and sends them at its lowest rate, without acknowledgements or
retransmissions.

So `roe` sends each packet for a known neighbor straight to that
neighbor's MAC address. Reticulum does not tell an interface which neighbor
a packet is for, but its packet header, which is not encrypted, says
enough to work it out from the traffic already received. For every packet
it receives, `roe` notes the source MAC against a key from the header:

| Received packet | Key | Used for |
|---|---|---|
| Announce sent by its origin (no transport id), path responses for its own destinations included | Destination hash | Packets for that destination, one hop away. |
| Announce rebroadcast by a transport node, or a path response from its cache | Its transport id | Packets in transport, which name their next hop by that id. |
| Link request | Link id | The proof that opens the link. |
| Any packet on a link | Link id | Every later packet on the link. |
| Other packet for a single destination | Packet hash | The proof for that packet. |

The packet hash and the link id are computed as Reticulum does: a SHA-256
of the low nibble of the flags and everything after the hops and transport
id, truncated to 16 bytes; for the link id, without any signalling bytes
after the public keys.

Everything else goes to the group: announces, path requests and other
packets for plain or group destinations, packets for neighbors not heard
from, and packets with an interface access code, whose header is masked.
A key heard from two different MACs is ambiguous and also goes to the
group, until it has only come from one of them for 60 seconds. Entries
expire: 30 minutes for announces, 15 minutes after the last packet on a
link, 2 minutes for proofs. So a wrong or missing entry costs at most what
multicast costs, never a packet that multicast would have delivered.

Unicast changes nothing for Reticulum: a node only acts on packets in
transport that name it, on links it is part of, and on packets for its own
destinations, so the neighbors that no longer receive a packet would have
dropped it anyway. It also changes nothing in the frame format, and older
`roe` releases, which send everything to the group, accept unicast frames,
so both can share a segment.

`roe --no-unicast`, or `unicast = No` in the `RoeInterface` configuration,
sends everything to the group, as releases before unicast did.

### Receiving

`roe` binds a packet socket to its interface and to EtherType `0x88B5`. The
kernel delivers every matching frame that the NIC accepts: the group
address, broadcast, the interface's own unicast address, or anything at all
in promiscuous mode.

`roe` skips frames that the host itself transmitted (packet type
`PACKET_OUTGOING`), so a node never hears its own packets, and unicast
frames for other hosts (`PACKET_OTHERHOST`), which only arrive in
promiscuous mode. It accepts everything else without checking the
destination MAC.

## MTU

With a standard 1500-byte Ethernet payload and the 4-byte `roe` header, a
frame can carry **1496 bytes** of Reticulum packet.

`RoeInterface` declares a fixed hardware MTU of **1432 bytes**: 1496 minus
64 bytes reserved for the largest possible IFAC field, so any IFAC size
fits. Declaring the MTU as fixed (`FIXED_MTU`) lets Reticulum negotiate
links with packets up to that size instead of its 500-byte default. In
practice, link traffic is carried in packets of about 1415 bytes, once
Reticulum rounds to its encryption block size.

Reticulum's built-in `PipeInterface` does not declare a fixed MTU, so `roe`
under `PipeInterface` carries 500-byte packets.

Jumbo frames are not used. Every node on the segment would need them
enabled, and a single node without them would silently lose large packets.

### Interfaces with a smaller MTU

On an interface with an MTU below 1500 bytes, the payload limit is the
interface MTU minus the 4-byte header, and everything scales down with it:

- `RoeInterface` reads the interface MTU each time it starts `roe` and sets
  its hardware MTU to that limit minus the 64-byte IFAC reserve (but never
  below Reticulum's 500 bytes). New links are negotiated accordingly.
- `roe` reads the MTU at startup and drops, with a log line, any packet
  that would not fit.
- If the MTU shrinks while running, sending fails with `EMSGSIZE`. `roe`
  then exits, and the restart lets `RoeInterface` pick up the new MTU.
- Reticulum needs every interface to carry 500-byte packets, so an MTU
  below 504 bytes cannot work: `roe` refuses to start and says so.

For example, on an interface with an MTU of 1000 bytes, `RoeInterface`
declares a hardware MTU of 932 bytes.

### VLANs

RoE frames can travel on 802.1Q VLANs like any other Ethernet traffic. The
VLAN tag is part of the Ethernet header, before the EtherType, so it does
not belong in the RoE header: `roe` uses a VLAN interface, and the kernel
(or the NIC, with VLAN offload) adds and removes the tag.

```sh
ip link add link eth0 name eth0.10 type vlan id 10
ip link set eth0.10 up
```

```ini
  [[VLAN 10]]
    type = RoeInterface
    interface_enabled = Yes
    device = eth0.10
```

On a managed switch, the host port can be an access port on any VLAN, with
`roe` on the plain interface, or a trunk, with a VLAN interface per VLAN on
the host. RoE does not need the native VLAN.

Each VLAN is a separate segment, to Reticulum as to IP. A transport node
with one `RoeInterface` per VLAN routes between them, much like a router on
a stick; `tests/vlan.sh` checks exactly that, and that nothing crosses
between VLANs without it.

VLAN interfaces normally keep a 1500-byte MTU. If one has a smaller MTU,
the previous section applies.

## Versioning

The version byte identifies the frame format. This document describes
version 1. Receivers drop frames with any other version, so incompatible
nodes ignore each other instead of misreading packets.

Builds from before version 1 was settled sent to the broadcast address
instead of the group address. They use the same version byte, but cannot
receive from newer builds, because their NIC filter does not accept the
group address. All nodes on a segment should run the same release.

## Local protocols between roe and Reticulum

`roe` is a separate process that Reticulum starts and talks to through its
standard input and output. It supports two framings on that channel.

### Packet mode (`roe --packet <interface>`)

Used by `RoeInterface`. Standard input and standard output are both the
same `AF_UNIX`/`SOCK_SEQPACKET` socket, one end of a socket pair whose other
end Reticulum keeps.

- Each message on the socket is exactly one Reticulum packet, in either
  direction. There is no framing.
- `roe` checks at startup that its standard input is a `SOCK_SEQPACKET`
  socket and refuses to run otherwise.
- Packets longer than 1496 bytes are dropped and logged; `roe` detects them
  with `MSG_TRUNC`, so a truncated packet is never sent.
- When Reticulum closes its end, `roe` reads end-of-file and exits.

### HDLC mode (`roe <interface>`)

Compatible with Reticulum's built-in `PipeInterface`, which exchanges
packets over a pipe using simplified HDLC framing:

- Each packet is sent as `FLAG`, the escaped packet, `FLAG`, where `FLAG` is
  `0x7E`.
- Inside the packet, `0x7E` and `0x7D` are each replaced by `0x7D` followed
  by the byte XOR `0x20`.
- A `FLAG` both ends the current frame and starts the next one, so
  back-to-back frames may share it.

This mode needs no extra module, but `PipeInterface` reads the pipe one
byte at a time in Python and does not declare a fixed MTU. It is several
times slower than packet mode (see [testing.md](testing.md)).

## Security considerations

- **Confidentiality and authenticity come from Reticulum.** `roe` adds no
  encryption or authentication. It does not need to: Reticulum encrypts
  traffic end to end and authenticates destinations by their keys. An
  observer on the segment sees the same thing a radio listener would.
- **Use IFAC on shared segments.** Any host on the segment can inject
  frames. Reticulum's Interface Access Codes (`network_name` and/or
  `passphrase` in the interface configuration) make nodes drop packets
  that are not signed for their network. Any `ifac_size` up to the maximum
  of 512 bits fits in an RoE frame: `RoeInterface` reserves room for it.
- **IP firewalls do not see `roe`.** Packet sockets receive frames before
  the IP layer, so rules in the `ip`, `ip6` and `inet` families of
  `nftables`, or in `iptables`, do not apply. To filter `roe` traffic, use
  the `netdev` family on the interface, for example:

  ```sh
  nft add table netdev roefilter
  nft add chain netdev roefilter ingress \
      '{ type filter hook ingress device "eth0" priority 0; }'
  nft add rule netdev roefilter ingress ether type 0x88b5 drop
  ```

  On a Linux bridge, the `bridge` family can do the same for forwarded
  frames.
- **Privilege is confined to `roe`.** Opening a packet socket requires
  `CAP_NET_RAW`. Only the `roe` binary needs it; the Reticulum daemon runs
  unprivileged and talks to `roe` over a local socket. See
  [architecture.md](architecture.md#privilege-model).
