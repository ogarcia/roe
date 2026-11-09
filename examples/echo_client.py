# Echo client for the Rust examples: a plain Python Reticulum node that
# finds the example's echo destination, opens a link to it, sends a message
# and waits for it to come back.
#
# Usage: python echo_client.py <configdir> <destination hash> [message]

import sys
import time

import RNS

TIMEOUT = 30


def wait(what, condition):
    deadline = time.time() + TIMEOUT
    while not condition():
        if time.time() > deadline:
            sys.exit(f"FAIL: timed out waiting for {what}")
        time.sleep(0.1)


def main():
    configdir, dest_hex = sys.argv[1], sys.argv[2]
    message = (sys.argv[3] if len(sys.argv) > 3 else "hello over RoE").encode()
    dest_hash = bytes.fromhex(dest_hex)

    RNS.Reticulum(configdir)

    if not RNS.Transport.has_path(dest_hash):
        RNS.Transport.request_path(dest_hash)
    wait("a path to the echo destination", lambda: RNS.Transport.has_path(dest_hash))
    print(f"path found, {RNS.Transport.hops_to(dest_hash)} hop(s)")

    identity = RNS.Identity.recall(dest_hash)
    destination = RNS.Destination(
        identity, RNS.Destination.OUT, RNS.Destination.SINGLE, "roe_example", "echo"
    )
    link = RNS.Link(destination)
    wait("the link to come up", lambda: link.status == RNS.Link.ACTIVE)
    print(f"link active, rtt {link.rtt * 1000:.1f} ms")

    replies = []
    link.set_packet_callback(lambda data, packet: replies.append(data))
    started = time.time()
    RNS.Packet(link, message).send()
    wait("the echo", lambda: replies)
    elapsed = (time.time() - started) * 1000
    if replies[0] != message:
        sys.exit(f"FAIL: sent {message!r}, got back {replies[0]!r}")
    print(f"echo received in {elapsed:.1f} ms: {replies[0].decode()}")
    link.teardown()


if __name__ == "__main__":
    main()
