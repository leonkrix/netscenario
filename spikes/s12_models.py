#!/usr/bin/env python3
"""S12: the models of netem against their theory.

For each model: the parameters as `tc` takes them, what the mathematics of the model says, and what packets (three
seeds) showed. A packet is `crc32 | sequence | time sent | padding`, where the CRC covers everything after it, so a
datagram with a flipped bit is told from a good one wherever the bit is.

Run by s12-netem-models.sh inside the namespace that holds v0; the receiver runs in the namespace of v1.
"""

from __future__ import annotations

import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib

N = int(os.environ.get("S12_PACKETS", "100000"))
SEEDS = (11, 22, 33)
GAP_NS = 50_000  # 20,000 packets per second: the Python receiver keeps up, and the socket buffer is enough
PORT = 5000
HEADER = 16  # crc 4, sequence 4, time sent 8


def report(kind: str, ident: str, text: str) -> None:
    print(f"{kind} {ident:<7} {text}", flush=True)


def tc(*args: str) -> None:
    subprocess.run(["tc", *args], check=True)


# ---------------------------------------------------------------------------------------------------------------
# The receiver (runs in the other namespace)
# ---------------------------------------------------------------------------------------------------------------


def receive(out: str) -> None:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4 * 1024 * 1024)
    sock.bind(("10.9.0.2", PORT))
    sock.settimeout(10.0)
    print("ready", flush=True)
    seen = []  # (sequence, intact, length, delay in ns), in the order of arrival
    first = True
    while True:
        try:
            data = sock.recv(4096)
        except TimeoutError:
            break
        arrived = time.monotonic_ns()
        if len(data) < HEADER:
            seen.append((-1, False, len(data), 0))
        else:
            crc, seq, sent = struct.unpack("!IIQ", data[:HEADER])
            seen.append((seq, zlib.crc32(data[4:]) == crc, len(data), arrived - sent))
        if first:
            sock.settimeout(0.6)
            first = False
    with open(out, "w") as f:
        json.dump(seen, f)


# ---------------------------------------------------------------------------------------------------------------
# One measurement
# ---------------------------------------------------------------------------------------------------------------


def counters(pid: str) -> dict[str, int]:
    """What the second namespace counted for IP and UDP (a corrupted header is dropped there)."""
    text = subprocess.run(["nsenter", "-t", pid, "-n", "cat", "/proc/net/snmp"], capture_output=True, text=True, check=True).stdout
    result: dict[str, int] = {}
    rows = {}
    for line in text.splitlines():
        proto, _, rest = line.partition(":")
        rows.setdefault(proto, []).append(rest.split())
    for proto in ("Ip", "Udp"):
        names, values = rows[proto]
        for name, value in zip(names, values):
            result[f"{proto}.{name}"] = int(value)
    return result


def measure(pid: str, netem: str, seed: int, size: int = 200, count: int = N, no_check: bool = False) -> dict:
    # A fresh qdisc for every measurement: netem keeps corrupt, reorder, rate and slot across a change (D-048), and a
    # corruption left over from an earlier section made the first run on the CI runners lose 18.7 % instead of 10 %
    # and duplicate 16.3 % instead of 20 % in the correlation section
    subprocess.run(["tc", "qdisc", "del", "dev", "v0", "root"], capture_output=True)
    tc("qdisc", "add", "dev", "v0", "root", "netem", "limit", "100000", *netem.split(), "seed", str(seed))
    out = os.path.join(tempfile.gettempdir(), f"s12-{os.getpid()}.json")
    receiver = subprocess.Popen(
        ["nsenter", "-t", pid, "-n", sys.executable, os.path.abspath(__file__), "recv", out], stdout=subprocess.PIPE, text=True
    )
    assert receiver.stdout is not None and receiver.stdout.readline().strip() == "ready"
    before = counters(pid)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    if no_check:
        sock.setsockopt(socket.SOL_SOCKET, 11, 1)  # SO_NO_CHECK: send with the UDP checksum 0 ("not computed")
    sock.bind(("10.9.0.1", 0))
    sock.connect(("10.9.0.2", PORT))
    pad = bytes([0xA5]) * (size - HEADER)
    start = time.perf_counter_ns()
    for n in range(count):
        due = start + n * GAP_NS  # pace the stream: the receiver is Python
        while time.perf_counter_ns() < due:
            pass
        body = struct.pack("!IQ", n, time.monotonic_ns()) + pad
        sock.send(struct.pack("!I", zlib.crc32(body)) + body)
    receiver.wait()
    after = counters(pid)
    with open(out) as f:
        seen = json.load(f)
    os.unlink(out)
    arrived: dict[int, int] = {}  # sequence -> delay in ns, the first intact arrival
    duplicates = corrupted = 0
    highest = -1
    reordered = 0
    for seq, intact, _length, delay in seen:
        if not intact:
            corrupted += 1
            continue
        if seq in arrived:
            duplicates += 1
            continue
        arrived[seq] = delay
        if seq < highest:
            reordered += 1
        highest = max(highest, seq)
    return {
        "lost": [n for n in range(count) if n not in arrived],
        "delays": arrived,
        "duplicates": duplicates,
        "corrupted": corrupted,
        "datagrams": len(seen),
        "count": count,
        "reordered": reordered,
        "delta": {k: after[k] - before[k] for k in after},
    }


