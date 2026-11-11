# Troubleshooting

Start with the machine: **`netscenario doctor`** (or `ns.doctor()` in Python) tests everything that a run needs and prints a `fix:` line for each problem ([DOCTOR.md](DOCTOR.md)). The exit codes of the commands say what kind of problem it is ([CLI.md](CLI.md)):

| Exit code | Kind | Where to look |
| --- | --- | --- |
| 2 | The command line or the scenario is wrong, or the backend cannot reproduce the scenario exactly | [The scenario](#the-scenario-is-refused-exit-code-2) |
| 3 | The machine does not allow the run | [The machine](#the-machine-does-not-allow-the-run-exit-code-3) |
| 4 | The run started and failed | [A run that fails](#a-run-that-fails-exit-code-4) |

## The machine does not allow the run (exit code 3)

The message ends with *run `netscenario doctor` to see what this machine can do and how to fix what it cannot*. Run it. The usual causes, in the order of how often they happen:

**`can-create-namespaces` fails.** No way to create network namespaces works. The `fix:` line names what applies to your machine: on Ubuntu 24.04 and newer the AppArmor setting, on some Debian and Arch kernels `kernel.unprivileged_userns_clone`, `user.max_user_namespaces` if it is 0, in a container the capability and the seccomp profile. The full explanation, with the cost of each setting, is [PERMISSIONS.md](PERMISSIONS.md). The quick ways out: `sudo netscenario run ...` (root needs no user namespace), or on Ubuntu 24.04:

```bash
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
```

**A tool is missing** (`tool-ip`, `tool-tc`, `tool-unshare`, `tool-nsenter`, `tool-prlimit`). Install the package the `fix:` line names: `iproute2` for `ip` and `tc`, `util-linux` for the others (`sudo apt install iproute2 util-linux` on Debian and Ubuntu). The tools are looked for in the `PATH` and in `/usr/sbin`, `/sbin`, `/usr/local/sbin`, `/usr/bin` and `/bin`; the line says where it looked. Under `sudo` the `PATH` is not yours: see [PERMISSIONS.md](PERMISSIONS.md).

**`veth` or `netem` fails.** The kernel modules are not there. `sudo modprobe veth` and `sudo modprobe sch_netem`. On Ubuntu cloud kernels `sch_netem` is in `linux-modules-extra-$(uname -r)`. In a container the modules come from the host.

**`the agent ... was not found; it belongs next to the netscenario program`.** A run needs two programs, `netscenario` and `netscenario-agent`, in the same directory. `cargo install netscenario-cli netscenario-agent` and `cargo build` put both there; if you copy `netscenario` somewhere, copy the agent too. In Python the wheel carries both. From a checkout set `NETSCENARIO_BIN_DIR` to the folder with both (the error `ProgramsNotFoundError` lists the places it looked).

**`seed of netem` is a warning.** The `tc` is older than iproute2 6.6.0 (Ubuntu 22.04 has 5.15, Ubuntu 24.04 has 6.1.0). A scenario with a `seed` and loss is refused (below); one without is not reproducible, and the result says so. Install a newer iproute2, or leave out `seed`.

**`jitter tables` is a warning.** A `tc` that you built yourself looks for `normal.dist` in `/usr/lib/tc`. Point `TC_LIB_DIR` to the `netem` folder of its build. Without it a jitter with `distribution: normal` fails when the run applies it.

## Networks, IPv6, TCP and files (release 0.3)

`netscenario doctor` has one check for each of these, and a scenario that needs what the machine lacks is refused before anything is created, with the feature, the backend and a way out.

**Routers and several links are refused (`forwarding and routes in a node` fails).** A network of more than two nodes needs a node that can switch on `ip_forward` and have a route through a next hop. A point-to-point scenario still runs. The usual cause is a container or a kernel that does not let a node write `/proc/sys/net`; run in a user namespace as in [PERMISSIONS.md](PERMISSIONS.md), or with the rights named there. Nothing is changed on the host: the setting is made inside the namespace of the node.

**`ip: v6` or `ip: dual` is refused (`IPv6 in a node` warns).** The kernel has no IPv6: `/proc/sys/net/ipv6` is missing, usually because it was started with `ipv6.disable=1`. Remove that option, or use IPv4 (`ip: v4`, the default). Everything that does not ask for IPv6 runs.

**TCP workloads are refused for `segment size of a link`.** The `ip` of the machine cannot set `gso_max_size` and `gso_max_segs` on an interface; iproute2 4.12 or newer is needed. Without it `netem` would see super-packets of up to 64 KB and the delay and loss of a TCP flow would be wrong, so the run is refused and not approximated.

**`this machine does not offer the congestion control 'bbr' to a socket`.** `cc:` names a congestion control that the kernel has not loaded. The message lists the ones it has (`reno` and `cubic` are always there); `sudo modprobe tcp_bbr` loads another. A socket in a user namespace can use only what is loaded, and a file that does not name `cc` gets `cubic` whatever the default of the machine is, so that it means one thing on two machines.

**A file the scenario refers to is refused (`cannot read '...'`).** A file of delay samples, a trace or a table is read once when the scenario is loaded, from the directory of the scenario file, at most 16 MiB. A path that leads out of that directory (`..`, an absolute path, a link) is refused; `--allow-outside-files` (Python: `allow_outside_files=True`) lifts it. The scenario of a run keeps what was read, so a replay needs no file.

**`a run may use at most 8 network namespaces (SAF-02)`.** A run has a namespace for each node, hosts and routers alike, and no more than 8. Cut the scenario into runs of at most 8 nodes (the pieces that do not interact can run apart), or fewer routers.

## The scenario is refused (exit code 2)

A scenario with a mistake is refused before anything is created, with the line and column, the path in the file, and a hint:

```text
error: `both` cannot be combined with `forward` or `reverse`
 --> scenarios/invalid/both-and-forward.yaml:9:13
  |
8 |       between: [client, server]
9 |       both: { delay: 10ms }
  |             ^
  = path: topology.links[0].both
  = help: use `both` for the same values in both directions, or `forward` and `reverse` for different ones
```

`netscenario validate file.yaml` checks a file without running it, and works on any machine. The mistakes people make most often:

- **A number without a unit** (`delay: 40`) or a probability without a percent sign (`p: 0.01`). A bare number could be seconds or milliseconds, so it is refused. Write `40ms` and `1%`.
- **A key of a later release** (a loss model such as `trace`, a workload kind such as `container`). The error says in which release it is planned ([FEATURES.md](../FEATURES.md)).
- **`repetitions` in the file.** It is an option of a run, not a key: `netscenario run --repetitions 10`, or `repetitions=10` in Python ([STATISTICS.md](STATISTICS.md)).
- **A misspelled key, value, unit or name**: the error lists the keys that are allowed there (`unknown field `dealy`, expected one of delay, jitter, loss, rate`) and, if exactly one name is close, asks `did you mean 'delay'?`. The same holds for the values of an enumeration, for a unit (`Mbit`, `min`, `mbps`) and for a node that does not exist. It is a guess: read the line the error points at.
- **The wrong shape**: `netscenario schema` prints the JSON Schema; editors can use it to complete and check the file ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md)).

**A workload with `udp-checksum: false` is refused: a node cannot open a raw socket.** The sender of such a stream is a raw socket and needs `CAP_NET_RAW` in the node. In a user namespace and as root it is there; a process that has only the two ambient capabilities of [PERMISSIONS.md](PERMISSIONS.md) does not have it. `netscenario doctor` shows `raw-socket` and the fix; the file can also leave `udp-checksum` out (the checksum is then on, a damaged packet is dropped by the receiving kernel and counted per node in `directions`).

**The scenario is valid but the backend refuses it.** The message names the feature, the backend, the path in the file and a way out, for example:

```text
error: the kernel backend cannot reproduce 'Same seed, same loss pattern': the `tc` of this machine is iproute2 6.1.0, and `seed` needs 6.6.0 or newer
 --> seeded.yaml:3:7
  |
2 | name: seeded
3 | seed: 1234
  |       ^^^^
  = path: seed
  = help: install iproute2 6.6.0 or newer, leave out `seed` (the run is then not reproducible and says so), or use the Engine backend (planned for release 0.4)
```

This is the rule of the project: a backend that cannot do something exactly refuses it, and never approximates it silently ([COVERAGE.md](COVERAGE.md)).

## A run that fails (exit code 4)

The network is torn down all the same; the message says what the supervisor or an agent reported.

- **Nothing is left behind**, also after a crash or a kill: the run lives in a namespace that the kernel removes when its first process ends. `netscenario clean --dry-run` shows what, if anything, would be ended; `netscenario clean` ends it ([DOCTOR.md](DOCTOR.md)).
- **The run took too long.** A watchdog ends a run after the duration of the scenario plus 60 seconds.
- **Run with `--packets`** to write the record of every packet next to the result, and look at the first and last packets of the workload ([RESULTS.md](RESULTS.md)).
- **A program of your own fails the run before any packet.** It ended before it was ready, was not ready within `within`, or could not be started; the message has its id, what it was waited for and the end of its standard error. A name without a slash is looked for in `PATH` and a path with a slash from the directory of the scenario file; a socket bound only to the loopback does not count as ready, because the other node cannot reach it ([PROGRAMS.md](PROGRAMS.md)).
- **A program of your own with `on-exit: fail` ended with an error before the end of the run.** The run ends with the exit code or signal and the end of its standard error, and writes no result, so there is no report to make; with `on-exit: continue` (the default) the crash is a finding and the result has it (`ended: "on-its-own"`).
- **A capture fails the run: *cannot open a packet socket*.** The capture needs `CAP_NET_RAW` in the node; in a user namespace it is there, otherwise add `+net_raw` to the capabilities ([PERMISSIONS.md](PERMISSIONS.md)).
- **A set of repetitions ended without a `runset.json`.** A set ends with the first run that fails, with that run's error, and keeps the directories of the runs that finished; an interval from fewer runs than were asked for would be a made-up number. `repetitions` is at least 5 and at most 1,000 ([STATISTICS.md](STATISTICS.md)).

## The result looks wrong (but is not)

- **Packets arrive in a different order (`reordered` is large).** A jitter larger than the gap between two packets lets a packet overtake the one before it. This is what the link does, and the result counts it. Lower the jitter or raise the packet rate to see it change ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md), the order of the stages).
- **The delay is higher than the configured one.** The delay is the configured delay plus the jitter plus the time in the queue of the `rate`: a stream that is close to the rate waits in the queue. Check the rate against the workload (`rate` of a workload counts payload; the link counts Ethernet frames, payload plus 42 bytes: [FIDELITY.md](FIDELITY.md)).
- **The loss differs from the configured one.** Loss is random: with 800 packets and 2 % configured, 16 are expected and about 8 to 24 are usual. The seed fixes which packets are lost, not how many a different seed loses.
- **The same seed gives different delays.** The seed fixes the loss pattern on the same kernel and `tc`; the jitter is not promised to repeat ([COVERAGE.md](COVERAGE.md)). Assert on a percentile with room, not on an exact number.
- **`corrupted_seq_unavailable` says the damaged packets could not be named.** The receiver reads the frames of the interface with a packet socket, which needs `CAP_NET_RAW` in the node; without it the run goes on and counts the damaged packets (`directions`) but does not name them ([PERMISSIONS.md](PERMISSIONS.md), `netscenario doctor` shows `raw-socket`). The names are also withdrawn when the node dropped datagrams because a receive buffer was full (`receive_buffer_drops` in `directions`), since a missing packet is then not necessarily a damaged one.
- **`corrupted_named` is lower than `dropped_corrupted`.** It should be: a frame whose bit hit the type, the address, the port or the header of the agent cannot be named (82 % of the dropped frames are named at 64 byte payloads, 92 % at 200, 99 % at 1,200). A value much lower than that means a loss in the same direction or a busy machine; one higher cannot happen on a link that only corrupts.
- **`corrupted` is 0 although the link corrupts packets.** With the checksum of UDP on (the default), the receiving kernel drops a damaged packet before the agent sees it, so nothing is counted as `corrupted`; `directions` counts them per node (`dropped_corrupted`). Write `udp-checksum: false` on the workload to let them arrive and be counted. Even then a part of the damaged packets never arrives: a bit in the headers of Ethernet, IP and UDP is caught by the kernel (the fidelity cases in [FIDELITY.md](FIDELITY.md) give the shares).
- **The program lost more packets than the loss says (there is `corrupt` in the link).** A packet with a flipped bit arrives at the node and the node drops it, so the program sees it as lost. `directions` in the result (and a line `link ...` in the summary) says how many were lost in the queue and how many arrived damaged and were dropped ([RESULTS.md](RESULTS.md)).
- **`loss_reproducible` is false.** `not_reproducible_because` in the result says why, and the command line prints it. With `corrupt` in the scenario it is always false, and the lost packets of two runs with one seed differ by a few (the place of the flipped bit is not seeded; `lost_in_queue` in `directions` repeats exactly). Otherwise either no seed could be applied (the `tc` is older than 6.6.0), or a queue counted packets that the workloads did not send (IPv6 neighbour discovery, ARP, a program of yours in the node) and that shifted the pattern. The command line names the queue and the two numbers, and `queue_checks` in the result has them ([RESULTS.md](RESULTS.md)).
- **Numbers on a busy machine.** The agents measure with the clock of the machine; a machine whose cores are busy delays the wake-up of the sender and the receiver by a little. The tests of the project run one at a time for this reason ([FIDELITY.md](FIDELITY.md)). In a CI, do not run several scenarios side by side on few cores.

