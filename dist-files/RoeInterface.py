# SPDX-License-Identifier: MIT
#
# RoeInterface: Reticulum over raw Ethernet through the roe helper.
#
# Copyright (c) 2026 Óscar García Amor
#
# Unlike the rest of the roe project, which is GPL-3.0, this module is
# licensed under the MIT License, so that it can be combined with Reticulum
# and distributed alongside it:
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to
# deal in the Software without restriction, including without limitation the
# rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
# sell copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
# FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
# IN THE SOFTWARE.
#
# ruff: noqa: F821 -- `Interface` and `RNS` are injected by Reticulum.
#
# Reticulum loads this file from the "interfaces" directory of its config
# directory and runs it with `Interface` and `RNS` already in scope.
#
# The helper process `roe --packet <device>` owns the raw socket, and is the
# only part that needs CAP_NET_RAW. It talks to this interface over a
# SOCK_SEQPACKET socket pair, where every message is exactly one packet, so
# there is no framing to encode or decode on the Python side.
#
# Example configuration:
#
#   [[Ethernet]]
#     type = RoeInterface
#     interface_enabled = Yes
#     device = eth0
#     binary = /path/to/roe
#     respawn_delay = 2
#     # Packets for a known neighbor go to its MAC address; set to No to
#     # send everything to the RoE multicast group instead.
#     unicast = Yes

import fcntl
import os
import shutil
import socket
import struct
import subprocess
import threading
import time

SIOCGIFMTU = 0x8921


def interface_mtu(device):
    """Returns the MTU of a network interface, in its network namespace."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as s:
        ifreq = struct.pack("16si20x", device.encode(), 0)
        return struct.unpack("16si20x", fcntl.ioctl(s, SIOCGIFMTU, ifreq))[1]


class RoeInterface(Interface):
    # Ethernet payload is 1500 bytes; roe uses 4 for its own header. Leave
    # room for the largest IFAC signature too, so any ifac_size fits. On
    # interfaces with a smaller MTU, update_mtu() lowers HW_MTU to match.
    ROE_HEADER = 4
    ETH_PAYLOAD = 1500 - ROE_HEADER
    IFAC_RESERVE = 64
    HW_MTU = ETH_PAYLOAD - IFAC_RESERVE

    # A fixed hardware MTU lets Reticulum negotiate links with packets of
    # HW_MTU bytes instead of the 500-byte default.
    FIXED_MTU = True

    BITRATE_GUESS = 100_000_000
    DEFAULT_IFAC_SIZE = 16
    RECV_SIZE = 4096

    def __init__(self, owner, configuration):
        super().__init__()
        c = Interface.get_config_obj(configuration)

        self.owner = owner
        self.name = c["name"]
        self.device = c.get("device")
        if not self.device:
            raise ValueError(f"No device specified for {self}")

        # Reticulum exec()s this module, so there is no __file__ to search
        # next to: take the binary from the config, or from PATH.
        self.binary = c.get("binary") or shutil.which("roe")
        if not self.binary or not os.access(self.binary, os.X_OK):
            raise ValueError(
                f"Cannot find the roe binary for {self}, set 'binary' in its config"
            )

        self.respawn_delay = (
            c.as_float("respawn_delay") if "respawn_delay" in c else 5.0
        )
        self.verbose = c.as_bool("verbose") if "verbose" in c else False
        self.unicast = c.as_bool("unicast") if "unicast" in c else True

        self.HW_MTU = RoeInterface.HW_MTU
        self.bitrate = RoeInterface.BITRATE_GUESS
        self.shared_medium = True
        self.process = None
        self.sock = None
        self.online = False

        self.spawn()
        threading.Thread(target=self.read_loop, daemon=True).start()

    def update_mtu(self):
        """Fits HW_MTU to the interface MTU, which may have changed since
        roe last started. New links are negotiated with the new value."""
        try:
            mtu = interface_mtu(self.device)
        except OSError:
            return  # No interface yet: roe will fail and report it.
        payload = min(self.ETH_PAYLOAD, mtu - self.ROE_HEADER)
        # Below Reticulum's base MTU nothing works; roe refuses to start
        # and explains why. Above it, give up the IFAC reserve if needed.
        hw_mtu = max(payload - self.IFAC_RESERVE, RNS.Reticulum.MTU)
        if hw_mtu != self.HW_MTU:
            RNS.log(
                f"{self} hardware MTU set to {hw_mtu} for an interface MTU of {mtu}",
                RNS.LOG_NOTICE,
            )
            self.HW_MTU = hw_mtu

    def spawn(self):
        self.update_mtu()
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        cmd = [self.binary, "--packet", self.device]
        if not self.unicast:
            cmd.insert(1, "--no-unicast")
        if self.verbose:
            cmd.insert(1, "-v")
        RNS.log(f"Starting {' '.join(cmd)} for {self}", RNS.LOG_VERBOSE)
        try:
            self.process = subprocess.Popen(
                cmd, stdin=theirs, stdout=theirs, close_fds=True
            )
        finally:
            theirs.close()
        self.sock = ours
        self.online = True

    def read_loop(self):
        while True:
            try:
                while True:
                    data = self.sock.recv(self.RECV_SIZE)
                    if not data:
                        break
                    self.process_incoming(data)
                RNS.log(f"The roe helper for {self} exited", RNS.LOG_WARNING)
            except OSError as e:
                RNS.log(
                    f"Error reading from the roe helper for {self}: {e}", RNS.LOG_ERROR
                )

            self.online = False
            self.reap()
            if self.detached:
                return
            self.respawn()

    def respawn(self):
        while not self.detached:
            time.sleep(self.respawn_delay)
            try:
                self.spawn()
                RNS.log(f"Restarted the roe helper for {self}")
                return
            except OSError as e:
                RNS.log(
                    f"Could not restart the roe helper for {self}: {e}", RNS.LOG_ERROR
                )

    def reap(self):
        if self.sock:
            self.sock.close()
        if self.process:
            self.process.kill()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                RNS.log(f"The roe helper for {self} did not exit", RNS.LOG_WARNING)

    def process_incoming(self, data):
        self.rxb += len(data)
        self.owner.inbound(data, self)

    def process_outgoing(self, data):
        if not self.online:
            return
        try:
            self.sock.send(data)
            self.txb += len(data)
        except OSError as e:
            RNS.log(
                f"Could not send through the roe helper for {self}: {e}", RNS.LOG_ERROR
            )

    def detach(self):
        self.detached = True
        self.online = False
        self.reap()

    def __str__(self):
        return f"RoeInterface[{self.name}]"


interface_class = RoeInterface
