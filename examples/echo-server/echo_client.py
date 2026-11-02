"""Sends packets to an echo server, one every 20 ms, and reports what came back as metrics.

Usage: echo_client.py ADDRESS PORT COUNT. The metrics go to standard output as lines of JSON, which
netscenario reads: one `rtt` (round trip time, ms) for every answer, and at the end `sent` and
`answered`.
"""

import json
import select
import socket
import struct
import sys
import time

address, port, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
GAP = 0.020
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sent_at: dict[int, float] = {}
answered = 0


def metric(name: str, value: float, unit: str | None = None) -> None:
    line = {"metric": name, "value": value}
    if unit:
        line["unit"] = unit
    print(json.dumps(line), flush=True)


def receive(until: float) -> None:
    """Reads the answers that arrive before `until`."""
    global answered
    while (left := until - time.monotonic()) > 0:
        if not select.select([sock], [], [], left)[0]:
            return
        data, _ = sock.recvfrom(2048)
        number = struct.unpack(">I", data[:4])[0]
        if number in sent_at:
            metric("rtt", (time.monotonic() - sent_at.pop(number)) * 1000, "ms")
            answered += 1


start = time.monotonic()
for number in range(count):
    sent_at[number] = time.monotonic()
    sock.sendto(struct.pack(">I", number) + b"x" * 60, (address, port))
    receive(start + (number + 1) * GAP)
receive(time.monotonic() + 0.5)  # the last answers: a round trip is 60 ms
metric("sent", count)
metric("answered", answered)
