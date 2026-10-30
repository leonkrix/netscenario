# Feature ledger

Every feature of netscenario has one line here. The ledger is the answer to "what is planned, what comes when, and what is done". A feature is never only in someone's head: if it is not here, it is not planned.

- **Priority**: `M` must have for 1.0, `S` should have, `C` could have (idea, no promise).
- **Release**: the version that is meant to contain it. `1.x` means after 1.0, `-` means not yet placed.
- **Status**: `idea`, `planned` (placed in a release), `designed` (format or API is written down), `in progress`, `done`.
- A `?` after the release means the plan depends on an experiment (a [spike](spikes/README.md)).
- Where a feature depends on the backend, [docs/COVERAGE.md](docs/COVERAGE.md) says which backends can do it faithfully.

A line moves to `in progress` when the first code of the feature exists, and to `done` when its tests, its documentation and its coverage cells are in.

## Releases

| Release | Theme | What it adds | Acceptance (all must hold) |
| --- | --- | --- | --- |
| 0.1 | **First light** | Kernel backend, point to point, basic impairments, UDP workload, per-packet measurement, JSON results, CLI, minimal Python and pytest | A scenario file runs from the command line and from a test, twice with the same seed gives comparable results, the measured delay, loss and rate match the configured values within the published tolerance on the CI, nothing is left behind after a crash |
| 0.2 | **Time and noise** | Timeline, burst loss, reorder, outages, your own program as the test subject, repetitions and statistics, JUnit, HTML, pcap | A scenario with a bandwidth drop and an outage produces a time series; an assertion with a confidence interval is stable over 100 CI runs |
| 0.3 | **Shapes** | Star, chain with routing, dumbbell, multipath, IPv6, TCP and request/response workloads, profiles, traces, queue disciplines | The classic fairness test (two flows over a dumbbell) and a multipath test run from a file; the profile library has one test per profile |
| 0.4 | **Rootless and exact** | Packet-level engine in userspace (Linux, no root, deterministic), netlink instead of command calls, kernel and engine compared | The same scenario on both backends gives statistically equal results for UDP, and the engine gives the same packet losses for the same seed on every run |
| 0.5 | **Everywhere** | Portable proxy backend, Docker backend, containers as test subjects, sweeps and parallel runs, CI templates | A UDP scenario runs on Windows and macOS through the proxy; a sweep of 100 points runs in parallel on the CI |
| 0.6 | **Motion** | Mobility models and traces, geometry-based links, handover, orbits | A moving node loses and regains connection at the places the geometry says; an imported mobility trace reproduces a published figure |
| 0.7 | **Insight** | Baselines and regression detection, comparison reports, Rust API, plug-in interfaces, schema migration | A regression of 5 % in a metric is found and a run with no change is not flagged in 100 repetitions |
| 0.8 | **Hardening** | Fuzzing, performance work, user guide, cookbook, shell completions, extra workloads | A book with tutorials for each audience; the fuzzers ran for 24 hours without a crash |
| 0.9 | **Release candidate** | Review of every public interface, security review, benchmark against prior art, packaging | Nothing in the ledger with priority `M` is open except the 1.0 review |
| 1.0 | **Stable** | Stability promise for the scenario format, the result format, the Rust and Python API and the command line | See "Criteria for 1.0" below |

## Scenario format (SCN)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| SCN-01 | Scenario file in YAML with a JSON Schema, versioned | M | 0.1 | done |
| SCN-02 | Validation with precise errors (line, column and source line for every error, the path in the file for errors of content, what was expected, how to fix it; reserved keys name their release) | M | 0.1 | done |
| SCN-03 | Units and durations parsed strictly (`40ms`, `10mbit`, `2%`) | M | 0.1 | done |
| SCN-04 | Scenarios built in code (Python and Rust builders), same model as the file | M | 0.1 | done (the Rust builder, and the Python builder, which gives the same canonical form as the file: a test compares them) |
| SCN-05 | Parameters, includes and templates (reuse a link, vary a value) | S | 0.3 | planned |
| SCN-06 | Profile library: 3G, LTE, 5G, Wi-Fi, DSL, GEO and LEO satellite, datacenter, lossy radio | S | 0.3 | planned |
| SCN-07 | Matrices and sweeps (grids, random sampling) | S | 0.5 | planned |
| SCN-08 | Migration between schema versions (`netscenario migrate`) | S | 0.7 | planned |
| SCN-09 | Convert to and from `tc netem` command lines | C | 0.7 | idea |
| SCN-10 | Hints for misspelled names and keys ("did you mean 'server'?"); node names have them since 0.1, keys, units and values follow | S | 0.2 | in progress |