def runs_of_loss(lost: list[int]) -> list[int]:
    runs, length, previous = [], 0, None
    for n in lost:
        if previous is not None and n == previous + 1:
            length += 1
        else:
            if length:
                runs.append(length)
            length = 1
        previous = n
    if length:
        runs.append(length)
    return runs


def mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else float("nan")


def stationary(matrix: list[list[float]]) -> list[float]:
    """The stationary distribution of a Markov chain, by repeated multiplication."""
    n = len(matrix)
    pi = [1.0 / n] * n
    for _ in range(200_000):
        nxt = [sum(pi[i] * matrix[i][j] for i in range(n)) for j in range(n)]
        if max(abs(a - b) for a, b in zip(nxt, pi)) < 1e-15:
            return nxt
        pi = nxt
    return pi


def judge(ident: str, name: str, theory: float, measured: list[float], what: str, scale: float = 1.0, unit: str = "%", relative: float = 0.02, burst: bool = False) -> bool:
    """PASS if the mean over the seeds is within three times the spread between the seeds (or a small relative band)."""
    value = mean(measured)
    spread = max(measured) - min(measured)
    tolerance = max(3 * spread, relative * abs(theory), 0.0005 if not burst else 0.02)
    ok = abs(value - theory) <= tolerance
    seeds = ", ".join(f"{x * scale:.3f}" for x in measured)
    report("PASS" if ok else "FAIL", ident, f"{name}: {what} theory {theory * scale:.3f}{unit} measured {value * scale:.3f}{unit} (seeds {seeds})")
    return ok


# ---------------------------------------------------------------------------------------------------------------
# The models
# ---------------------------------------------------------------------------------------------------------------


def loss_models(pid: str) -> None:
    def seeds(netem: str, **kw) -> list[dict]:
        return [measure(pid, netem, s, **kw) for s in SEEDS]

    control = seeds("delay 1ms")
    lost = max(len(m["lost"]) for m in control)
    report("PASS" if lost == 0 else "FAIL", "S12.0", f"control, no loss configured: {lost} lost packets in {N} (the pacing and the receiver keep up)")

    for percent in (1, 10, 30):
        p = percent / 100
        runs = seeds(f"loss random {percent}%")
        judge("S12.1", f"loss random {percent}%", p, [len(m["lost"]) / N for m in runs], "loss", 100)
        judge("S12.1", f"loss random {percent}%", 1 / (1 - p), [mean(runs_of_loss(m["lost"])) for m in runs], "mean burst", 1, " packets", 0.03, True)

    # Gilbert-Elliott. The manual names the arguments p, r, 1-h, 1-k. Two readings are compared with the measurement:
    #   A: the 3rd argument is the loss probability in the bad state, the 4th the loss probability in the good state
    #   B: it is the complement of that (h and k, as the letters alone would suggest)
    for p, r, loss_bad, loss_good in ((0.01, 0.20, 1.0, 0.0), (0.02, 0.10, 0.7, 0.02), (0.05, 0.30, 0.5, 0.0)):
        pi_bad = p / (p + r)
        a = pi_bad * loss_bad + (1 - pi_bad) * loss_good
        b = pi_bad * (1 - loss_bad) + (1 - pi_bad) * (1 - loss_good)
        netem = f"loss gemodel {p * 100:g}% {r * 100:g}% {loss_bad * 100:g}% {loss_good * 100:g}%"
        runs = seeds(netem)
        measured = mean([len(m["lost"]) / N for m in runs])
        reading = "A" if abs(measured - a) < abs(measured - b) else "B"
        report("INFO", "S12.2", f"{netem}: reading A predicts {a * 100:.3f}%, reading B {b * 100:.3f}%, measured {measured * 100:.3f}%: the measurement agrees with reading {reading}")
        judge("S12.2", netem, a, [len(m["lost"]) / N for m in runs], "loss", 100)
        if loss_bad == 1.0 and loss_good == 0.0:
            judge("S12.2", netem, 1 / r, [mean(runs_of_loss(m["lost"])) for m in runs], "mean burst (the stay in the bad state)", 1, " packets", 0.03, True)

    # The four-state model: p13 p31 p32 p23 p14. States: 1 receiving in a gap, 2 receiving inside a burst,
    # 3 losing inside a burst, 4 losing a single packet in a gap (the manual's names).
    for p13, p31, p32, p23, p14 in ((0.02, 0.30, 0.20, 0.10, 0.01), (0.05, 0.50, 0.10, 0.20, 0.02)):
        m = [[1 - p13 - p14, 0, p13, p14], [0, 1 - p23, p23, 0], [p31, p32, 1 - p31 - p32, 0], [1, 0, 0, 0]]
        pi = stationary(m)
        loss = pi[2] + pi[3]
        burst = loss / (pi[0] * (p13 + p14) + pi[1] * p23)
        netem = f"loss state {p13 * 100:g}% {p31 * 100:g}% {p32 * 100:g}% {p23 * 100:g}% {p14 * 100:g}%"
        runs = seeds(netem)
        judge("S12.3", netem, loss, [len(m["lost"]) / N for m in runs], "loss", 100)
        judge("S12.3", netem, burst, [mean(runs_of_loss(m["lost"])) for m in runs], "mean burst", 1, " packets", 0.03, True)


