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
- **A key of a later release** (a `timeline`, a loss model other than `random`, a queue). The error says in which release it is planned ([FEATURES.md](../FEATURES.md)).
- **A misspelled key, value, unit or name**: the error lists the keys that are allowed there (`unknown field `dealy`, expected one of delay, jitter, loss, rate`) and, if exactly one name is close, asks `did you mean 'delay'?`. The same holds for the values of an enumeration, for a unit (`Mbit`, `min`, `mbps`) and for a node that does not exist. It is a guess: read the line the error points at.
- **The wrong shape**: `netscenario schema` prints the JSON Schema; editors can use it to complete and check the file ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md)).

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

## The result looks wrong (but is not)

- **Packets arrive in a different order (`reordered` is large).** A jitter larger than the gap between two packets lets a packet overtake the one before it. This is what the link does, and the result counts it. Lower the jitter or raise the packet rate to see it change ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md), the order of the stages).
- **The delay is higher than the configured one.** The delay is the configured delay plus the jitter plus the time in the queue of the `rate`: a stream that is close to the rate waits in the queue. Check the rate against the workload (`rate` of a workload counts payload; the link counts Ethernet frames, payload plus 42 bytes: [FIDELITY.md](FIDELITY.md)).
- **The loss differs from the configured one.** Loss is random: with 800 packets and 2 % configured, 16 are expected and about 8 to 24 are usual. The seed fixes which packets are lost, not how many a different seed loses.
- **The same seed gives different delays.** The seed fixes the loss pattern on the same kernel and `tc`; the jitter is not promised to repeat ([COVERAGE.md](COVERAGE.md)). Assert on a percentile with room, not on an exact number.
- **The program lost more packets than the loss says (there is `corrupt` in the link).** A packet with a flipped bit arrives at the node and the node drops it, so the program sees it as lost. `directions` in the result (and a line `link ...` in the summary) says how many were lost in the queue and how many arrived damaged and were dropped ([RESULTS.md](RESULTS.md)).
- **`loss_reproducible` is false.** `not_reproducible_because` in the result says why, and the command line prints it. With `corrupt` in the scenario it is always false, and the lost packets of two runs with one seed differ by a few (the place of the flipped bit is not seeded; `lost_in_queue` in `directions` repeats exactly). Otherwise either no seed could be applied (the `tc` is older than 6.6.0), or a queue counted packets that the workloads did not send (IPv6 neighbour discovery, ARP, a program of yours in the node) and that shifted the pattern. The command line names the queue and the two numbers, and `queue_checks` in the result has them ([RESULTS.md](RESULTS.md)).
- **Numbers on a busy machine.** The agents measure with the clock of the machine; a machine whose cores are busy delays the wake-up of the sender and the receiver by a little. The tests of the project run one at a time for this reason ([FIDELITY.md](FIDELITY.md)). In a CI, do not run several scenarios side by side on few cores.

## pytest

- **Tests are skipped: *this machine cannot run scenarios*.** On a laptop the plugin skips and prints the reason and the fix from `doctor`. On a CI (the variable `CI` is set) it fails the test instead, because a test that never ran proves nothing. `--netscenario-environment=skip|fail|auto` chooses ([PYTEST.md](PYTEST.md)).
- **`FileNotFoundError` for a scenario.** The file is looked for next to the test file, in the directories of the ini option `netscenario_scenarios` (relative to the ini file) and in the root directory of the project. The error lists the places.
- **No section `netscenario` in the report.** It is shown for failing tests; run with `-rA` for passing tests too. The plugin is found through its entry point: when it is installed, the header of a pytest run shows a line `netscenario: <version>`.

## Installing

- **`pip` finds no matching distribution.** The wheels are for Linux on x86_64 and aarch64 with glibc 2.28 or newer (`manylinux_2_28`), for Python 3.11 or newer. There is no wheel for musl (Alpine), macOS or Windows yet. The Kernel backend is Linux-only anyway ([COVERAGE.md](COVERAGE.md)).
- **`pip install pytest-netscenario` installs an older `netscenario`?** It cannot: the plugin pins exactly the same version.
- **`ProgramsNotFoundError` in a checkout** (after `uv sync`): run `cargo build` first; the Python package finds the programs in `target/` ([PYTHON.md](PYTHON.md)).

## Still stuck

Open an issue and paste the output of `netscenario doctor --json` and, for a run, `result.json` without the per-packet files: it has the versions, the kernel, the `tc` and the seed that made it ([RESULTS.md](RESULTS.md)). Do not paste anything that you do not want to be public.
