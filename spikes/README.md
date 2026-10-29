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

The summary is written to `spikes/out/summary.md` (not committed). The workflow `Spikes` in `.github/workflows` runs the same scripts on GitHub's runners (Actions tab, "Run workflow") and keeps the results as an artifact.

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

Two more experiments are not scripts and are described in the plan: whether the packet engine can run on a virtual device inside a user namespace (a prototype in Rust), and which Rust netlink library to use (S7, S8).

## How to report

Paste `spikes/out/summary.md` into the issue or the conversation that asked for it, together with the output of `uname -a` and the name of the distribution. A `FAIL` is a result, not a mistake: it says what the machine does not allow, and the project needs to know.

## Known limits of the spikes

- They were written without being run on every system. If a script itself misbehaves (not the thing it tests), that is a bug to fix; open an issue.
- S2 uses `ping` and therefore counts ICMP packets; ARP and IPv6 traffic are suppressed as well as possible, but a different count of packets before the test can still shift the pattern. The test says so when the result is not valid.
- S4 and S5 measure one machine at one moment. They are for orders of magnitude, not for exact figures; the fidelity tests that come later (QA-06) repeat and publish properly.