## Topology (TOP)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| TOP-01 | Point to point (two nodes, one link) | M | 0.1 | planned |
| TOP-02 | Star (nodes around a hub) | M | 0.3 | planned |
| TOP-03 | Chain with routing (several hops, router nodes) | M | 0.3 | planned |
| TOP-04 | Dumbbell (shared bottleneck) | M | 0.3 | planned |
| TOP-05 | Multipath and multi-homing (several links between nodes) | M | 0.3 | planned |
| TOP-06 | Arbitrary graph with static routes | M | 0.3 | planned |
| TOP-07 | Dynamic topology: links up and down (0.2), nodes joining and leaving (0.6) | M | 0.2 | planned |
| TOP-08 | IPv4 (0.1) and IPv6 (0.3) | M | 0.1 | planned |
| TOP-09 | NAT and firewall nodes | C | 0.8 | idea |
| TOP-10 | Shared segments, broadcast, multicast | C | 0.8 | idea |
| TOP-11 | Shared wireless medium (contention, hidden terminals) | C | 1.x | idea |

## Link impairments (LNK)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| LNK-01 | Fixed delay | M | 0.1 | planned |
| LNK-02 | Jitter: uniform and normal (0.1), pareto and tables (0.3) | M | 0.1 | planned |
| LNK-03 | Independent loss | M | 0.1 | planned |
| LNK-04 | Burst loss: Gilbert-Elliott and four-state Markov | M | 0.2 | planned |
| LNK-05 | Reordering | M | 0.2 | planned |
| LNK-06 | Duplication | S | 0.2 | planned |
| LNK-07 | Corruption | S | 0.2 | planned |
| LNK-08 | Rate limit (token bucket) | M | 0.1 | planned |
| LNK-09 | Queue limit and queue disciplines (FIFO, CoDel, FQ-CoDel, PIE) | M | 0.3 | planned |
| LNK-10 | MTU and fragmentation | S | 0.3 | planned |
| LNK-11 | Different values per direction | M | 0.1 | planned |
| LNK-12 | Outages (windows without any delivery) | M | 0.2 | planned |
| LNK-13 | ECN marking | C | 0.4 | idea |

## Link models over time (LMD)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| LMD-01 | Static model | M | 0.1 | planned |
| LMD-02 | Scheduled steps | M | 0.2 | planned |
| LMD-03 | Trace-driven rate, delay and loss (including import of Mahimahi traces) | M | 0.3 | planned |
| LMD-04 | Markov models with states and transitions | S | 0.3 | planned |
| LMD-05 | Periodic patterns (a satellite pass, a duty cycle) | S | 0.3 | planned |
| LMD-06 | Geometry-based: distance to loss, delay and rate (free space, log-distance) | M | 0.6 | planned |
| LMD-07 | Plug-in interface for your own model | S | 0.7 | planned |

## Timeline (TML)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| TML-01 | Events at times: change parameters | M | 0.2 | planned |
| TML-02 | Link down and up | M | 0.2 | planned |
| TML-03 | Repeating events | S | 0.3 | planned |
| TML-04 | Events tied to a milestone of a workload (after N packets) | C | 0.7 | idea |
| TML-05 | Change a link from test code while the run is going | S | 0.3 | planned |

## Mobility (MOB)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| MOB-01 | Static positions | M | 0.6 | planned |
| MOB-02 | Paths through waypoints with speed | M | 0.6 | planned |
| MOB-03 | Random waypoint, random walk, Gauss-Markov | S | 0.6 | planned |
| MOB-04 | Import of mobility traces (CSV, BonnMotion, ns-2 setdest) | S | 0.6 | planned |
| MOB-05 | Orbital motion from orbit files (satellites) | C | 0.6 | idea |
| MOB-06 | Handover between access links | M | 0.6 | planned |
| MOB-07 | Three dimensions: altitude, airborne scenarios, delay change from motion | S | 0.6 | planned |

