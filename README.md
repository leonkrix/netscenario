# netscenario

[![CI](https://github.com/leonkrix/netscenario/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/leonkrix/netscenario/actions/workflows/ci.yml)
[![CodeQL](https://github.com/leonkrix/netscenario/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/leonkrix/netscenario/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/leonkrix/netscenario/badge)](https://scorecard.dev/viewer/?uri=github.com/leonkrix/netscenario)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust)](docs/DECISIONS.md)
[![Python 3.11+](https://img.shields.io/badge/python-3.11%2B-blue?logo=python&logoColor=white)](docs/DECISIONS.md)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux&logoColor=white)](docs/COVERAGE.md)
[![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-red)](FEATURES.md)
[![Release](https://img.shields.io/github/v/release/leonkrix/netscenario)](https://github.com/leonkrix/netscenario/releases)
[![PyPI](https://img.shields.io/pypi/v/netscenario?label=PyPI)](https://pypi.org/project/netscenario/)
[![crates.io](https://img.shields.io/crates/v/netscenario-cli?label=crates.io)](https://crates.io/crates/netscenario-cli)

Describe a network, send traffic through it, measure what happens, and assert on it. In a test, in your CI, or on your laptop.

> **Status: pre-alpha, release 0.2.** The Kernel backend, the command line, the Python package and the pytest plugin work on Linux: you describe a point-to-point link in a file, with burst loss, reordering, duplication, corruption, outages and changes over time, run it in network namespaces with a stream or **your own program** in the nodes, and get loss, delay percentiles, throughput and a series over time back, with the seed and the versions that produced them. Repeat a run with derived seeds to get **confidence intervals**, replay a run from its directory, and make reports as Markdown, JUnit and HTML. What is not there yet is listed under "What 0.2 does not do". The [feature ledger](FEATURES.md) says what exists, what is next and what is only an idea.

## Why

Code that talks over a network behaves differently on a good link and on a bad one: a video call over a mobile network that loses bursts of packets, a game on a link with 150 ms of delay, a protocol on a satellite hop, a service when a link goes down for two seconds. Testing that today usually means a shell script with `tc netem`, run by hand, with results read by eye, not repeatable on the next machine and not part of the tests.

netscenario is meant to make this a normal, repeatable test:

- **A scenario is data.** Nodes, links, delay, jitter, loss (also in bursts), reordering, duplication, corruption, a rate, outages and changes over time, written in a short file or built in code. (Queues and more than two nodes come in the next release.)
- **Results are numbers with provenance.** Loss, delay percentiles, throughput, the series over time and the time to recover after an event, together with the seed, the kernel and the tool versions that produced them. Several runs give intervals, and a run can be replayed.
- **It fits the tools you have.** A command line, a Python package, a `pytest` plugin, reports for a CI (Markdown for the summary of a job, JUnit, one HTML file), and a CI that does not need a special machine.
- **Later in the plan:** more than two nodes, TCP, queue disciplines, baselines and regression detection, containers as nodes, and backends that need no root.

## Install

Linux, with the tools `ip`, `tc`, `unshare`, `nsenter` and `prlimit` (packages `iproute2` and `util-linux`) and the right to create network namespaces. Debian and Ubuntu 22.04 give a user that right; Ubuntu 24.04 needs one setting; a container needs more ([docs/PERMISSIONS.md](docs/PERMISSIONS.md)).

```bash
pip install netscenario pytest-netscenario     # wheels for Linux, x86_64 and aarch64, Python 3.11 or newer
```

```bash
cargo install netscenario-cli netscenario-agent    # the programs `netscenario` and `netscenario-agent`, Rust 1.89 or newer
```

The wheel of `netscenario` carries the two programs, so the Python package needs nothing else. The command line needs both programs side by side, which is why both crates are installed.

**From source** (a checkout of this repository, for development):

```bash
cargo build --release          # target/release/netscenario and netscenario-agent
uv sync                        # the Python packages, if you want them
```

## First run

Ask the machine what it can do, then run an example. Nothing needs root on a machine that allows user namespaces.

```bash
netscenario doctor
netscenario run scenarios/lte-like.yaml --seed 42
```

```text
running lte-like for 30s ...
netscenario run: lte-like
  backend kernel (user namespace, iproute2 6.15.0)
  seed 42

workload media  client -> server  2 mbit/s, 1200 byte packets
  sent 6250  received 6203  lost 47 (0.75 %)  reordered 1760
  delay  min 21.0 ms  median 41.1 ms  p95 48.9 ms  p99 52.4 ms  max 57.6 ms
  throughput  1.98 mbit/s on average over 30 s (31 bins of one second in result.json)

result: netscenario-results/lte-like-20261030T202546Z/result.json
```

The scenario is [scenarios/lte-like.yaml](scenarios/lte-like.yaml): two nodes and a link with 40 ms of delay, a jitter of 5 ms, 1 % loss and different rates in the two directions, and one stream of 2 mbit/s. The same seed gives the same lost packets on the same kernel and `tc`; the delays vary a little from run to run, as they do on a real link.

**From Python** ([docs/PYTHON.md](docs/PYTHON.md)):

```python
import netscenario as ns

result = ns.run(ns.Scenario.load("scenarios/lte-like.yaml"))
assert result["media"].delay.p99 < 0.150   # seconds
```

**As a pytest test** ([docs/PYTEST.md](docs/PYTEST.md)):

```python
def test_media_survives_the_lte_link(netscenario):
    media = netscenario.run("scenarios/lte-like.yaml")["media"]
    assert media.loss_ratio < 0.02
    assert media.delay.p99 < 0.150  # seconds
```

On a machine that cannot create namespaces the test is skipped with the reason (and fails on a CI, where a test that never ran proves nothing). Two complete examples with their output are in [examples/](examples/).

A scenario file that is wrong is refused with the line, the column and a hint:

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

The commands are `run`, `replay`, `report`, `validate`, `schema`, `doctor` and `clean` ([docs/CLI.md](docs/CLI.md)); the result is described field by field in [docs/RESULTS.md](docs/RESULTS.md). If something does not work, [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) starts from the message.

## Time, noise and your own program

**A link that changes while a stream runs** ([scenarios/bandwidth-drop-and-outage.yaml](scenarios/bandwidth-drop-and-outage.yaml)): the rate drops, the link goes quiet for two seconds, and the limit is taken away again.

```yaml
timeline:
  - { at: 10s, link: access, set: { both: { rate: 2mbit } } }
  - { at: 20s, link: access, outage: 2s }
  - { at: 25s, link: access, set: { both: { rate: null } } }
```

The result has the series by second in two views (what was sent in each second and what arrived in it), how late each step was, and how long each workload took to be back at its level after each event ([docs/RESULTS.md](docs/RESULTS.md); [examples/timeline/](examples/timeline/)).

**One run is one sample.** Run it several times with seeds derived from one seed and assert on an interval ([docs/STATISTICS.md](docs/STATISTICS.md)):

```bash
netscenario run scenarios/lte-like.yaml --repetitions 10 --out results/lte    # ten runs of 30 s, an interval for each metric
netscenario report results/lte --format md >> "$GITHUB_STEP_SUMMARY"           # the summary of a CI job
netscenario replay results/lte/run-003                                         # the same seed, the same lost packets?
```

```python
runs = ns.run("scenarios/lte-like.yaml", repetitions=10)
assert runs.metric("media.loss_ratio").ci95_high < 0.05     # the upper end of the 95 % interval of the mean
assert runs.metric("media.delay.p99").ci95_high < 0.150     # seconds
```

**Your own program in the network**: a node can run a server, a client or the program that you build, started in order, waited for, supervised, with the addresses of the other nodes in its environment, and with the metrics it prints as lines of JSON in the result ([docs/PROGRAMS.md](docs/PROGRAMS.md); [examples/echo-server/](examples/echo-server/)). **Reports** ([docs/REPORTS.md](docs/REPORTS.md)) are made from a result directory: Markdown, JUnit with one test case for each workload, and one HTML file with charts and nothing to fetch ([sample](docs/sample-report/report.html)).

## What 0.2 does and does not do

| Does | Does not do yet |
| --- | --- |
| Two nodes and one link, IPv4; a link that goes down and up | More than two nodes (a star, a chain, a dumbbell: 0.3), nodes that join and leave (0.6), IPv6 (0.3) |
| Delay, jitter (uniform, normal), loss at random and in bursts (Gilbert-Elliott, four states), reordering, duplication, corruption, a rate, outages, different values per direction | Queue limits and disciplines, other jitter distributions, traces (0.3) |
| Changes over time: steps that set any of these, links down and up, outages, done so that the same seed gives the same lost packets | Mobility and geometry (0.6) |
| UDP streams, constant, Poisson, on/off and bursty, also without the checksum, measured per packet: loss, one-way delay, throughput, reordering, jitter, loss runs, the series over time and the time to recover | TCP and request/response workloads (0.3), a container as a node (0.5) |
| Your own program as a node, with readiness, supervision, logs and metrics; packet capture of a link as pcap | Baselines and regression detection (0.7) |
| Seeds, and the same lost packets for the same seed on the same kernel and `tc`; repetitions with confidence intervals; replay of a run | Backends other than the Linux kernel (0.4 and 0.5) |
| Results as JSON with the versions and the seed; reports as Markdown, JUnit and HTML | A user guide (0.8) |
| A command line, a Python package, a pytest plugin | |

A scenario that asks for something that is not in the left column is refused with an error that names the release in which it is planned; it is never approximated silently. **Release 0.2 changes the format of the scenario file and of the result** (version 0 of both, which may change in a minor release): the [changelog](CHANGELOG.md) names what breaks.

## Principles

1. **Correct before convenient.** Every backend says what it reproduces faithfully and what it does not. A scenario that asks for something a backend cannot do correctly fails with an error. It is never quietly approximated. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
2. **The kernel is the reference.** The Linux network stack (network namespaces and `netem`) is the ground truth. Other backends are checked against it.
3. **Reproducible by default.** Seeds everywhere they are possible, and every result records how it was made.
4. **Nothing happens to the host.** All emulation lives in namespaces that netscenario creates and removes. It never changes the network of your machine.
5. **Complete in the end, released in steps.** The [feature ledger](FEATURES.md) and the [coverage matrix](docs/COVERAGE.md) list everything that is planned, and every release has acceptance criteria.

## Architecture in one paragraph

A Rust core (scenario model, scheduler, backends, traffic generators, probes, statistics) with a command line, and a Python package with a `pytest` plugin on top. Backends: the Linux kernel (namespaces plus `netem`), later a packet-level emulator in userspace that runs without root, and a portable proxy mode for UDP. The core is also usable from Rust tests. Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Where to read next

- [FEATURES.md](FEATURES.md): every planned feature, its priority, its release and its status.
- [docs/COVERAGE.md](docs/COVERAGE.md): what is faithful on which backend.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): the parts and how they fit.
- [docs/PYTEST.md](docs/PYTEST.md): the pytest plugin: the fixture, the marker, the guard for machines that cannot run scenarios, the report.
- [docs/PYTHON.md](docs/PYTHON.md): the Python package: scenarios, builder, results in SI units, repetitions, reports, replay, errors, observing and stopping a run.
- [docs/CLI.md](docs/CLI.md): the commands (`run`, `replay`, `report`, ...), their options, the exit codes and the JSON they print.
- [docs/FIDELITY.md](docs/FIDELITY.md): how close the Kernel backend is to what a scenario says, measured, and what it does that you may not expect.
- [docs/RESULTS.md](docs/RESULTS.md): the result of a run, field by field.
- [docs/AGENT.md](docs/AGENT.md): the agent in the nodes: the packet, the record file, the command line.
- [docs/PERMISSIONS.md](docs/PERMISSIONS.md): the ways to get the right to create namespaces, on a laptop, in a container and in a CI.
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md): from a message or a symptom to the cause and the fix.
- [examples/](examples/): a UDP protocol on the command line, your own program over a bad link, a pytest test with a confidence interval, and a link that changes during a run, with their output.
- [docs/STATISTICS.md](docs/STATISTICS.md): repetitions and confidence intervals: the method, its coverage, and what "stable" means.
- [docs/PROGRAMS.md](docs/PROGRAMS.md): your own program as a node: order, readiness, supervision, logs, metrics.
- [docs/REPORTS.md](docs/REPORTS.md): Markdown, JUnit and HTML from a result directory, with a CI example and a [sample](docs/sample-report/).
- [scenarios/](scenarios/): example scenarios, checked by the tests.
- [docs/DOCTOR.md](docs/DOCTOR.md): `netscenario doctor` and `netscenario clean`: what they check, what they print, their exit codes.
- [docs/SCENARIO-FORMAT.md](docs/SCENARIO-FORMAT.md): the scenario file, version 0: every key, every unit, every error.
- [docs/PRIOR-ART.md](docs/PRIOR-ART.md): what exists, and where netscenario differs.
- [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md): how each criterion of a release is proven, and what the proof does not cover.
- [docs/VERSIONING.md](docs/VERSIONING.md): what a version number promises, and what a release may break.
- [docs/RELEASING.md](docs/RELEASING.md): how a release is made, and the one-time setup of the registries.
- [docs/DECISIONS.md](docs/DECISIONS.md): the decisions made so far, with reasons (one file each in [docs/decisions/](docs/decisions/)).
- [spikes/](spikes/): experiments that check what the Linux kernel allows, to run before building.

## Requirements

Linux for the real emulation, on x86_64 and aarch64; the tests ran on kernels 6.8, 6.12 and 7.0 and older kernels are untested. Python 3.11 or newer; Rust 1.89 or newer to build from source. Other systems are targeted by the portable backends later.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and the [code of conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you say otherwise, any contribution you submit for inclusion is licensed as above, without additional terms or conditions.