def reorder_and_duplicate(pid: str) -> None:
    count = N // 5
    # `reorder P C` without `gap`: tc uses gap 1, each packet is sent on at once with probability P and the rest are
    # held for the delay; the early ones overtake the held ones, which is the reordering. With `gap g` the g-1
    # packets after an early one are held, then each is early with probability P: the share is 1 / (g - 1 + 1/P).
    for percent, corr, gap in ((25, 0, None), (25, 0, 5), (50, 0, 3), (25, 50, None), (25, 50, 5)):
        p = percent / 100
        netem = f"delay 10ms reorder {percent}% {corr}%" + (f" gap {gap}" if gap else "")
        theory = p if gap is None else 1 / (gap - 1 + 1 / p)
        runs = [measure(pid, netem, s, count=count) for s in SEEDS]
        early = [sum(1 for d in m["delays"].values() if d < 5_000_000) / len(m["delays"]) for m in runs]
        if corr == 0:
            judge("S12.4", netem, theory, early, "share of packets sent on at once", 100)
        else:
            report("INFO", "S12.4", f"{netem}: share of packets sent on at once, without correlation it would be {theory * 100:.2f}%, measured {mean(early) * 100:.2f}% (seeds {', '.join(f'{x * 100:.2f}' for x in early)})")
        report("INFO", "S12.4", f"{netem}: packets that arrived after a packet with a higher number (RFC 4737): {mean([m['reordered'] / count for m in runs]) * 100:.2f}% of the packets")

    for percent, corr in ((5, 0), (20, 0), (20, 50)):
        netem = f"duplicate {percent}% {corr}%"
        runs = [measure(pid, netem, s, count=count) for s in SEEDS]
        shares = [m["duplicates"] / count for m in runs]
        if corr == 0:
            judge("S12.5", netem, percent / 100, shares, "share of packets that arrived twice", 100)
        else:
            report("INFO", "S12.5", f"{netem}: share of packets that arrived twice, without correlation {percent}%, measured {mean(shares) * 100:.2f}%")


