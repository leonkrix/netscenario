# Architecture

How netscenario is put together, and why. This is the design the code will follow; where it depends on an experiment, it says so. Decisions that were made are in [DECISIONS.md](DECISIONS.md).

## What it is not

- **Not a simulator.** There is no virtual network with virtual time that runs faster or slower than real time (ns-3 does that well). netscenario runs real programs on a real network stack with controlled impairments. (The Engine backend may offer an optional virtual clock for determinism, but the programs under test still run in real time.)
- **Not a load tester.** It does not measure how many requests a server handles. It measures how a program behaves on a given network.
- **Not chaos engineering for clusters.** It does not break Kubernetes. It describes a network and measures.
- **Not a packet generator for security work.**

## Vocabulary

| Term | Meaning |
| --- | --- |
| Scenario | The whole description: topology, impairments, timeline, workloads, and optionally expectations |
| Node | A place where programs run (a network namespace) |
| Link | A connection between two nodes with its own impairments in each direction |
| Impairment | Delay, jitter, loss, reordering, duplication, corruption, rate limit, queue behaviour |
| Link model | A function from time (and state, such as positions) to the impairments of a link |
| Timeline | Events that change the scenario at given times |
| Workload | Something that runs on a node: a built-in traffic generator or your program |
| Probe | Something that measures: per-packet records, packet capture, metrics from your program |
| Backend | The thing that makes the network real: Kernel, Engine or Proxy |
| Run | One execution of a scenario with one seed |
| Experiment | Several runs of one scenario (repetitions, a sweep) |
| Fidelity | What a backend reproduces faithfully |

## The parts

```
                      +---------------------------+
   scenario file ---->|  netscenario-model        |  types, validation, JSON Schema
   code (Py / Rust)-->|  (parse, check, derive)   |
                      +-------------+-------------+
                                    |
                      +-------------v-------------+      +---------------------------+
                      |  netscenario-engine       |<---->|  link models (plug-ins)   |
                      |  run lifecycle, timeline, |      |  static, steps, trace,    |
                      |  supervision, results     |      |  Markov, geometry         |
                      +------+------+-------+-----+      +---------------------------+
                             |      |       |
              +--------------+      |       +----------------+
              |                     |                        |
     +--------v--------+  +---------v---------+   +----------v---------+
     | backend: kernel |  | backend: packet   |   | backend: proxy     |
     | netns+veth+netem|  | userspace engine  |   | portable, UDP exact|
     +--------+--------+  +---------+---------+   +----------+---------+
              |                     |                        |
              +----------+----------+------------------------+
                         |
              +----------v-----------+
              | netscenario-agent    |  static binary inside every node:
              | traffic and probes   |  generators, per-packet records
              +----------+-----------+
                         |
          +--------------v---------------+       +-------------------------+
          | netscenario-cli              |       | netscenario-py (PyO3)   |
          | run, validate, doctor, clean |       | + pytest-netscenario    |
          +------------------------------+       +-------------------------+
```

Planned layout of the repository (a Cargo workspace plus Python packages; created in Phase 1):

| Path | Purpose |
| --- | --- |
| `crates/netscenario-model` | Scenario types, units, validation, JSON Schema. No system access. |
| `crates/netscenario-core` | Run lifecycle, timeline, link models, statistics, results |
| `crates/netscenario-backend-kernel` | Namespaces, links, `netem`, cleanup |
| `crates/netscenario-backend-packet` | Packet-level emulator (from 0.4) |
| `crates/netscenario-backend-proxy` | Portable proxy (from 0.5) |
| `crates/netscenario-agent` | Traffic generators and probes that run inside the nodes |
| `crates/netscenario-cli` | The command `netscenario` |
| `crates/netscenario-py` | Python bindings (PyO3, built with maturin) |
| `python/netscenario` | Python API (builder, results, assertions) |
| `python/pytest-netscenario` | The pytest plugin |
| `scenarios/` | Example and test scenarios, which are also used by the tests |
| `docs/` | These documents, and later the user guide |
| `spikes/` | Experiments that check what the system allows |

## The life of a run

1. **Parse and validate.** The file (or the object built in code) becomes a model. Errors name the place in the file.
2. **Check the capabilities.** The chosen backend reports what it can do faithfully ([COVERAGE.md](COVERAGE.md)). A scenario that asks for more is refused here, before anything is created, with a message that names the feature and a backend that can do it.
3. **Check the environment.** `doctor` checks permissions, kernel features and tools. A missing permission is an error that says how to fix it.
4. **Provision.** Create the namespaces, the links, the addresses and routes, apply the impairments of time zero. Everything runs in a PID namespace whose first process is the supervisor, and the nodes are network namespaces held by processes and addressed by their process id, so that a crash cannot leave a mess: when the supervisor dies, the kernel ends every process of the run and removes the namespaces (D-022). There are no named objects to clean up; `netscenario clean` is a safety net for runs that lost their supervisor.
5. **Start.** Agents first (they open ports and wait), then your programs, with readiness checks. Addresses and ports reach your program through environment variables.
6. **Run the timeline.** A scheduler applies the events and link-model updates at their times and records when each was applied, so that the result shows the real moment, not only the planned one.
7. **Collect.** Agents stream per-packet records; the backend adds counters and packet captures; your program's metrics are read. Everything goes into one result with the provenance block.
8. **Stop and tear down.** Orderly stop of programs with a timeout, then removal of everything. A watchdog does the same when a run hangs.
9. **Assert.** Done by the test code or by the `expect` section, on the finished result. Repetitions and statistics happen above single runs.

