# RtpInterface: Reticulum over raw Ethernet through the rtp helper.
#
# ruff: noqa: F821 -- `Interface` and `RNS` are injected by Reticulum.
#
# Reticulum loads this file from the "interfaces" directory of its config
# directory and runs it with `Interface` and `RNS` already in scope.
#
# The helper process `rtp --packet <device>` owns the raw socket, and is the
# only part that needs CAP_NET_RAW. It talks to this interface over a
# SOCK_SEQPACKET socket pair, where every message is exactly one packet, so
# there is no framing to encode or decode on the Python side.
#
# Example configuration:
#
#   [[Ethernet]]
#     type = RtpInterface
#     interface_enabled = Yes
#     device = eth0
#     binary = /path/to/rtp
#     respawn_delay = 2

import os
import shutil
import socket
import subprocess
import threading
import time


class RtpInterface(Interface):
    # Ethernet payload is 1500 bytes; rtp uses 4 for its own header. Leave
    # room for the largest IFAC signature too, so any ifac_size fits.
    ETH_PAYLOAD = 1496
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
        self.binary = c.get("binary") or shutil.which("rtp")
        if not self.binary or not os.access(self.binary, os.X_OK):
            raise ValueError(
                f"Cannot find the rtp binary for {self}, set 'binary' in its config"
            )

        self.respawn_delay = (
            c.as_float("respawn_delay") if "respawn_delay" in c else 5.0
        )
        self.verbose = c.as_bool("verbose") if "verbose" in c else False

        self.HW_MTU = RtpInterface.HW_MTU
        self.bitrate = RtpInterface.BITRATE_GUESS
        self.shared_medium = True
        self.process = None
        self.sock = None
        self.online = False

        self.spawn()
        threading.Thread(target=self.read_loop, daemon=True).start()

    def spawn(self):
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        cmd = [self.binary, "--packet", self.device]
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
                RNS.log(f"The rtp helper for {self} exited", RNS.LOG_WARNING)
            except OSError as e:
                RNS.log(
                    f"Error reading from the rtp helper for {self}: {e}", RNS.LOG_ERROR
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
                RNS.log(f"Restarted the rtp helper for {self}")
                return
            except OSError as e:
                RNS.log(
                    f"Could not restart the rtp helper for {self}: {e}", RNS.LOG_ERROR
                )

    def reap(self):
        if self.sock:
            self.sock.close()
        if self.process:
            self.process.kill()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                RNS.log(f"The rtp helper for {self} did not exit", RNS.LOG_WARNING)

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
                f"Could not send through the rtp helper for {self}: {e}", RNS.LOG_ERROR
            )

    def detach(self):
        self.detached = True
        self.online = False
        self.reap()

    def __str__(self):
        return f"RtpInterface[{self.name}]"


interface_class = RtpInterface
