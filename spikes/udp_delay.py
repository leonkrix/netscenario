#!/usr/bin/env python3
"""Measures the one-way delay of UDP packets between two network namespaces on one machine.

All namespaces share one clock (CLOCK_MONOTONIC), so the delay is the difference of two timestamps,
no clock synchronisation involved. The packet carries its sequence number and its send time.

  udp_delay.py server ADDRESS PORT COUNT     prints a JSON line with the statistics, then exits
  udp_delay.py client ADDRESS PORT COUNT PPS sends COUNT packets at PPS packets per second
"""

import json
import socket
import statistics
import struct
import sys
import time

HEADER = struct.Struct("!IQ")  # sequence number, send time in nanoseconds


def percentile(sorted_values, fraction):
    if not sorted_values:
        return None
    index = min(len(sorted_values) - 1, int(round(fraction * (len(sorted_values) - 1))))
    return sorted_values[index]


def server(address, port, count):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((address, port))
    sock.settimeout(3.0)
    delays = []
    seen = set()
    print("ready", flush=True)
    try:
        while len(seen) < count:
            data, _ = sock.recvfrom(2048)
            received = time.monotonic_ns()
            if len(data) < HEADER.size:
                continue
            seq, sent = HEADER.unpack_from(data)
            if seq in seen:
                continue
            seen.add(seq)
            delays.append((received - sent) / 1e6)
    except socket.timeout:
        pass
    delays_sorted = sorted(delays)
    result = {
        "received": len(delays),
        "expected": count,
        "mean_ms": round(statistics.fmean(delays), 3) if delays else None,
        "stdev_ms": round(statistics.pstdev(delays), 3) if delays else None,
        "min_ms": round(delays_sorted[0], 3) if delays else None,
        "p50_ms": round(percentile(delays_sorted, 0.50), 3) if delays else None,
        "p99_ms": round(percentile(delays_sorted, 0.99), 3) if delays else None,
        "max_ms": round(delays_sorted[-1], 3) if delays else None,
    }
    print(json.dumps(result), flush=True)


def client(address, port, count, pps):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    interval = 1.0 / pps
    padding = b"\0" * 200
    start = time.monotonic()
    for seq in range(count):
        target = start + seq * interval
        while True:
            now = time.monotonic()
            if now >= target:
                break
            if target - now > 0.002:
                time.sleep(target - now - 0.001)
        sock.sendto(HEADER.pack(seq, time.monotonic_ns()) + padding, (address, port))


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "server":
        server(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    elif mode == "client":
        client(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5]))
    else:
        sys.exit("unknown mode " + mode)
