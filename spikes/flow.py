#!/usr/bin/env python3
"""A small UDP flow for the spikes of release 0.3: sequence numbers, send times, and a report of what arrived.

All namespaces of one machine share CLOCK_MONOTONIC, so a one-way delay is the difference of two timestamps.

  flow.py server FAMILY ADDRESS PORT COUNT [WAIT]  prints one JSON line: received, missing (a hash and the first 20),
                                                   delay statistics in ms; ends after COUNT packets or WAIT seconds
                                                   without a packet (default 2)
  flow.py client FAMILY ADDRESS PORT COUNT PPS [SRC_PORT]  sends COUNT packets at PPS per second; the size of the
                                                   datagram is FLOW_SIZE bytes (default 200); with FLOW_BURST=n the
                                                   packets go in bursts of n back to back, PPS bursts per second
The server also prints the arrivals per 100 ms bin since the first packet ("bins") and the largest delay.

FAMILY is 4 or 6.
"""

import hashlib
import json
import os
import socket
import struct
import sys
import time

HEADER = struct.Struct("!IQ")


def family(text):
    return socket.AF_INET6 if text == "6" else socket.AF_INET


def percentile(values, fraction):
    if not values:
        return None
    return values[min(len(values) - 1, int(round(fraction * (len(values) - 1))))]


def server(fam, address, port, count, wait):
    sock = socket.socket(family(fam), socket.SOCK_DGRAM)
    sock.bind((address, port))
    sock.settimeout(wait)
    seen = {}
    arrivals = []
    reordered = 0
    highest = -1
    print("ready", flush=True)
    try:
        while len(seen) < count:
            data, _ = sock.recvfrom(2048)
            received = time.monotonic_ns()
            if len(data) < HEADER.size:
                continue
            seq, sent = HEADER.unpack_from(data)
            if seq not in seen:
                if seq < highest:
                    reordered += 1
                highest = max(highest, seq)
                seen[seq] = (received - sent) / 1e6
                arrivals.append(received)
    except socket.timeout:
        pass
    dump = os.environ.get("FLOW_DUMP")
    if dump:
        with open(dump, "w") as handle:
            handle.write("\n".join(str(moment) for moment in arrivals))
    missing = [n for n in range(count) if n not in seen]
    delays = sorted(seen.values())
    bins = []
    if arrivals:
        first = arrivals[0]
        for moment in arrivals:
            index = int((moment - first) / 1e8)
            while len(bins) <= index:
                bins.append(0)
            bins[index] += 1
    print(
        json.dumps(
            {
                "received": len(seen),
                "missing_count": len(missing),
                "missing_hash": hashlib.sha256(",".join(map(str, missing)).encode()).hexdigest()[:16],
                "missing_first": missing[:20],
                "reordered": reordered,
                "bins": bins,
                "delay_max_ms": delays[-1] if delays else None,
                "delay_ms": {
                    "mean": sum(delays) / len(delays) if delays else None,
                    "min": delays[0] if delays else None,
                    "p50": percentile(delays, 0.5),
                    "p99": percentile(delays, 0.99),
                },
            }
        ),
        flush=True,
    )


def client(fam, address, port, count, pps, src_port):
    sock = socket.socket(family(fam), socket.SOCK_DGRAM)
    if src_port:
        # Bind to the address the route to the target would use, not to all interfaces: ask the kernel by connecting a
        # throwaway socket, then bind the real one to that address and the wanted port
        probe = socket.socket(family(fam), socket.SOCK_DGRAM)
        probe.connect((address, port))
        local = probe.getsockname()[0]
        probe.close()
        sock.bind((local, src_port))
    forced = os.environ.get("FLOW_SNDBUFFORCE")
    plain = os.environ.get("FLOW_SNDBUF")
    note = ""
    try:
        if forced:
            sock.setsockopt(socket.SOL_SOCKET, 32, int(forced))  # SO_SNDBUFFORCE
        elif plain:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, int(plain))
    except OSError as error:
        note = str(error)
    sndbuf = sock.getsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF)
    gap = 1.0 / pps
    start = time.monotonic()
    pad = b"x" * (int(os.environ.get("FLOW_SIZE", "200")) - HEADER.size)
    burst = int(os.environ.get("FLOW_BURST", "1"))
    for n in range(count):
        target = start + (n // burst) * gap
        now = time.monotonic()
        if target > now:
            time.sleep(target - now)
        sock.sendto(HEADER.pack(n, time.monotonic_ns()) + pad, (address, port))
    print(json.dumps({"client_elapsed_s": round(time.monotonic() - start, 3), "planned_s": round(count * gap, 3), "sndbuf": sndbuf, "note": note}), flush=True)


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "server":
        server(sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), float(sys.argv[6]) if len(sys.argv) > 6 else 2.0)
    else:
        client(sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), float(sys.argv[6]), int(sys.argv[7]) if len(sys.argv) > 7 else 0)