## Workloads (WRK)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| WRK-01 | UDP constant bit rate | M | 0.1 | done (`netscenario run`, tested in real namespaces; [AGENT.md](docs/AGENT.md)) |
| WRK-02 | UDP Poisson, on/off and bursty | M | 0.2 | planned |
| WRK-03 | Replay of a trace of packet sizes and times | S | 0.3 | planned |
| WRK-04 | TCP bulk transfer | M | 0.3 | planned |
| WRK-05 | Request and response (echo, latency) | M | 0.3 | planned |
| WRK-06 | Your own program as a node, with the addresses in its environment | M | 0.2 | planned |
| WRK-07 | A container as a node | S | 0.5 | planned |
| WRK-08 | Replay of a pcap | C | 0.8 | idea |
| WRK-09 | Models of real traffic (video, voice, game) | S | 0.7 | planned |
| WRK-10 | Supervision: start order, readiness checks, orderly stop, captured logs, timeouts | M | 0.2 | planned |

## Measurement (PRB)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| PRB-01 | Per-packet records (sequence, send time, receive time) | M | 0.1 | done (record file, JSON Lines export; [RESULTS.md](docs/RESULTS.md)) |
| PRB-02 | Loss and delivery ratio | M | 0.1 | done (`tests/run.rs`, canaries in `stats.rs`) |
| PRB-03 | One-way delay and round trip time with percentiles | M | 0.1 | in progress (one-way delay with nearest-rank percentiles is done; the round trip time is open) |
| PRB-04 | Jitter (as in RFC 3550) and reordering metrics | S | 0.2 | in progress (the count of reordered packets is in the result; the extent and the RFC 3550 jitter are open) |
| PRB-05 | Throughput and goodput over time | M | 0.1 | done (payload per second; for UDP throughput and goodput are the same number) |
| PRB-06 | Burst statistics (length of loss runs) | S | 0.2 | planned |
| PRB-07 | Time to recover after an event | M | 0.2 | planned |
| PRB-08 | Metrics from your program (JSON on stdout, exit code) | M | 0.2 | planned |
| PRB-09 | Packet capture per link | M | 0.2 | planned |
| PRB-10 | Queue occupancy and drops from the kernel | S | 0.3 | planned |
| PRB-11 | CPU and memory of the program under test | C | 0.8 | idea |

## Assertions and statistics (ASR)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| ASR-01 | Threshold assertions on metrics | M | 0.1 | planned |
| ASR-02 | Repetitions with derived seeds | M | 0.2 | planned |
| ASR-03 | Confidence intervals (bootstrap) | M | 0.2 | planned |
| ASR-04 | Baselines and regression detection (effect size, interval, "erratic" marking) | M | 0.7 | planned |
| ASR-05 | Statistical comparison of two sets of runs (rank test) | S | 0.7 | planned |
| ASR-06 | Policy for flaky runs: rerun and report it | S | 0.5 | planned |
| ASR-07 | Declarative `expect` section in the scenario file | S | 0.3 | planned |
| ASR-08 | Sweeps with shape checks (goodput does not rise with loss) | C | 0.8 | idea |

## Backends (BKD)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| BKD-01 | Kernel backend: namespaces, veth and `netem` as root | M | 0.1 | in progress (tested in the root code path locally; the CI runs the tests as root with `sudo`, to be confirmed by its first run) |
| BKD-02 | Kernel backend in an unprivileged user namespace | M | 0.1 | in progress (tested; to be confirmed on both CI images) |
| BKD-03 | Every backend declares what it reproduces faithfully, and refuses the rest with an error | M | 0.1 | in progress |
| BKD-04 | Cleanup guarantee: no leftovers after a crash (PID namespace, D-022), and `netscenario clean` as a safety net | M | 0.1 | done (the guarantee is tested at 18 steps in `tests/crash.rs`; `clean` in `tests/clean.rs`, D-024) |
| BKD-05 | Netlink instead of calls to `ip` and `tc` | S | 0.4 | planned |
| BKD-06 | Packet engine in userspace (deterministic, no root) | M | 0.4? | planned |
| BKD-07 | Kernel and engine compared by differential tests | M | 0.4 | planned |
| BKD-08 | Portable proxy backend (UDP exact, TCP at stream level) | M | 0.5 | planned |
| BKD-09 | Docker backend (containers as nodes) | S | 0.5 | planned |
| BKD-10 | Nodes on several machines | C | 1.x | idea |
| BKD-11 | Hardware in the loop (a real device behind a physical or virtual port) | C | 1.x | idea |
| BKD-12 | Exchange of traces with ns-3 | C | 1.x | idea |
| BKD-13 | An engine based on eBPF/XDP | C | 1.x | idea |

