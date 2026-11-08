# Running as a service

This document covers running `rnsd` with RoE as a system service, and in
particular how `roe` gets the `CAP_NET_RAW` capability it needs there. The
examples use systemd.

## How roe gets CAP_NET_RAW

`roe` needs `CAP_NET_RAW` to open its raw Ethernet socket. It can get it in
two ways, and the right one depends on how the service is sandboxed.

**A file capability on the binary.** `setcap cap_net_raw+ep /usr/bin/roe`
(the AUR package does this on install). `rnsd` runs with no capabilities at
all, and `roe` gains `CAP_NET_RAW` when `rnsd` starts it. This keeps the
capability in the smallest possible process, but the kernel ignores file
capabilities in some situations; see [below](#settings-that-break-roe).

**An ambient capability from the service.** The unit grants
`AmbientCapabilities=CAP_NET_RAW`, and `rnsd` passes it on to every program
it starts, `roe` included. The binary needs no file capability, and this
works even with `NoNewPrivileges=yes`. The cost is that `rnsd`, a Python
process, holds `CAP_NET_RAW` too.

How each one behaves, starting from a process with no capabilities of its
own, as an unprivileged `rnsd` is:

| `roe` gets the capability from | Normal process | With `no_new_privs` |
|---|---|---|
| Nowhere | cannot open the socket | cannot open the socket |
| A file capability | works | **cannot open the socket** |
| An ambient capability | works | works |

These results were reproduced with `capsh` and `setpriv`, which use the same
kernel mechanisms as systemd: `no_new_privs` is what `NoNewPrivileges=yes`
sets.

Use the file capability with a plain unit, and the ambient capability with
a hardened one.

## Where Reticulum keeps its state

Reticulum stores its identities, path tables and log file inside its
configuration directory. A service therefore needs a configuration
directory it can write to. With systemd, the simplest is a state directory,
`/var/lib/reticulum`, passed to `rnsd` with `--config`:

```
/var/lib/reticulum/
├── config                      Reticulum configuration
├── interfaces/
│   └── RoeInterface.py         link to /usr/share/roe/RoeInterface.py
└── storage/                    created and kept by Reticulum
```

```sh
sudo install -d /var/lib/reticulum/interfaces
sudo ln -s /usr/share/roe/RoeInterface.py /var/lib/reticulum/interfaces/
```

With the AUR package the module is at `/usr/share/roe/RoeInterface.py`;
otherwise copy it from a bundle or from `dist-files/`. A link means package
updates reach it. The interface in `config` is set up as usual:

```ini
[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = eth0
    binary = /usr/bin/roe
```

Running `rnsd` without `--service` makes it log to standard output, so its
messages end up in the journal next to those of `roe`.

## A plain unit

`roe` has a file capability, and the unit adds no sandboxing that would
disable it:

```ini
[Unit]
Description=Reticulum Network Stack daemon
After=network.target

[Service]
User=reticulum
ExecStart=/usr/bin/rnsd --config /var/lib/reticulum
StateDirectory=reticulum
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Adjust the path to `rnsd` to your installation (a venv, for instance).
Create the user first, for example with
`sudo useradd --system --home-dir /var/lib/reticulum reticulum`.

## A hardened unit

The same service, sandboxed. `NoNewPrivileges=yes` disables the file
capability, so `roe` gets `CAP_NET_RAW` as an ambient capability instead:

```ini
[Unit]
Description=Reticulum Network Stack daemon
After=network.target

[Service]
User=reticulum
ExecStart=/usr/bin/rnsd --config /var/lib/reticulum
StateDirectory=reticulum
Restart=on-failure
RestartSec=5

# CAP_NET_RAW for roe, inherited from rnsd; nothing else.
AmbientCapabilities=CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_RAW
NoNewPrivileges=yes

# AF_PACKET for roe; AF_UNIX for the shared instance and for RoeInterface;
# AF_INET and AF_INET6 only if other interfaces use IP. Other Reticulum
# interfaces may need further families.
RestrictAddressFamilies=AF_UNIX AF_PACKET AF_INET AF_INET6

ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

The file capability on the binary does no harm here: the ambient
capability takes over.

The capability behaviour these units rely on has been reproduced (see the
table above), but the units themselves have not been run under systemd as
part of the test suite.

## Settings that break roe

| Setting | Effect on roe | What to do |
|---|---|---|
| `NoNewPrivileges=yes` | The file capability is ignored: `roe` cannot open its socket and keeps being restarted. | Use `AmbientCapabilities=CAP_NET_RAW`. |
| Options that imply `NoNewPrivileges=` | Same as above. For a service that does not run as root, systemd enables `NoNewPrivileges=` implicitly when certain sandboxing options are set, such as `SystemCallFilter=`, `RestrictAddressFamilies=` or `MemoryDenyWriteExecute=` (see `systemd.exec(5)` for the full list). | Same as above. |
| `CapabilityBoundingSet=` without `CAP_NET_RAW` | `roe` cannot get the capability in any way. With a file capability, the kernel refuses to start it at all (`Operation not permitted`). | Include `CAP_NET_RAW`. |
| `RestrictAddressFamilies=` without `AF_PACKET` | `roe` cannot create its socket. | Include `AF_PACKET` (and `AF_UNIX`). |
| `PrivateNetwork=yes` | The service only sees a private loopback interface, not the real ones. | Do not use it. |
| Binary on a `nosuid` filesystem | File capabilities are ignored. | Install `roe` elsewhere, or use the ambient capability. |

A capability problem shows in the log as `roe` failing to open the
interface with `Operation not permitted`, repeated every `respawn_delay`
seconds.
