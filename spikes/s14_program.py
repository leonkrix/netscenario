#!/usr/bin/env python3
"""S14: a program of the user as a node.

`pid` is a process that holds the network namespace of the node (the "holder" of the backend). Programs are started
the way the backend starts them (`nsenter --net=/proc/<pid>/ns/net -- program`) and looked at from outside.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import tempfile
import time

HOLDER = sys.argv[1] if len(sys.argv) > 1 else ""
NODE_ADDRESS = "10.9.0.2"


def report(kind: str, ident: str, text: str) -> None:
    print(f"{kind} {ident:<7} {text}", flush=True)


def start(argv: list[str], **kw) -> subprocess.Popen:
    # The same shape as Provisioner::start_program (a new process group is the proposal being tested)
    return subprocess.Popen(["nsenter", f"--net=/proc/{HOLDER}/ns/net", "--", *argv], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kw)


def percentile(values: list[float], p: float) -> float:
    v = sorted(values)
    return v[min(len(v) - 1, max(0, int(-(-p * len(v) // 100)) - 1))]


# --- probes: /proc/<holder>/net/{udp,tcp} ------------------------------------------------------------------------


def listening(protocol: str, port: int, address: str | None = None) -> bool:
    """Is something bound (UDP, state 07) or listening (TCP, state 0A) on this port in the namespace of the holder?

    Read from outside the namespace: /proc/<pid>/net is the table of the namespace of that process."""
    wanted_state = "07" if protocol == "udp" else "0A"
    try:
        with open(f"/proc/{HOLDER}/net/{protocol}") as table:
            next(table)
            for line in table:
                fields = line.split()
                local, state = fields[1], fields[3]
                host, _, hex_port = local.partition(":")
                if int(hex_port, 16) != port or state != wanted_state:
                    continue
                if address is None:
                    return True
                packed = bytes.fromhex(host)[::-1]  # little endian words
                if ".".join(str(b) for b in packed) in (address, "0.0.0.0"):
                    return True
    except FileNotFoundError:
        return False
    return False


SERVER = """
import socket, sys, time
kind, host, port, delay = sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4])
time.sleep(delay)
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM if kind == 'udp' else socket.SOCK_STREAM)
s.bind((host, port))
if kind == 'tcp':
    s.listen(8)
print(time.monotonic_ns(), flush=True)
time.sleep(60)
"""


def probes() -> None:
    for protocol in ("udp", "tcp"):
        for host in ("0.0.0.0", NODE_ADDRESS):
            lags = []
            wrong = 0
            for trial in range(20):
                port = 6000 + trial
                if listening(protocol, port):
                    wrong += 1  # a port that nobody uses must not look ready
                program = start([sys.executable, "-c", SERVER, protocol, host, str(port), "0.15"])
                began = time.monotonic_ns()
                while not listening(protocol, port, NODE_ADDRESS):
                    time.sleep(0.005)
                    if time.monotonic_ns() - began > 3_000_000_000:
                        break
                seen = time.monotonic_ns()
                assert program.stdout is not None
                bound = int(program.stdout.readline())
                lags.append((seen - bound) / 1e6)
                program.kill()
                program.wait()
            ok = wrong == 0 and all(lag >= -1 for lag in lags)
            report("PASS" if ok else "FAIL", "S14.1", f"ready probe for {protocol} bound to {host}, polled every 5 ms from outside the namespace: detected {percentile(lags, 50):.1f} ms (p50), {percentile(lags, 99):.1f} ms (p99) after the bind; false 'ready' for unused ports: {wrong}")
    # A UDP port of the *other* namespace must not count (the table is per namespace)
    program = subprocess.Popen([sys.executable, "-c", SERVER, "udp", "0.0.0.0", "6100", "0"], stdout=subprocess.PIPE)
    assert program.stdout is not None
    program.stdout.readline()
    outside = listening("udp", 6100)
    program.kill()
    program.wait()
    report("PASS" if not outside else "FAIL", "S14.1", "canary: a port that is bound in the namespace of the supervisor is not reported as ready in the node")


# --- the user ---------------------------------------------------------------------------------------------------


def identity() -> None:
    with tempfile.TemporaryDirectory() as directory:
        program = start([sys.executable, "-c", f"import os; open({directory + '/made'!r}, 'w').write('x'); print(os.getuid(), os.getgid())"])
        out, err = program.communicate()
        inside = out.decode().split()
        owner = os.stat(directory + "/made").st_uid
        report(
            "PASS" if owner == os.getuid() else "FAIL",
            "S14.2",
            f"the program sees uid/gid {' '.join(inside)}; a file it creates belongs to uid {owner} (the invoking user is {os.getuid()}): {'the invoking user' if owner == os.getuid() else 'somebody else'}",
        )
        environment = start([sys.executable, "-c", "import os; print(os.environ.get('NETSCENARIO_PROBE', 'unset'), os.getcwd())"], env={**os.environ, "NETSCENARIO_PROBE": "yes"}, cwd=directory)
        out, _ = environment.communicate()
        ok = out.decode().split()[0] == "yes" and out.decode().split()[1] == directory
        report("PASS" if ok else "FAIL", "S14.2", f"environment and working directory reach the program through nsenter: {out.decode().strip()}")


# --- stop -------------------------------------------------------------------------------------------------------

STUBBORN = """
import signal, subprocess, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen(['sleep', '1000'])
print(child.pid, flush=True)
while True:
    time.sleep(1)
