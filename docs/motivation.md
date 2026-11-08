# Motivation

## Reticulum is not an IP overlay, except on Ethernet

Reticulum is a complete network stack, not a protocol on top of TCP/IP. It
has its own addressing (truncated hashes of public keys), its own routing
(transport nodes and announces) and end-to-end encryption, and it treats
every physical medium as just another interface: LoRa through RNode, packet
radio through KISS/AX.25, serial lines, I2P, and IP.

On Ethernet, though, every built-in interface goes through IP:

- `AutoInterface` discovers peers with IPv6 link-local multicast and sends
  data over UDP.
- `UDPInterface`, `TCPClientInterface`, `TCPServerInterface` and
  `BackboneInterface` are, as their names say, UDP and TCP.

So on the medium where most Reticulum nodes actually live, the stack that
claims independence from IP depends on the host's IP configuration after
all. Classic LAN protocols such as IPX/SPX or AppleTalk did not: they had
their own EtherType and sat directly on the link layer. `rtp` gives
Reticulum the same.

## What depending on IP costs in practice

Running over IP is not wrong, and for links across the Internet it is the
only option. On a single Ethernet segment it brings costs that Reticulum
does not need:

- **It needs a working IP stack.** `AutoInterface` needs IPv6 enabled and a
  link-local address on the interface. Hosts with IPv6 disabled, or
  interfaces configured with `addrgenmode none`, cannot use it.
- **It goes through the IP firewall.** While testing `rtp`, Reticulum's own
  `AutoInterface` failed between two Arch Linux hosts because the default
  `nftables` rules dropped its UDP ports (`No multicast echoes received`).
  `rtp` had worked between the same hosts from the first try. This cuts both
  ways; see [protocol.md](protocol.md#security-considerations).
- **It exposes the node to the IP network.** A node reachable over UDP or
  TCP has the attack surface of an IP service. A node that only speaks
  `rtp` has no IP surface at all on that interface.
- **It adds overhead.** IPv6 and UDP headers take 48 bytes per packet, and
  every packet goes through the IP layer of both hosts. On fast Ethernet
  this matters less than the next point.
- **Packet size is not chosen for the medium.** `AutoInterface` fixes its
  hardware MTU at 1196 bytes. `rtp` uses 1432, close to the 1500-byte
  Ethernet payload, and Reticulum's per-packet cost is high enough (it is
  written in Python, and every packet is encrypted) that fewer, larger
  packets are noticeably faster.

## Goals

- Carry Reticulum packets directly in Ethernet frames, with no IP address
  required on the interface.
- Behave like any other Reticulum interface: work with transport nodes,
  IFAC, interface modes and all the standard tools.
- Be at least as fast as Reticulum's IP-based interfaces on the same
  hardware.
- Keep the privileged part minimal: only the code that opens the raw socket
  should need `CAP_NET_RAW`.
- Survive the failures a LAN sees: interfaces going down, cables pulled,
  USB NICs unplugged, crashed helpers.
- Be easy to try: static binaries and a self-contained test bundle.

## Non-goals

- **Routing beyond the segment.** `rtp` is a link layer. Reaching other
  segments is the job of Reticulum transport nodes, which can forward
  between `rtp` and any other interface.
- **Replacing IP interfaces over the Internet.** TCP and Backbone
  interfaces remain the right tool to join nodes across routed networks.
- **Unicast at layer 2.** All frames go to one group address. Reticulum
  packets carry no source address, so mapping destinations to MAC addresses
  would mean `rtp` second-guessing Reticulum's routing. See
  [protocol.md](protocol.md#addressing).
- **Reliability at layer 2.** Like Ethernet itself and like every other
  Reticulum interface, `rtp` is best effort. Reticulum provides proofs,
  retransmission and ordering where they are needed.

## Results so far

Between two physical hosts on the same LAN, transferring 10 MB with `rncp`:

| Transport | Throughput |
|---|---|
| `rtp` through Reticulum's `PipeInterface` (first version) | 12.1 Mbps |
| Reticulum's `AutoInterface` (UDP over IPv6 link-local) | 53.2 Mbps |
| `rtp` through `RtpInterface` | **72.7 Mbps** |

Multi-hop routing through a transport node, recovery from link failures and
the aarch64 build are covered too. See [testing.md](testing.md) for the
details and caveats.
