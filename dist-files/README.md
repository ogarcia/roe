# RoE: Reticulum over Ethernet

<p align="center"><strong>English</strong> · <a href="README.es.md">Español</a></p>

Test bundle to make two or more Reticulum nodes talk directly over Ethernet
(EtherType `0x88B5`), with no TCP/IP between them.

## Requirements on each machine

- Linux (x86_64 or aarch64, depending on the bundle).
- `uv`, or `python3` with `venv`, and Internet access the first time, to
  install Reticulum.
- `sudo` (only once, to give the `roe` binary `CAP_NET_RAW`), or running as
  root.
- The machines must be on the same layer 2 segment: same switch or a direct
  cable. Routers do not forward these frames.

## Usage

```sh
tar xzf roe-*.tar.gz && cd roe-*/
./roe-node setup               # installs Reticulum into ./.venv
./roe-node start eth0          # starts the node (stays in the foreground)
```

Do not unpack the bundle under `/tmp` or on any other filesystem mounted
`nosuid`: file capabilities are ignored there and `roe` cannot open the
interface. `roe-node` detects this and tells you.

In another terminal on the same machine:

```sh
./roe-node id                  # this node's hash, to give to the other one
./roe-node probe <other-hash>  # Reticulum ping to the other node
./roe-node status              # interface status and traffic
```

File transfer, to measure throughput:

```sh
./roe-node listen              # on the receiver: shows its hash
./roe-node send file <hash>    # on the sender, with the hash listen showed
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
not run `roe` drops these frames in hardware. That is why all machines in
the test must use the same version of the bundle. `ip maddr show dev eth0`
shows whether the node has joined the group.

## Interface modes

`./roe-node start eth0` uses `RoeInterface`, a Reticulum interface included
in the bundle that talks to `roe` over a `SOCK_SEQPACKET` socket (one
message per packet, no framing) and uses packets of up to 1432 bytes. This
is the normal mode.

To compare throughput with the same `send` test:

- `./roe-node start --pipe eth0` uses Reticulum's `PipeInterface` with
  `roe`. This is the first version: HDLC framing and 500-byte packets.
- `./roe-node start --udp eth0` uses Reticulum's `AutoInterface` (UDP over
  IPv6 link-local), without `roe`. It needs the firewall to allow UDP on
  ports 29716 and 42671; `roe` does not, because it works below IP.

With `ROE_VERBOSE=1 ./roe-node start eth0`, `roe` logs every frame it sends
or receives.

## Known issues

- It may not work over Wi-Fi. Some access points or drivers drop EtherTypes
  they do not know.
- `roe` exits if it cannot open the interface. `rnsd` retries every 2 s, so
  the error shows up repeatedly in the log.
