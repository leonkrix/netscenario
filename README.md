# netscenario

[![CI](https://github.com/leonkrix/netscenario/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/leonkrix/netscenario/actions/workflows/ci.yml)
[![CodeQL](https://github.com/leonkrix/netscenario/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/leonkrix/netscenario/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/leonkrix/netscenario/badge)](https://scorecard.dev/viewer/?uri=github.com/leonkrix/netscenario)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust)](docs/DECISIONS.md)
[![Python 3.11+](https://img.shields.io/badge/python-3.11%2B-blue?logo=python&logoColor=white)](docs/DECISIONS.md)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux&logoColor=white)](docs/COVERAGE.md)
[![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-red)](FEATURES.md)

Describe a network, run your program in it, measure what happens, and assert on it. In a test, in your CI, or on your laptop.

> **Status: pre-alpha.** The Kernel backend and the command line work on Linux: you can describe a point-to-point link in a file, run it in network namespaces, and get loss, delay percentiles and throughput back, with the seed and the versions that produced them. There is no release, no Python package and no pytest plugin yet, and most of what is below under "How it is going to work" does not exist. The [feature ledger](FEATURES.md) says what exists, what is next and what is only an idea.

## Why

Code that talks over a network behaves differently on a good link and on a bad one: a video call over a mobile network that loses bursts of packets, a game on a link with 150 ms of delay, a protocol on a satellite hop, a service when a link goes down for two seconds. Testing that today usually means a shell script with `tc netem`, run by hand, with results read by eye, not repeatable on the next machine and not part of the tests.

netscenario is meant to make this a normal, repeatable test:

- **A scenario is data.** Nodes, links, delay, jitter, loss (also bursty), rate, queues, and things that change over time, written in a short file or built in code.
- **Your program is the test subject.** Any command or container, in any language. netscenario starts it inside the scenario, generates traffic, measures, and stops it.
- **Results are numbers with provenance.** Loss, delay percentiles, throughput, time to recover, together with the seed, the kernel and the tool versions that produced them.
- **Assertions understand noise.** Network measurements vary. Assertions can use repetitions, confidence intervals and baselines instead of one fragile number.
- **It fits the tools you have.** A `pytest` plugin and a command line, results as JUnit, JSON and HTML, and a CI that does not need a special machine.

## What works today

Build it (Rust 1.89 or newer, and `ip`, `tc`, `unshare`, `nsenter`, `prlimit` from iproute2 and util-linux), then ask the machine what it can do and run an example. Nothing needs root on a machine that allows user namespaces; `netscenario doctor` says what to change if yours does not.

```bash
cargo build --release          # target/release/netscenario and netscenario-agent
target/release/netscenario doctor
target/release/netscenario run scenarios/lte-like.yaml --seed 42
```

```text
running lte-like for 30s ...
netscenario run: lte-like
  backend kernel (user namespace, iproute2 6.15.0)
  seed 42

workload media  client -> server  2 mbit/s, 1200 byte packets
  sent 6250  received 6197  lost 53 (0.85 %)
  delay  min 30.4 ms  median 42.5 ms  p95 49.5 ms  p99 52.7 ms  max 58.6 ms
  throughput  1.98 mbit/s on average over 30 s (31 bins of one second in result.json)

result: out/result.json
```

The commands are `run`, `validate`, `schema`, `doctor` and `clean`, described in [docs/CLI.md](docs/CLI.md). The result is described field by field in [docs/RESULTS.md](docs/RESULTS.md). A scenario file that is wrong is refused with the line, the column and a hint:

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

## How it is going to work

A sketch, **not implemented** (no timeline, no Gilbert-Elliott loss, no Python). The real format of release 0.1 is described in [docs/SCENARIO-FORMAT.md](docs/SCENARIO-FORMAT.md).

```yaml
# scenarios/lte-burst-loss.yaml
version: 0
name: lte-burst-loss
seed: 42
duration: 30s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:
        delay: 40ms
        jitter: 5ms
        rate: 10mbit
        loss: { model: gilbert-elliott, p: 0.02, r: 0.3 }
      reverse: { delay: 40ms, rate: 20mbit }
timeline:
  - { at: 10s, link: access, set: { forward.rate: 2mbit } }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200 }
```

```python
# test_media.py  (sketch)
def test_media_survives_burst_loss(netscenario):
    run = netscenario.run("scenarios/lte-burst-loss.yaml", repetitions=10)
    assert run.metric("media.delivered_ratio").ci95_low > 0.97
    assert run.metric("media.delay").p99 < 0.150  # seconds
```

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
- [docs/CLI.md](docs/CLI.md): the commands, their options, the exit codes and the JSON they print.
- [docs/FIDELITY.md](docs/FIDELITY.md): how close the Kernel backend is to what a scenario says, measured, and what it does that you may not expect.
- [docs/RESULTS.md](docs/RESULTS.md): the result of a run, field by field.
- [docs/AGENT.md](docs/AGENT.md): the agent in the nodes: the packet, the record file, the command line.
- [docs/DOCTOR.md](docs/DOCTOR.md): `netscenario doctor` and `netscenario clean`: what they check, what they print, their exit codes.
- [docs/SCENARIO-FORMAT.md](docs/SCENARIO-FORMAT.md): the scenario file, draft.
- [docs/PRIOR-ART.md](docs/PRIOR-ART.md): what exists, and where netscenario differs.
- [docs/DECISIONS.md](docs/DECISIONS.md): the decisions made so far, with reasons.
- [spikes/](spikes/): experiments that check what the Linux kernel allows, to run before building.

## Requirements

Linux for the real emulation. The spikes ran on kernels 6.8, 6.12 and 7.0; older kernels are untested. Other systems are targeted by the portable backends later. Rust and Python versions are fixed when the code starts.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and the [code of conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you say otherwise, any contribution you submit for inclusion is licensed as above, without additional terms or conditions.