## Backends and fidelity

A backend implements a small interface: report capabilities, create a node, create a link, apply impairments to a link direction, start a program in a node, capture, tear down. The engine uses nothing else, so a new backend means implementing that interface and filling its column in the coverage matrix.

- **Kernel** (reference). One network namespace per node, virtual Ethernet pairs for links, `tc` with `netem` (and a token bucket or a queue discipline) on each direction. Time-varying behaviour is done by changing the parameters at the steps of a trace. It is real, and its limits are the kernel's: a change of a running `netem` takes about 12 microseconds in a batch (spike S5, and through netlink in S7), so the resolution is set by the scheduler and not by `tc`, and the seed of `netem` guides only the loss and corruption events and needs a quiet namespace (see [COVERAGE.md](COVERAGE.md)).
- **Engine** (from 0.4). A Rust process that forwards packets between virtual devices inside a user namespace, with its own queues and impairment code. It needs no root, can be seeded exactly, can log every packet, and can implement behaviour that `netem` lacks (traces per packet, any loss model). Its correctness is not assumed: it is compared with the Kernel backend by differential tests. A prototype (spike S8) forwarded 100,000 packets per second on one thread with an exact delay and a deterministic seeded loss; the timer, the completion of checksums for TCP and the queue for jitter are still to be designed (see D-014 in [DECISIONS.md](DECISIONS.md)).
- **Proxy** (from 0.5). Your program talks to a proxy process. Exact for UDP, stream-level for TCP. It exists so that people on Windows and macOS, or in containers without rights, can run the scenarios that are meaningful there. Everything it cannot do is refused.

**Rule:** a backend never approximates silently. If it can do something only in part, the scenario must say `allow-limited` for that feature, and the result records that it did.

## Link models and mobility

A link model is a function that answers "what are the impairments of this link direction at time t?". Built-in models: static, scheduled steps, trace, Markov states, periodic pattern, and geometry-based (distance between two nodes through a propagation model to loss, delay and rate). Mobility models give nodes positions over time (waypoints, random models, imported traces, orbits), and geometry-based link models read those positions. The model interface is public (a Rust trait, and traces generated from Python), so research scenarios (airborne or satellite networks) are a configuration and not a fork.

The scheduler asks the models for their steps ahead of time, so a run's timeline is known before it starts, can be shown in the report, and does not depend on the speed of the machine.

## Time, measurement and determinism

- **One clock.** All nodes run on one machine, so `CLOCK_MONOTONIC` is shared and one-way delay is measured exactly, not estimated.
- **Agents, not the program, measure.** Built-in workloads write per-packet records (sequence number, send time, receive time) into the result. Your own program reports through its output, and the framework can add a capture on the links.
- **What is deterministic.** With a seed, loss and corruption patterns, jitter samples, trace choices and generated traffic are reproducible where the backend supports it (Engine and Proxy always; Kernel for loss and corruption when `tc` is iproute2 6.6.0 or newer and the namespace is quiet, see [COVERAGE.md](COVERAGE.md)). Scheduling noise of the machine is not removed, and results say so: they report timer precision and the real moments of events.
- **Statistics above runs.** A single measurement is a sample. The framework runs repetitions with seeds derived from the scenario seed, and assertions can be stated with a confidence interval.

## Python and pytest

The Rust core is the only implementation of semantics. The Python package wraps it (PyO3, built with maturin) and adds the builder, the result objects, the statistics helpers and the pytest plugin (`pytest-netscenario`, a separate small distribution that depends on `netscenario`). Fixtures give a runner per test, scenario files are found by path or by marker, and results are attached to the test report. The plugin works with `pytest-xdist`: every run owns its namespaces, and names include the process, so parallel runs do not collide.

## Privileges

The preferred mode is an unprivileged user namespace (`unshare` with a user, a network and a mount namespace), where the process is root only inside it. That works by default on Debian 13 and Ubuntu 22.04. Ubuntu 24.04 and newer refuse it until `kernel.apparmor_restrict_unprivileged_userns` is 0 or an AppArmor profile allows the binary, and a container needs `SYS_ADMIN` and a seccomp profile that allows `unshare` (spikes S1 and S6). The fallbacks are root, `sudo`, or the capabilities `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` granted to the binary. `doctor` says which mode is available and why the others are not.

## Errors and messages

The same philosophy as the sibling projects (the Playwright guards): an error says what happened, where, and how to fix or allow it. A scenario error names the path in the file. A capability error names the feature, the backend, and a backend that can do it. A permission error names the missing right and the options.

## Extension points

Link models, workloads, probes, reporters and backends are interfaces. Built-ins use the same interfaces as plug-ins from outside, so the interfaces stay honest. Plug-ins are Rust crates in the beginning; Python callbacks are limited to generating traces and to test code, because they must not sit in the packet path.

## Versioning and compatibility

Four things are versioned separately and say so in their files: the scenario format (`version:` in the file), the result format, the Rust and Python APIs (semantic versioning), and the command line. Before 1.0 a minor release may break things and says so in the changelog. From 1.0 on, each stays compatible within a major version. Migration between scenario versions is a command (SCN-08).