## Reproducibility and environment (ENV)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| ENV-01 | Seeds: one per scenario, derived for each component | M | 0.1 | done (derivation pinned, drawing and recording in the result, `tests/run.rs`; [RESULTS.md](docs/RESULTS.md)) |
| ENV-02 | Provenance in every result (versions, kernel, `tc`, CPU, backend, seeds, scenario hash) | M | 0.1 | done |
| ENV-03 | Replay a run from its result file | S | 0.2 | planned |
| ENV-04 | `netscenario doctor`: what this machine can do, and what it cannot | M | 0.1 | done for the Kernel backend ([DOCTOR.md](docs/DOCTOR.md), `tests/doctor.rs`); the probe of other backends comes with them |
| ENV-05 | Report on timer and clock precision | S | 0.3 | planned |
| ENV-06 | Result format with a version | M | 0.1 | done (`format` and `version`, 0 until 1.0) |
| ENV-07 | Reproducibility check: the run compares the packet counter of each queue with the packets of the workload, and marks a run in which something else entered the queue as not reproducible; `doctor` checks that `tc` knows `seed` | M | 0.1 | in progress (the `doctor` check is done; the packet counter of the queues comes with the agent, 3.5) |

## Safety (SAF)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| SAF-01 | Refuse any change to an interface outside the namespaces created by the tool | M | 0.1 | in progress |
| SAF-02 | Limits: number of namespaces, run time, memory of agents | M | 0.1 | done in the backend (namespaces, run time, `Program::memory_limit` with `prlimit`, tested: `tests/safety.rs`, `tests/programs.rs`); a run limits its agents to 1 GiB (`RunOptions::agent_memory_limit`) |
| SAF-03 | Watchdog that ends and cleans up a run that hangs | M | 0.1 | in progress |

## Command line (CLI)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| CLI-01 | `run`, `validate`, `doctor`, `clean` | M | 0.1 | done ([CLI.md](docs/CLI.md), `tests/binary.rs`) |
| CLI-02 | `report`: HTML and Markdown from result files | M | 0.2 | planned |
| CLI-03 | `profiles` and `capabilities`: what exists, what a backend can do | S | 0.3 | planned |
| CLI-04 | `schema`: print the JSON Schema | S | 0.1 | done (`netscenario schema`) |
| CLI-05 | Shell completions and man pages | S | 0.8 | planned |
| CLI-06 | Machine-readable output and documented exit codes | M | 0.1 | done (exit codes in [CLI.md](docs/CLI.md); `--json` on `run`, `validate`, `doctor`, `clean`; the code 1 comes with assertions) |

## Python (PYT)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| PYT-01 | Python package with bindings and API (builder, run, results) | M | 0.1 | in progress (bindings, builder, run with observer and cancel, results and errors are done and tested; [PYTHON.md](docs/PYTHON.md); the wheels follow with PYT-08) |
| PYT-02 | `pytest` plugin: fixtures, markers, options | M | 0.1 | done (fixture `netscenario`, marker and `netscenario_result`, the guard, report section and JUnit properties; [PYTEST.md](docs/PYTEST.md), tested with `pytester` in real runs) |
| PYT-03 | Typed API (`py.typed`, checked with a strict type checker) | M | 0.1 | done (`pyright` strict over the package and its tests, part of `just check`) |
| PYT-04 | `asyncio` friendly API | S | 0.3 | planned |
| PYT-05 | Per-packet data as pandas and polars frames | S | 0.3 | planned |
| PYT-06 | Notebook helpers for plots | C | 0.8 | idea |
| PYT-07 | Safe with `pytest-xdist` (parallel runs) | M | 0.5 | planned |
| PYT-08 | Wheels for manylinux on x86_64 and aarch64 | M | 0.1 | planned |

## Rust API (RST)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| RST-01 | Documented library API for `cargo test` | S | 0.7 | planned |
| RST-02 | Test macros | C | 0.8 | idea |

