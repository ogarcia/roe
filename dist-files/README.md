# rtp: Reticulum Transmission Protocol

<p align="center"><strong>English</strong> · <a href="README.es.md">Español</a></p>

Test bundle to make two or more Reticulum nodes talk directly over Ethernet
(EtherType `0x88B5`), with no TCP/IP between them.

## Requirements on each machine

- Linux (x86_64 or aarch64, depending on the bundle).
- `uv`, or `python3` with `venv`, and Internet access the first time, to
  install Reticulum.
- `sudo` (only once, to give the `rtp` binary `CAP_NET_RAW`), or running as
  root.
- The machines must be on the same layer 2 segment: same switch or a direct
  cable. Routers do not forward these frames.

## Usage

```sh
tar xzf rtp-*.tar.gz && cd rtp-*/
./rtp-node setup               # installs Reticulum into ./.venv
./rtp-node start eth0          # starts the node (stays in the foreground)
```

Do not unpack the bundle under `/tmp` or on any other filesystem mounted
`nosuid`: file capabilities are ignored there and `rtp` cannot open the
interface. `rtp-node` detects this and tells you.

In another terminal on the same machine:

```sh
./rtp-node id                  # this node's hash, to give to the other one
./rtp-node probe <other-hash>  # Reticulum ping to the other node
./rtp-node status              # interface status and traffic
```

File transfer, to measure throughput:

```sh
./rtp-node listen              # on the receiver: shows its hash
./rtp-node send file <hash>    # on the sender, with the hash listen showed
```

Everything stays inside the unpacked directory (`.venv`, `node/` and
`received/`). `~/.reticulum` is not touched, and the node does not clash
with a regular `rnsd` already running on the machine.

## Checking that there is no IP involved

```sh
sudo tcpdump -e -n -i eth0 ether proto 0x88b5
```

While running `probe` or `send` you should see the Reticulum frames, all
sent to the group address `03:52:4e:53:00:01`. The node has no IP
interface configured.

That address is multicast, not broadcast. The NIC of a machine that does
not run `rtp` drops these frames in hardware. That is why all machines in
the test must use the same version of the bundle. `ip maddr show dev eth0`
shows whether the node has joined the group.

## Interface modes

`./rtp-node start eth0` uses `RtpInterface`, a Reticulum interface included
in the bundle that talks to `rtp` over a `SOCK_SEQPACKET` socket (one
message per packet, no framing) and uses packets of up to 1432 bytes. This
is the normal mode.

To compare throughput with the same `send` test:

- `./rtp-node start --pipe eth0` uses Reticulum's `PipeInterface` with
  `rtp`. This is the first version: HDLC framing and 500-byte packets.
- `./rtp-node start --udp eth0` uses Reticulum's `AutoInterface` (UDP over
  IPv6 link-local), without `rtp`. It needs the firewall to allow UDP on
  ports 29716 and 42671; `rtp` does not, because it works below IP.

With `RTP_VERBOSE=1 ./rtp-node start eth0`, `rtp` logs every frame it sends
or receives.

## Known issues

- It may not work over Wi-Fi. Some access points or drivers drop EtherTypes
  they do not know.
- `rtp` exits if it cannot open the interface. `rnsd` retries every 2 s, so
  the error shows up repeatedly in the log.
