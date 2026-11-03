#!/usr/bin/env python3
"""Steps a shaper through a staircase of rates and judges how well a receiver's arrivals follow it (spike S19).

  trace_steps.py drive PID IFACE ARM STEP_MS SECONDS LOW_KBIT HIGH_KBIT T0_FILE
      enters the network namespace of PID, starts one `tc -batch -`, and at absolute CLOCK_MONOTONIC moments (1 s after
      the start) writes a rate change every STEP_MS, alternating HIGH and LOW. Writes the moment of the first step to
      T0_FILE and prints the lateness of the writes. ARM is tbf (a tbf root, burst 3500), tbf-min (burst 1600), htb (one class) or netem (a netem root with a rate).
  trace_steps.py analyze DUMP T0_FILE STEP_MS SECONDS LOW_KBIT HIGH_KBIT FRAME_BYTES
      reads the arrival moments (nanoseconds) and compares the bytes that arrived in each step with the rate x time.
"""

import json
import subprocess
import sys
import time


def command(arm, iface, kbit):
    if arm == "tbf":
        return f"qdisc change dev {iface} root handle 1: tbf rate {kbit}kbit burst 3500 limit 400000\n"
    if arm == "tbf-min":
        return f"qdisc change dev {iface} root handle 1: tbf rate {kbit}kbit burst 1600 limit 400000\n"
    if arm == "htb2":  # an htb with the handle 2: below a netem root (handle 1:)
        return f"class change dev {iface} parent 2: classid 2:10 htb rate {kbit}kbit ceil {kbit}kbit burst 1600 cburst 1600\n"
    if arm == "htb":
        return f"class change dev {iface} parent 1: classid 1:10 htb rate {kbit}kbit ceil {kbit}kbit burst 1600 cburst 1600\n"
    return f"qdisc change dev {iface} root handle 1: netem limit 100000 rate {kbit}kbit\n"


def drive(pid, iface, arm, step_ms, seconds, low, high, t0_file):
    proc = subprocess.Popen(["nsenter", "-t", pid, "-n", "tc", "-batch", "-"], stdin=subprocess.PIPE, text=True)
    steps = int(seconds * 1000 / step_ms)
    t0 = time.monotonic_ns() + 1_000_000_000
    with open(t0_file, "w") as handle:
        handle.write(str(t0))
    late = []
    for k in range(steps):
        moment = t0 + k * step_ms * 1_000_000
        while True:
            now = time.monotonic_ns()
            if now >= moment:
                break
            gap = (moment - now) / 1e9
            time.sleep(gap - 0.0002 if gap > 0.0005 else 0)
        proc.stdin.write(command(arm, iface, high if k % 2 == 0 else low))
        proc.stdin.flush()
        late.append((time.monotonic_ns() - moment) / 1e6)
    proc.stdin.close()
    cpu_before = None
    try:
        with open(f"/proc/{proc.pid}/stat") as handle:
            fields = handle.read().split()
            cpu_before = (int(fields[13]) + int(fields[14])) / 100.0
    except OSError:
        pass
    proc.wait()
    late.sort()
    print(json.dumps({"steps": steps, "write_lateness_ms": {"p50": round(late[len(late) // 2], 3), "p99": round(late[int(len(late) * 0.99)], 3), "max": round(late[-1], 3)}}))


def analyze(dump, t0_file, step_ms, seconds, low, high, frame):
    arrivals = [int(line) for line in open(dump).read().split()]
    t0 = int(open(t0_file).read())
    steps = int(seconds * 1000 / step_ms)
    delivered = [0] * steps
    for moment in arrivals:
        k = (moment - t0) // (step_ms * 1_000_000)
        if 0 <= k < steps:
            delivered[k] += frame
    expected = [(high if k % 2 == 0 else low) * 1000 / 8 * step_ms / 1000 for k in range(steps)]
    hi = [d / e for d, e, k in zip(delivered, expected, range(steps)) if k % 2 == 0]
    lo = [d / e for d, e, k in zip(delivered, expected, range(steps)) if k % 2 == 1]
    absolute = sum(abs(d - e) for d, e in zip(delivered, expected)) / sum(expected)
    total = sum(delivered) / sum(expected)
    print(json.dumps({
        "step_ms": step_ms, "steps": steps,
        "total_delivered_over_expected": round(total, 3),
        "mean_abs_error_per_step": round(absolute, 3),
        "high_steps_delivered_over_expected": round(sum(hi) / len(hi), 3),
        "low_steps_delivered_over_expected": round(sum(lo) / len(lo), 3),
    }))


if __name__ == "__main__":
    if sys.argv[1] == "drive":
        drive(sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5]), float(sys.argv[6]), int(sys.argv[7]), int(sys.argv[8]), sys.argv[9])
    else:
        analyze(sys.argv[2], sys.argv[3], int(sys.argv[4]), float(sys.argv[5]), int(sys.argv[6]), int(sys.argv[7]), int(sys.argv[8]))
