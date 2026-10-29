# Prior art

What exists, what it does well, and where netscenario is meant to differ. Checked in October 2026 with a web search; statements from memory are marked. Corrections are welcome.

## Summary

Every piece exists somewhere: the kernel can emulate a bad link, there are wrappers around it, there are research testbeds, there are deterministic simulators for Rust code. What is not there (as far as the search found) is the combination: **a declarative scenario, a program of any language as the test subject, per-packet measurement, statistical assertions, reproducibility records, and a pytest and CI fit.** That combination is the project.

## The tools

| Tool | What it is | What to learn from it | Where netscenario differs |
| --- | --- | --- | --- |
| `tc netem` ([man page](https://man7.org/linux/man-pages/man8/tc-netem.8.html)) | Kernel emulation of delay, loss (including Gilbert-Elliott and four-state models), reordering, duplication, corruption; newer versions have a [seed](https://man.archlinux.org/man/tc-netem.8.en) for reproducible loss ([kernel patches](https://lkml.iu.edu/hypermail/linux/kernel/2308.2/00277.html)) | The reference for the Kernel backend; the model names | It is a mechanism, not a test: no scenario, no measurement, no assertion |
| [pynetem](https://pypi.org/project/pynetem) | Python wrapper around `tc`, with a command line | Simple settings in the format of `tc` | Needs root, no pytest, no measurement |
| [linkem](https://docs.rs/crate/linkem/latest) | Rust library: one namespace per peer, a hub, per-link `netem` settings | The closest relative; the namespace-per-peer layout | A library without measurement, assertions or pytest |
| [netem for Go](https://pkg.go.dev/github.com/kasader/netem) | Wraps `net.Conn` and `net.PacketConn` in the process, no admin rights, live changes | Rootless operation for code that can be wrapped | Only code of the same process and language; no kernel behaviour |
| [pytest-asyncio-network-simulator](https://pypi.python.org/pypi/pytest-asyncio-network-simulator) | Network in memory for asyncio tests | Test ergonomics | Not a real packet path |
| [slow-network](https://github.com/j1elo/slow-network) | Shell script for netem on one interface | Shows how often people write this by hand | Egress only, a script |
| [QUIC Interop Runner](https://www.ietf.org/blog/quic-automated-interop-testing/) | Python, Docker and ns-3: interop tests between QUIC implementations under set network conditions | Proof that scenario-based protocol testing is valuable; fixed scenarios, repeatable | Specific to QUIC interop; scenarios are not user-defined |
| Mahimahi (see [this paper](https://arxiv.org/pdf/2603.04381)) | User-space shells for delay, trace-driven rate, loss; record and replay | Trace-driven links; the trace format netscenario imports | A research tool without a test layer |
| [CoCo-Beholder](https://mpi-inf.mpg.de/inet/software/coco-beholder), Pantheon ([no longer supported](https://www.mpi-inf.mpg.de/departments/inet/benchmarking-congestion-control-algorithms)) | Congestion control evaluation with a dumbbell topology | The dumbbell experiment as a standard shape; reproducibility of results | Specific to congestion control, not maintained |
| [turmoil](https://docs.rs/turmoil/0.7.2) | Deterministic simulation of networked Rust code with a seeded random source | Seeded determinism; [practice shows](https://s2.dev/blog/dst) that it is easy to lose | Only for code written against its socket types |
| [Toxiproxy](https://dev.co/testing/open-source/toxiproxy), [Chaos Mesh](https://chaos-mesh.org/docs/simulate-network-chaos-on-kubernetes/) | TCP proxy for faults; fault injection in Kubernetes with `netem` | The idea of faults as test inputs | TCP level only, or tied to a cluster |
| Mininet, Containerlab, Shadow (from memory, not checked in this search for Shadow; Containerlab is [described here](https://brianlinkletter.com/2023/02/network-emulators-and-network-simulators-2023/)) | Topology emulation in namespaces and containers; Shadow runs real programs in a simulated network | Topology descriptions; running real code in a controlled network | Network-lab and research tools, not test frameworks |
| ns-3 | Discrete-event network simulator | Mobility models, propagation models, trace formats | A simulator, not real code on a real stack |

## What the experiments of others teach

- **Determinism is fragile.** The [report on turmoil](https://s2.dev/blog/dst) found sources of nondeterminism that the simulator did not cover (timestamps in packets, scheduling). netscenario therefore records what is and is not deterministic per backend, instead of promising it.
- **Noise needs statistics.** Regression detection in performance testing uses an effect size and a confidence interval, and marks unstable experiments ([Datadog's detector](https://github.com/DataDog/datadog-agent/pull/25827) is an example). The same applies to network measurements.
- **Fixed scenarios age.** The QUIC runner's scenarios serve one purpose well. A scenario language lets the next purpose be a file.

## Open questions about prior art

- Whether a pytest plugin for `netem` scenarios exists that the search did not find.
- How close Mininet-based test harnesses come to the same combination.
- Licensing and format of the traces that are in circulation (Mahimahi, cellular datasets) before they are bundled.
