# Spikes

Small experiments that answer questions the design depends on, **before** the code is written. Each one is a script that prints `PASS`, `FAIL`, `WARN` or `INFO` lines. The results decide what goes into release 0.1 and which cells of the [coverage matrix](../docs/COVERAGE.md) are real.

Nothing here changes the network of your machine. Every check runs in network namespaces that exist only while it runs (`unshare`), and nothing is installed.

## What they need

A Linux machine with `iproute2` (`ip`, `tc`), `util-linux` (`unshare`, `nsenter`), `iputils-ping` and `python3`. On Debian and Ubuntu: `sudo apt install iproute2 iputils-ping python3 util-linux procps`.

## Run them

```bash
bash spikes/run-all.sh          # as yourself: tests what an unprivileged user can do
sudo bash spikes/run-all.sh     # as root: tests the root case (use SPIKE_OUT=out-root to keep both results)
bash spikes/s6-docker.sh        # optional: what works inside a Docker container
```

The summary is written to `spikes/out/summary.md` (not committed). The workflow `Spikes` in `.github/workflows` runs the same scripts, and the Rust and Python experiments S7 to S10 below, on GitHub's runners (Actions tab, "Run workflow") and keeps the results as an artifact.

Run one spike alone with `bash spikes/s2-netem-seed.sh`.

## The questions

| Spike | Question | Decides |
| --- | --- | --- |
| S1 `s1-userns.sh` | Can an unprivileged user create namespaces, a veth pair and a `netem` qdisc here? Which tools and settings are present? | Whether release 0.1 can work without root (BKD-02); the text of the permissions guide (CIX-04) |
| S2 `s2-netem-seed.sh` | Does `netem ... seed N` give the same lost packets for the same seed? | Whether the Kernel backend can promise reproducible loss; the "Reproducibility" rows of the coverage matrix |
| S3 `s3-netem-features.sh` | Which impairments and queue disciplines can be configured (delay distributions, Gilbert-Elliott, four-state loss, reordering, rate, `tbf`, `fq_codel`, `cake`, `ifb`)? | The cells of the Kernel column for impairments and queue disciplines |
| S4 `s4-delay-precision.sh` | How exact is the delay a UDP stream sees through `netem`, and how big is the noise and the overhead of the path? | The tolerances of the fidelity tests (QA-06); how the agent measures |
| S5 `s5-change-rate.sh` | How fast can the parameters of a running `netem` be changed, one by one and in a batch? | What "trace-driven" can mean on the Kernel backend (LMD-03) |
| S6 `s6-docker.sh` | What works inside a container with and without extra rights? | The Docker rows of the matrix; the CI guide |

Four more experiments are small Rust or Python projects, each with its own `run.sh`. They are scratch crates, not part of any workspace, and their build output (`target/`) is not committed.

| Spike | Question | Decides | Run |
| --- | --- | --- | --- |
| S7 `rust-netlink/` | Can a netlink library (`nlink`, `rtnetlink`) build the namespaces and configure `netem` exactly as `ip` and `tc` do? How fast can it change a running `netem`? | Whether the Kernel backend calls `ip` and `tc` or speaks netlink (D-009); BKD-05 | `bash spikes/rust-netlink/run.sh` |
| S8 `rust-engine/` | Can a userspace packet engine inside an unprivileged user namespace forward about 100,000 packets per second on one core, with an exact delay and a seeded loss? Which timer? | Whether the Engine backend (BKD-06) is feasible and how its data path is built | `bash spikes/rust-engine/run.sh` (about 3 minutes; `QUICK=1` for a short run) |
| S9 `rust-pyo3/` | Does a PyO3 extension built with maturin work as one wheel for several Python versions, with the pytest plugin in a second distribution? | The layout of the Python packages (D-010) | `bash spikes/rust-pyo3/run.sh` (needs network access to PyPI and crates.io) |

Four more are for release 0.2 and are about time, the models of `netem`, capture and programs. They need a `tc` that knows `netem ... seed` (iproute2 6.6.0 or newer); the workflow `Spikes` builds one for the runners.

| Spike | Question | Decides | Run |
| --- | --- | --- | --- |
| S11 `rust-timeline/` | How fast can a running `netem` be changed through a persistent `tc -batch -` or one `tc` process each time? Does a change that the sender asks for at a packet land exactly on that packet, compared with a timer? Does the same seed give the same lost packets when the parameters change during the run? | How the timeline is applied (D-038) | `bash spikes/rust-timeline/run.sh` (about 12 minutes; `QUICK=1` for 4) |
| S12 `s12-netem-models.sh` | Do the loss models, reordering, duplication and corruption of `netem` do what their parameters say? What does the correlation argument do? | The names and meanings of the parameters in the format, the fidelity cases, what is refused (D-040, D-048) | `bash spikes/s12-netem-models.sh` (about 8 minutes) |
| S13 `rust-capture/` | Can the agent capture from a veth interface with `AF_PACKET`, without `unsafe` code of ours, with kernel timestamps and a pcap that `tcpdump` reads? What does a capture show, and how complete is it under load? | Where capture lives and what it promises (D-047) | `bash spikes/rust-capture/run.sh` (about 2 minutes; needs `tcpdump` for the comparison) |
| S13b `s13b-corruption-accounting.sh` | With corruption, can lost and corrupted packets be told apart, exactly and by sequence number? Which `netem` settings survive a change that does not mention them? | The accounting of corruption in the result (D-040), the whole-state rule for changes (D-048) | `bash spikes/s13b-corruption-accounting.sh` (about 1 minute; needs `rust-capture` built) |
| S14 `s14-program-node.sh` | Can the supervisor tell from outside that a program in a node is ready? Does a stop reach a program and its children? What does the output of a chatty program do to the reader? | The probes, the stop and the output of `kind: command` (D-041) | `bash spikes/s14-program-node.sh` (under a minute) |

S10 (the toolchain of the CI images and the oldest Rust version that the dependencies allow) is a reading of the published image lists and of the dependency manifests, recorded in the decisions and not a script.

## How to report

Paste `spikes/out/summary.md` into the issue or the conversation that asked for it, together with the output of `uname -a` and the name of the distribution. A `FAIL` is a result, not a mistake: it says what the machine does not allow, and the project needs to know.

## Known limits of the spikes

- They were written without being run on every system. If a script itself misbehaves (not the thing it tests), that is a bug to fix; open an issue.
- S2 uses `ping` and therefore counts ICMP packets; ARP and IPv6 traffic are suppressed as well as possible, but a different count of packets before the test can still shift the pattern. The test says so when the result is not valid.
- S4 and S5 measure one machine at one moment. They are for orders of magnitude, not for exact figures; the fidelity tests that come later (QA-06) repeat and publish properly.