## Reports (RPT)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| RPT-01 | Results as JSON, per-packet data as JSON Lines | M | 0.1 | done (`result.json`, `--packets`; [RESULTS.md](docs/RESULTS.md)) |
| RPT-02 | JUnit XML | M | 0.2 | planned |
| RPT-03 | HTML report with time series and distributions | M | 0.2 | planned |
| RPT-04 | Markdown for the summary of a CI run | S | 0.2 | planned |
| RPT-05 | Comparison of two runs or two backends | S | 0.7 | planned |
| RPT-06 | OpenMetrics export | C | 0.8 | idea |
| RPT-07 | Trends over many runs | C | 1.x | idea |

## Continuous integration (CIX)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| CIX-01 | GitHub Actions example and composite action | S | 0.5 | planned |
| CIX-02 | GitLab CI example | C | 0.5 | idea |
| CIX-03 | Container image with everything installed | S | 0.5 | planned |
| CIX-04 | Guide to permissions: user namespaces, `NET_ADMIN`, restrictions of distributions | M | 0.1 | planned |

## Quality of the project (QA)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| QA-01 | Unit tests and property tests | M | 0.1 | in progress |
| QA-02 | Integration tests inside namespaces, on the CI | M | 0.1 | planned |
| QA-03 | Fuzzing of the scenario parser | S | 0.3 | planned |
| QA-04 | Snapshot tests for reports and the schema | S | 0.2 | planned |
| QA-05 | Benchmarks of the engine (throughput, added delay) | S | 0.4 | planned |
| QA-06 | Fidelity tests: measured against configured delay, loss and rate, with published tolerances | M | 0.1 | done ([FIDELITY.md](docs/FIDELITY.md), `tests/fidelity.rs`: delay, loss, rate, both jitters, jitter with a rate; three canaries; CI numbers per runner as artifacts) |
| QA-07 | Supported Rust version policy, checked on the CI, and checks for breaking changes of the API | M | 0.1 | in progress |
| QA-08 | Supply chain: dependency audit and policy, OpenSSF Scorecard, actions pinned to commits | M | 0.1 | in progress |
| QA-09 | Coverage figures (for information) | C | 0.3 | idea |
| QA-10 | Tests on several kernels and distributions | S | 0.5 | planned |

## Documentation (DOC)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| DOC-01 | README, architecture, decisions, scenario format, coverage | M | 0.1 | in progress |
| DOC-02 | User guide with tutorials | M | 0.3 | planned |
| DOC-03 | Cookbook: QUIC, WebRTC, games, IoT, satellite | S | 0.7 | planned |
| DOC-04 | API documentation for Rust and Python | M | 0.1 | planned |
| DOC-05 | Troubleshooting (permissions, kernels, containers) | M | 0.1 | planned |
| DOC-06 | Chapter on fidelity and limits, generated from the coverage matrix | M | 0.4 | planned |
| DOC-07 | Validation report: how well the emulation matches what it claims | C | 0.9 | idea |

## Releases and maintenance (REL)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| REL-01 | Release by tag to crates.io and PyPI with provenance and a staged approval | M | 0.1 | planned |
| REL-02 | Automatic changelog from the history | M | 0.1 | planned |
| REL-03 | Compatibility policy for the scenario format, the result format, the APIs | M | 0.1 | planned |
| REL-04 | Review list for 1.0 | M | 0.9 | planned |

## Integration with other projects (INT)

| ID | Feature | Prio | Release | Status |
| --- | --- | --- | --- | --- |
| INT-01 | Example scenarios for loss recovery (forward error correction), as a test of the Tetrys library | S | 0.8 | planned |
| INT-02 | Scenario library for the research use of the maintainer (airborne networks) | C | 1.x | idea |

## Parking lot

Ideas that are not features yet. Add them here before they are lost; promote them to a line above when they have a use and a release.

- Fault injection at the application level (slow disk, clock skew) next to the network: probably out of scope, noted so the question is answered once.
- Recording the network conditions of a real device and replaying them in a test.
- A public collection of scenarios and profiles that users can share.
- Visual editor for topologies.

## Criteria for 1.0

1.0 is a promise that the scenario format, the result format, the Rust and Python APIs and the command line stay compatible within the major version. It is released when all of these hold:

1. Every line with priority `M` is `done`, and every line with priority `S` is `done` or has a written reason.
2. The [coverage matrix](docs/COVERAGE.md) has no `?` cells, and each cell with a check mark has a test.
3. The fidelity tests are published with their tolerances, on at least two kernel versions.
4. At least two projects other than this one use it in their tests, and what they found is fixed.
5. No breaking change in the public interfaces for three months.
6. A review of the security design and of the documentation by someone other than the author.