## Replay and reports

- **`replay` says the directory is not a run, or the directory of a set.** Give the directory that holds `result.json` (for a run of a set, `run-001` and so on). A directory without `scenario.json` was made by a version that did not record the scenario, and cannot be replayed: run the scenario file again ([CLI.md](CLI.md#replay)).
- **`replay` says that `scenario.json` is not the scenario of the result.** One of the two files was changed after the run; the fingerprints in the message are the one of the result and the one of the file. Nothing is replayed.
- **The replay is *NOT REPRODUCED* (exit code 1, `verdict` `differs`).** Both runs said that their loss repeats from the seed and the replay lost other packets. The summary names the packets, and says if the machine is not the original's: the lost packets of a seed depend on the kernel's `netem` and on `tc`, so look at the fields it lists first. A *not promised* replay (exit code 0) is a difference that the notes explain: an outage, corruption or a program that sends.
- **`report --format html` names a file that is missing.** The HTML of a run is drawn from `series-<id>.jsonl` and `flow-<n>.records`, which have to be in the directory; a result of a version without a series gets a page without the charts over time. Markdown and JUnit need only `result.json` ([REPORTS.md](REPORTS.md)).
- **A JUnit case fails.** Two reasons only: the run was interrupted, or the kernel refused packets of the workload and no `down` step explains it. A warning about reproducibility is text in the case, not a failure.

## pytest

- **Tests are skipped: *this machine cannot run scenarios*.** On a laptop the plugin skips and prints the reason and the fix from `doctor`. On a CI (the variable `CI` is set) it fails the test instead, because a test that never ran proves nothing. `--netscenario-environment=skip|fail|auto` chooses ([PYTEST.md](PYTEST.md)).
- **`FileNotFoundError` for a scenario.** The file is looked for next to the test file, in the directories of the ini option `netscenario_scenarios` (relative to the ini file) and in the root directory of the project. The error lists the places.
- **`netscenario_result` or `netscenario_runs` is an error of the test.** The first is for a marker without `repetitions` and gives a `Result`; the second is for a marker with `repetitions` and gives a `RunSet`. The message says which one to use ([PYTEST.md](PYTEST.md)).
- **No section `netscenario` in the report.** It is shown for failing tests; run with `-rA` for passing tests too. The plugin is found through its entry point: when it is installed, the header of a pytest run shows a line `netscenario: <version>`.

## Installing

- **`pip` finds no matching distribution.** The wheels are for Linux on x86_64 and aarch64 with glibc 2.28 or newer (`manylinux_2_28`), for Python 3.11 or newer. There is no wheel for musl (Alpine), macOS or Windows yet. The Kernel backend is Linux-only anyway ([COVERAGE.md](COVERAGE.md)).
- **`pip install pytest-netscenario` installs an older `netscenario`?** It cannot: the plugin pins exactly the same version.
- **`ProgramsNotFoundError` in a checkout** (after `uv sync`): run `cargo build` first; the Python package finds the programs in `target/` ([PYTHON.md](PYTHON.md)).

## Still stuck

Open an issue and paste the output of `netscenario doctor --json` and, for a run, `result.json` without the per-packet files: it has the versions, the kernel, the `tc` and the seed that made it ([RESULTS.md](RESULTS.md)). Do not paste anything that you do not want to be public.
