#!/usr/bin/env python3
"""TCP bulk flows for the spikes of release 0.3.

  tcpflow.py server ADDRESS PORT CONNECTIONS SECONDS   accepts CONNECTIONS connections, reads for SECONDS after the first
                                                       byte, prints one JSON line: bytes per second for each connection
  tcpflow.py client ADDRESS PORT SECONDS [CC]          sends for SECONDS; CC is the congestion control (a socket option);
                                                       prints a JSON line with TCP_INFO at the end and the sampling cost
  tcpflow.py cc                                        tries to select each congestion control on a socket
"""

import json
import socket
import struct
import sys
import threading
import time

TCP_INFO = 11
TCP_CONGESTION = 13
# struct tcp_info: 8 bytes of u8, then u32 fields; the offsets of the ones read here (linux/tcp.h)
FIELDS = ["state", "ca_state", "retransmits", "probes", "backoff", "options", "wscale", "flags"]
U32 = [
    "rto", "ato", "snd_mss", "rcv_mss", "unacked", "sacked", "lost", "retrans", "fackets",
    "last_data_sent", "last_ack_sent", "last_data_recv", "last_ack_recv", "pmtu", "rcv_ssthresh", "rtt", "rttvar",
    "snd_ssthresh", "snd_cwnd", "advmss", "reordering",
]


def tcp_info(sock):
    raw = sock.getsockopt(socket.IPPROTO_TCP, TCP_INFO, 256)
    info = dict(zip(FIELDS, raw[:8]))
    count = min(len(U32), (len(raw) - 8) // 4)
    info.update(zip(U32, struct.unpack_from(f"={count}I", raw, 8)))
    return info


def server(address, port, connections, seconds):
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((address, port))
    listener.listen(16)
    print("ready", flush=True)
    results = {}
    start = [None]
    lock = threading.Lock()

    def reader(index, conn):
        bins = {}
        conn.settimeout(5.0)
        try:
            while True:
                data = conn.recv(1 << 16)
                if not data:
                    break
                now = time.monotonic()
                with lock:
                    if start[0] is None:
                        start[0] = now
                second = int(now - start[0])
                bins[second] = bins.get(second, 0) + len(data)
                if now - start[0] > seconds + 3:
                    break
        except (socket.timeout, ConnectionError):
            pass
        results[index] = [bins.get(s, 0) for s in range(seconds)]
        conn.close()

    threads = []
    for index in range(connections):
        conn, _ = listener.accept()
        thread = threading.Thread(target=reader, args=(index, conn))
        thread.start()
        threads.append(thread)
    for thread in threads:
        thread.join()
    print(json.dumps({"bytes_per_second": [results[i] for i in sorted(results)]}), flush=True)


def client(address, port, seconds, cc):
    sock = socket.socket()
    note = ""
    if cc:
        try:
            sock.setsockopt(socket.IPPROTO_TCP, TCP_CONGESTION, cc.encode())
        except OSError as error:
            note = f"{cc}: {error}"
    sock.connect((address, port))
    used = sock.getsockopt(socket.IPPROTO_TCP, TCP_CONGESTION, 16).split(b"\0")[0].decode()
    block = b"x" * (1 << 16)
    end = time.monotonic() + seconds
    samples = []
    cost = 0.0
    next_sample = time.monotonic()
    while time.monotonic() < end:
        sock.sendall(block)
        if time.monotonic() >= next_sample:
            t0 = time.perf_counter()
            info = tcp_info(sock)
            cost += time.perf_counter() - t0
            samples.append((round(end - time.monotonic(), 2), info["snd_cwnd"], info["rtt"]))
            next_sample += 0.5
    info = tcp_info(sock)
    sock.close()
    print(json.dumps({"cc": used, "note": note, "final": {k: info[k] for k in ("snd_mss", "snd_cwnd", "rtt", "retrans", "lost") if k in info},
                      "samples": samples[:: max(1, len(samples) // 8)], "getsockopt_us": round(1e6 * cost / max(1, len(samples)), 1)}), flush=True)


def cc():
    out = {}
    for name in ("reno", "cubic", "bbr", "bic", "vegas", "dctcp", "nonexistent"):
        s = socket.socket()
        try:
            s.setsockopt(socket.IPPROTO_TCP, TCP_CONGESTION, name.encode())
            out[name] = s.getsockopt(socket.IPPROTO_TCP, TCP_CONGESTION, 16).split(b"\0")[0].decode()
        except OSError as error:
            out[name] = f"refused: {error.strerror}"
        s.close()
    print(json.dumps(out))


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "server":
        server(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]))
    elif mode == "client":
        client(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5] if len(sys.argv) > 5 else "")
    else:
        cc()