"""


def alive(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def stop() -> None:
    # What the backend does now: kill the one process it started. A child of the program is left behind.
    program = start([sys.executable, "-c", STUBBORN])
    assert program.stdout is not None
    grandchild = int(program.stdout.readline())
    program.kill()
    program.wait()
    time.sleep(0.2)
    left = alive(grandchild)
    report("INFO", "S14.3", f"kill of the one process that was started (what the backend does): a child of the program {'is still running' if left else 'is gone'}")
    if left:
        os.kill(grandchild, signal.SIGKILL)

    # The proposal: a process group of its own, TERM to the group, KILL after the grace time
    program = start([sys.executable, "-c", STUBBORN], process_group=0)
    assert program.stdout is not None
    grandchild = int(program.stdout.readline())
    group = os.getpgid(program.pid)
    began = time.monotonic()
    os.killpg(group, signal.SIGTERM)
    try:
        program.wait(timeout=0.5)
        term_worked = True
    except subprocess.TimeoutExpired:
        term_worked = False
    os.killpg(group, signal.SIGKILL)
    program.wait()
    elapsed = time.monotonic() - began
    time.sleep(0.2)
    report(
        "PASS" if not alive(grandchild) and not term_worked else "FAIL",
        "S14.3",
        f"a program that ignores TERM: TERM {'worked' if term_worked else 'was ignored'}, KILL to the process group after 0.5 s ended it and its child ({elapsed:.2f} s in all); child left: {alive(grandchild)}",
    )
    # A program that obeys TERM ends at once and says how
    polite = start([sys.executable, "-c", "import time; time.sleep(60)"], process_group=0)
    began = time.monotonic()
    os.killpg(os.getpgid(polite.pid), signal.SIGTERM)
    code = polite.wait()
    report("PASS" if code == -signal.SIGTERM else "FAIL", "S14.3", f"a program that obeys TERM ends in {(time.monotonic() - began) * 1000:.1f} ms with status {code} (the signal, as the result reports it)")


# --- output -----------------------------------------------------------------------------------------------------

FLOOD = """
import sys
line = 'x' * 1000 + '\\n'
while True:
    sys.stdout.write(line)
"""

NO_NEWLINE = """
import sys
while True:
    sys.stdout.write('y' * 65536)
"""


def rss_kib(pid: int) -> int:
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith("VmRSS"):
                return int(line.split()[1])
    return 0


def output() -> None:
    # A reader that keeps the first MiB and drains the rest, so that the program is never blocked or killed for being chatty
    program = start([sys.executable, "-c", FLOOD])
    assert program.stdout is not None
    kept = 0
    total = 0
    began = time.monotonic()
    while time.monotonic() - began < 2.0:
        chunk = program.stdout.read1(65536)
        total += len(chunk)
        kept = min(kept + len(chunk), 1024 * 1024)
    program.kill()
    program.wait()
    report("INFO", "S14.4", f"a program that prints without pause: a reader that keeps 1 MiB and drains the rest takes {total / 2 / 1e6:.0f} MB/s; the program keeps running (it is not stopped for printing)")

    # What the supervisor does now: read_until(b'\\n') into a Vec, without a limit for the line. The same with Python's readline.
    program = start([sys.executable, "-c", NO_NEWLINE])
    assert program.stdout is not None
    me = os.getpid()
    base = rss_kib(me)
    began = time.monotonic()
    buffer = bytearray()
    while time.monotonic() - began < 1.0:
        buffer += program.stdout.read1(65536)
    grown = (rss_kib(me) - base) / 1024
    program.kill()
    program.wait()
    report("WARN", "S14.4", f"a program that prints without a newline: a reader that waits for the end of the line (what read_until in the supervisor does) held {len(buffer) / 1e6:.0f} MB after one second ({grown:.0f} MiB more resident); the supervisor needs a limit for the length of a line")


def main() -> None:
    report("INFO", "S14.0", f"holder of the node: process {HOLDER}; python {sys.version.split()[0]}; uid {os.getuid()}")
    probes()
    identity()
    stop()
    output()


if __name__ == "__main__":
    main()