def corruption(pid: str) -> None:
    """`corrupt P` flips one bit in a random byte of the frame (S13b found that the Ethernet header is among the bytes). What the
    receiver sees depends on the checksums: with the UDP checksum on (the default of every program) the first run of
    this spike found that no corrupted payload ever arrived. With the checksum off (SO_NO_CHECK) the payload arrives
    with the flipped bit, and the CRC in the packet tells."""
    count = N // 5
    for no_check in (False, True):
        for size in (64, 200, 1200):
            netem = "corrupt 10%"
            runs = [measure(pid, netem, s, size=size, count=count, no_check=no_check) for s in SEEDS]
            p = 0.10
            label = f"{netem}, {size} byte payload, UDP checksum {'off' if no_check else 'on'}"
            visible = [m["corrupted"] / count for m in runs]
            if no_check:
                # a flip in the payload arrives corrupted; one in the IP header is dropped by the header checksum; in
                # the source port or the checksum field it arrives intact; in the length or the destination port it is dropped
                # the frame is the Ethernet header (14), IP (20), UDP (8) and the payload: netem picks a byte of all of it
                judge("S12.6", label, p * size / (size + 42), visible, "share that arrived with a corrupted payload", 100, relative=0.05)
            else:
                judge("S12.6", label, 0.0, visible, "share that arrived with a corrupted payload", 100)
            gone = [1 - m["datagrams"] / count for m in runs]
            ip = [m["delta"]["Ip.InHdrErrors"] / count for m in runs]
            csum = [m["delta"].get("Udp.InCsumErrors", 0) / count for m in runs]
            udp = [(m["delta"]["Udp.InErrors"] + m["delta"]["Udp.NoPorts"]) / count for m in runs]
            report(
                "INFO",
                "S12.6",
                f"{label}: {mean(gone) * 100:.2f}% of the packets never arrived; the receiver counted IP header errors {mean(ip) * 100:.2f}%, UDP checksum errors {mean(csum) * 100:.2f}%, all UDP errors and unknown ports {mean(udp) * 100:.2f}%",
            )


def correlations(pid: str) -> None:
    """The second argument of `loss random`, `duplicate` and `reorder` is a correlation. Does it keep the mean?"""
    for what, base in (("loss random", 10), ("duplicate", 20), ("reorder", 25)):
        for corr in (0, 50, 90):
            extra = "delay 10ms " if what == "reorder" else ""
            netem = f"{extra}{what} {base}% {corr}%"
            count = N
            runs = [measure(pid, netem, s, count=count) for s in SEEDS]
            if what == "loss random":
                share = [len(m["lost"]) / count for m in runs]
                burst = mean([mean(runs_of_loss(m["lost"])) for m in runs])
                text = f"share lost {mean(share) * 100:.2f}% (asked {base}%), mean burst {burst:.3f} packets (uncorrelated {1 / (1 - base / 100):.3f})"
            elif what == "duplicate":
                share = [m["duplicates"] / count for m in runs]
                text = f"share duplicated {mean(share) * 100:.2f}% (asked {base}%) (seeds {', '.join(f'{x * 100:.2f}' for x in share)})"
            else:
                share = [sum(1 for d in m["delays"].values() if d < 5_000_000) / len(m["delays"]) for m in runs]
                text = f"share sent on at once {mean(share) * 100:.2f}% (asked {base}%)"
            report("INFO", "S12.7", f"{netem}: {text}")


def duplicate_check(pid: str) -> None:
    """Why did `duplicate 20%` give 20.1 % in 20,000 packets and 16.3 % in 100,000? Look at what the receiver lost."""
    for count in (20000, 100000):
        for gap_ns in (50_000, 100_000):
            global GAP_NS
            GAP_NS = gap_ns
            m = measure(pid, "duplicate 20%", 11, count=count)
            d = m["delta"]
            report(
                "INFO",
                "S12.8",
                f"duplicate 20%, {count} packets, one every {gap_ns // 1000} us: duplicates {m['duplicates'] / count * 100:.2f}%, datagrams that reached the program {m['datagrams']} (the queue sent {int(count * 1.2)} expected), "
                f"packets never seen {len(m['lost'])}, Udp.RcvbufErrors {d.get('Udp.RcvbufErrors', 0)}, Udp.InDatagrams {d.get('Udp.InDatagrams', 0)}",
            )
    GAP_NS = 50_000


def main() -> None:
    pid = sys.argv[1]
    report("INFO", "S12.0", f"{N} packets per loss run, seeds {SEEDS}, one packet every {GAP_NS / 1000:.0f} us")
    only = os.environ.get("S12_ONLY", "")
    if only in ("", "loss"):
        loss_models(pid)
    if only in ("", "reorder"):
        reorder_and_duplicate(pid)
    if only in ("", "corruption"):
        corruption(pid)
    if only in ("", "correlation"):
        correlations(pid)
    if only in ("", "dupcheck"):
        duplicate_check(pid)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "recv":
        receive(sys.argv[2])
    else:
        main()
