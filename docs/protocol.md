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
| 0 | 6 | Destination MAC | Always the group address `03:52:4e:53:00:01`. |
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
[Interface Access Code](#security-considerations) configured. `roe` does not
look into it.

## Addressing

Every frame is sent to the group address `03:52:4e:53:00:01`:

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
these frames to every port, as they do with broadcast. The gain is in the
hosts, not in the switch.

### Why no unicast

Reticulum is designed for shared media such as radio, where every node
hears every packet, and its packets carry no source address. `roe` presents
the Ethernet segment to Reticulum in the same way: as one shared medium.

Sending to individual MAC addresses would need a table from Reticulum
destinations or links to MACs, learnt by peeking into packets. That would
duplicate, at the link layer, decisions that belong to Reticulum's
transport. On a switched LAN the bandwidth saved is not worth it.

### Receiving

`roe` binds a packet socket to its interface and to EtherType `0x88B5`. The
kernel delivers every matching frame that the NIC accepts: the group
address, broadcast, the interface's own unicast address, or anything at all
in promiscuous mode. Receivers do not check the destination MAC.

Frames that the host itself transmitted are skipped (packet type
`PACKET_OUTGOING`), so a node never hears its own packets.

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

`roe` works on VLAN interfaces (`eth0.10` and the like) without changes:
the kernel adds and removes the 802.1Q tag. If the VLAN interface has a
smaller MTU, the previous section applies.

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
