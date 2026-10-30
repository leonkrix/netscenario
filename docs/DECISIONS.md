# Decisions

A log of the decisions that are hard to undo, with the reasons and the alternatives. Add an entry before building on a decision. To change one, add a new entry that supersedes it; do not edit history.

Status: **accepted**, **proposed** (to be confirmed by an experiment or a first implementation), **superseded**.

## D-001: The name is netscenario

Accepted. Descriptive, like the sibling projects, and free on PyPI and crates.io when checked. Components: the Rust crates `netscenario-*`, the command `netscenario`, the Python package `netscenario`, the pytest plugin `pytest-netscenario`.
Alternatives: netweather (memorable, says less), wirelab (sounds like hardware).

## D-002: Rust core with a Python layer

Accepted. The semantics live in one place, in Rust: a static binary for the command line and the agents, precise timers and a fast packet path for the later engine, the same language as the planned Tetrys library, and use from `cargo test`. Python (PyO3 bindings built with maturin, plus a pytest plugin) is how most people will use it.
Alternatives: Python only (faster start, but a weak base for a packet-level engine and for the agents), Go (good network libraries, no other reason).

## D-003: Dual license, Apache-2.0 or MIT

Accepted. Open source, the convention in the Rust ecosystem. Apache-2.0 adds an explicit patent grant, which matters for networking code; MIT keeps it simple for those who prefer it. The same for all crates and Python packages.
Alternatives: MIT only (as the Playwright guards), copyleft licenses (would limit use in company test suites, which are the main users).

## D-004: One repository for the framework; the Tetrys library lives elsewhere

Accepted. Crates, bindings and plugin are released together and share a version history, and maturin expects the Python package next to the crate. The Tetrys library is independent (it should be usable without this framework) and later becomes an example and a test subject here.

## D-005: The kernel is the reference, and backends declare their fidelity

Accepted. The Linux kernel network stack is the ground truth. Every backend publishes what it reproduces faithfully ([COVERAGE.md](COVERAGE.md)). A scenario that asks for more is refused with an error and never approximated silently. Limited features need an explicit `allow-limited`. Other backends are checked against the kernel with differential tests.

## D-006: Linux first

Accepted. The real emulation needs Linux. The development happens on Linux. Windows and macOS are served by the portable proxy backend, with honest limits.

## D-007: The scenario format is versioned YAML with a generated JSON Schema

Proposed (confirm with the first implementation). Readable, diffable, familiar. The schema is generated from the Rust types so that code and documentation cannot drift. Values have strict units. Scenarios are data and cannot run code except through an explicit `command` workload.
Alternatives: TOML (worse for nested time lines), a Python DSL only (ties the format to one language), a custom language (cost without benefit).

## D-008: Traffic and measurement come from agents inside the nodes

Accepted (confirmed by the sender and receiver of spike S8, see D-016). A small static Rust binary runs inside every node, so scenarios do not depend on tools installed in the node (iperf and so on) and per-packet records have one format. Your own programs are measured through their output and through packet captures.

## D-009: Call `ip` and `tc` first, netlink later

Accepted (confirmed by spike S7, see D-014). Getting a first, correct kernel backend is worth more than a native implementation. The calls sit behind the backend interface and are replaced by netlink in 0.4 (BKD-05).
Alternative: netlink from the start with `rtnetlink` or `nlink` (better in the end, slower to a first result).

## D-010: Two Python distributions

Accepted (confirmed by spike S9, see D-014). `netscenario` (bindings, API, statistics) and `pytest-netscenario` (the plugin, which depends on it). This follows the naming convention of pytest plugins and lets the API be used without pytest.

## D-011: The feature ledger and the coverage matrix drive the work

Accepted. `FEATURES.md` and `docs/COVERAGE.md` are the plan that is visible to everybody. A pull request moves lines forward; a feature that is not in the ledger is not planned. A release has acceptance criteria and is not made until they hold.

## D-012: Working practice follows the sibling projects

Accepted. Pull requests into a protected `main`, squash merge, a single required check `CI passed`, Conventional Commits, all actions pinned to commits, releases by tag with provenance and staged approval, documentation written as part of the change. The maintainer's working documents (`PLAN.md`, `CLAUDE.md`) stay local and are listed in `.gitignore`; nothing in the repository refers to them.

## D-013: Findings of the spikes S1 to S6 decide the Kernel backend of release 0.1

Accepted. Measured on Debian 13 (kernel 6.12, iproute2 6.15), on the GitHub runners (Ubuntu 22.04 with kernel 6.8, Ubuntu 24.04 with kernel 7.0) and in Docker; the scripts are in [spikes/](../spikes/README.md) and the consequences are in [COVERAGE.md](COVERAGE.md).

- **Rootless stays in 0.1 (BKD-02).** Unprivileged user namespaces work by default on Debian 13 and Ubuntu 22.04, and on Ubuntu 24.04 after one setting. The tool does not hide the restriction: `doctor` and the error message name what failed, why, and the three ways out (user namespaces, root, capabilities), and the permissions guide (CIX-04) belongs to 0.1. In a container the rootless way needs `SYS_ADMIN` and a seccomp profile that allows `unshare`; this is documented, not worked around.
- **Reproducible loss is limited, not promised without conditions.** `netem ... seed` exists from iproute2 6.6.0 on and gives the same loss pattern in a quiet namespace (checked in 7 runs on each of three machines and across kernels 6.8, 6.12 and 7.0). The pattern depends on the packets that enter the queue, so the Kernel backend uses quiet namespaces, checks the packet counter of the queue, and marks a run that is not reproducible. The coverage cell is `~` until a test of the framework proves it. The `doctor` checks that the installed `tc` knows `seed`.
- **Judging tests use percentiles, never the maximum, and never a minimum from the tail of a distribution.** One packet in about 2000 arrives roughly 2 ms late, and the tables of `netem` are cut at about 3.5 to 4 sigma. The numerical tolerances of QA-06 are set when measurements from the CI runners exist, which are noisier than a development machine.
- **`ip` and `tc` are looked up in `PATH` plus `/usr/sbin` and `/sbin`** (on Debian they are not in the `PATH` of a normal user), and `doctor` prints where it looked.

Alternatives: dropping rootless from 0.1 (would leave the main CI case, an unprivileged user, to the later engine); promising reproducible loss on the Kernel backend without conditions (would be false as soon as a stray packet enters the queue).

## D-014: The spikes S7 to S10 confirm D-009 and D-010, fix the Rust version, and show what the engine still has to solve

Accepted. Measured on Debian 13 (kernel 6.12, Ryzen 7 5800X) in unprivileged user namespaces; the experiments are in [spikes/](../spikes/README.md).

- **D-009 holds: the Kernel backend of 0.1 calls `ip` and `tc`.** Netlink works (spike S7): with `nlink` 0.33.0 the namespaces, the veth pair, the addresses and every `netem` setting that was tried give the same qdisc as the `tc` command line, and `netem seed` gives exactly the loss pattern that `tc` gives. A change of a running `netem` takes 12 microseconds through netlink and through `tc -batch`, against 1.1 ms for one `tc` process per change, so the speed is no reason to hurry. What speaks against netlink now is the library: `rtnetlink` cannot configure `netem` at all (`netlink-packet-route` 0.33.0 knows only `fq_codel` and `ingress`), and `nlink` is ten months old, has one maintainer, changes its API between minor versions and contains `unsafe` code. The calls stay behind the backend interface, and netlink replaces them with BKD-05. Candidates then: `nlink` pinned to an exact version, or our own thin layer for the `netem` options on `netlink-packet-core` and `netlink-sys`. Decide at BKD-05 with the maturity of `nlink` at that time. Either way, a fidelity test must measure the jitter distributions, because `tc qdisc show` does not print the distribution table, and the distribution tables of iproute2 (`/usr/lib/tc/*.dist`) are a run-time dependency of `nlink` unless we embed them.
- **D-010 holds: two Python distributions, built with maturin, abi3.** One wheel (`cp39-abi3`, 256 KB) serves several Python versions; the plugin as a second distribution with a `pytest11` entry point is found by pytest; the interpreter lock can be released while Rust works. Release wheels are built in a manylinux container, not on the development machine, whose tag (`manylinux_2_34`) is too new for older systems. The lowest supported Python version is still to be chosen (3.10 is the system Python of Ubuntu 22.04).
- **Minimum supported Rust version: 1.85 for release 0.1**, with a CI job on exactly that version next to stable. It is what `pyo3` 0.30 and `tokio` need, and it allows edition 2024. If `nlink` is adopted it becomes 1.88. Both CI images have Rust 1.99.1 (spike S10).
- **The userspace engine (BKD-06) is feasible and keeps its place in 0.4.** One thread on AF_PACKET sockets forwarded 100,000 packets per second with a fixed delay of 20 ms and no loss using about 44 percent of one core, and with a seed it drops exactly the same packets in every run. The timer decides the accuracy: `epoll_pwait2` is exact at high rates but 0.3 to 0.7 ms late at 10,000 to 50,000 packets per second (the core sleeps between packets), a spinning loop is exact at the price of a whole core, and `epoll_wait` with milliseconds is too coarse. Open before the design of 0.4: jitter needs a sorted queue, not a FIFO; frames from a veth pair carry unfinished checksums (offload), so a TCP workload needs the engine to complete them, to switch the offload off, or to use a TAP device (which an unprivileged user can create in a user namespace); batching with `recvmmsg` and `sendmmsg`; many flows; bit rates with full-size frames.

Alternatives: netlink from the start (a first Kernel backend would depend on an immature library), a scratch-built netlink layer now (cost without a need), a lower MSRV (it would exclude the current `pyo3`).

## D-015: The run on the CI runners confirms S7 to S9 and qualifies the timer finding of S8

Accepted. The `Spikes` workflow ran S7 to S10 on `ubuntu-22.04` (kernel 6.8, 4 virtual CPUs, AMD EPYC) and `ubuntu-24.04` (kernel 7.0), as the runner user.

- **S7 and S9 gave the same results as on the development machine.** `nlink` made the same qdiscs as `tc` in all ten comparisons, and its seed 1234 gave the same 200-packet loss pattern even with the old `tc` of the images (5.15, 6.1.0), which only reads the result. A change of a running `netem` took 16 to 18 microseconds. The abi3 wheel built and the five tests passed with Python 3.10 and 3.12. Together with spike S2 the complete loss patterns of two seeds are now identical on three machines, kernels 6.8, 6.12 and 7.0.
- **The engine of S8 forwarded 100,000 packets per second on both runners** with no loss and with the delay within 0.04 ms (p99) with `epoll_pwait2` and `spin`, using 3 seconds of CPU in 6 (about half a core). At 150,000 packets per second without delay the 22.04 runner shows the first strain (p99 0.4 ms, p99.9 2.3 ms), so about 150,000 small packets per second is a practical ceiling on a 4-CPU runner for this simple loop. The seeded loss was identical on both kernels and in both runs (3,971 of 40,000 packets for seed 7), and another seed gave another set.
- **The timer finding of D-014 was specific to the development machine.** There, `epoll_pwait2` was 0.3 to 0.7 ms late (p99) at 10,000 to 50,000 packets per second; on the runners it was within 0.2 ms. How late a sleeping core wakes depends on the processor and its idle states, so the engine must report its timer precision per run (ENV-05) and never promise a fixed figure. `spin` stays the most exact choice at the price of a whole core; `epoll_wait` with milliseconds stays too coarse (up to 1 ms late at low rates).
- **The image lists were right about the tools and slightly off about the compiler:** both images have Rust 1.99.1, rustup 1.29.2, clippy and rustfmt; the system Python is 3.10.12 and 3.12.3; gcc is 11.4.0 and 13.3.0; glibc is 2.35 and 2.39; Docker 28.0.4. The wheel is tagged `manylinux_2_34` on both images and on the development machine (glibc 2.41), because the tag follows the symbols that the build links against; wheels for older systems need a build in a manylinux container.
- **The corrected spike runs show failures honestly:** on `ubuntu-24.04` with the default settings the runner reports 5 FAIL (S1.1 and S2.0 to S5.0, no namespace can be created), where before the fix only S1.1 was reported and S2 to S5 recorded nothing at all.

## D-016: D-008 is accepted; D-007 stays proposed until the first implementation

Accepted. The phase of the spikes ends with D-008 accepted and D-007 open on purpose.

- **D-008 (agents inside the nodes) holds.** The sender and the receiver of spike S8 are small Rust programs that run inside a node, put a sequence number and the send time into every packet, and record the arrival time. At 100,000 packets per second they measured the one-way delay of a direct veth pair as 2 microseconds on average (p99 3 microseconds) on the development machine and 4 to 5 microseconds on the CI runners, with no packet lost, and a delay of 20 ms through the engine within 0.04 ms (p99). No tool had to be installed in the node. Not yet checked: that the agent is a static binary (musl) that the supervisor can start by path in any node, the documented wire format, and the delivery of the binary next to the command line; these belong to section 3.5 of the plan and the acceptance of release 0.1.
- **D-007 (versioned YAML with a generated JSON Schema) stays proposed.** No experiment can show whether the format is good; the first implementation of the model (SCN-01 to SCN-04) and the tests of its error messages will. The decision is confirmed or replaced when the model of release 0.1 is finished.

Alternatives: accepting D-007 now (would record a decision that nothing has tested), leaving D-008 proposed (the measurement it waited for exists).

## D-017: The lowest supported Python is 3.11, and the rule is the oldest version that has not reached its end of life

Accepted. Python 3.9 reached its end of life on 2025-10-31 and 3.10 on 2026-10-01, so release 0.1 starts at **3.11** (end of life 2027-10), built as one `abi3` wheel (`abi3-py311`) that also serves the newer versions. The system Python of Ubuntu 22.04 (3.10) is no reason to go lower: the CI installs the Python it needs, and users of that system can use `uv` or another installer. The rule for later releases is that the lowest supported version is the oldest one that has not reached its end of life when the release is made; raising it is allowed in a minor release before 1.0 and is written in the changelog. The `abi3-py39` setting of spike S9 was a trial and is not carried over.

Alternatives: 3.10 (already past its end of life), 3.12 (a longer window, but it would leave out users who still run 3.11 for no gain).

## D-018: The Python side uses uv, maturin, hatchling, ruff and pyright in strict mode

Accepted. The tools of the Python packages are chosen for the strictness of the checks and for one reproducible environment, and they are cheap to change while there is little code.

- **uv** manages the environment as a workspace of the two packages (D-010) with one lock file, `uv.lock`, which is committed. It also fetches the lowest supported Python (3.11, D-017) for `just py-test-min`.
- **maturin** builds `netscenario`, the package with the native module from `crates/netscenario-py`; its version is the one of the Rust workspace, so the crates, the bindings and the plugin are released together (D-004). The `extension-module` feature of PyO3 is not set in `Cargo.toml` but by maturin, so that `cargo test` still links. **hatchling** builds the plugin, which is pure Python; a test checks that both packages have the same version.
- **ruff** formats and lints (security, annotations, docstrings in the Google style, naming, bugbear, simplifications, pytest style).
- **pyright in strict mode** checks the types, with `py.typed` and a stub for the native module. It is chosen over mypy because it is faster, strict mode is its normal way of working, and it is the checker that most editors use. The cost is that the PyPI package fetches a Node runtime on first use; this is no problem on a development machine or a CI runner. mypy would be the choice for a build without network access.
- **The tests of the plugin** run pytest inside pytest (`pytester`), and each check that judges has a canary that must fail.

The wheels that `uv build` makes on the development machine carry the platform tag of that machine (`linux_x86_64`). Release wheels are built in a manylinux container (REL-01, PYT-08).

Alternatives: mypy (the long-standing standard, slower, strict mode is a collection of flags), pip with venv and a requirements file (no lock for the workspace), setuptools for the plugin (works, was used in spike S9, but hatchling needs no `setup.py` history).

## D-019: The bindings are built as an extension module without a Python interpreter, and the crate has no Rust tests

Accepted. The first CI run failed on Ubuntu 22.04: its system Python is 3.10, and PyO3 looks for `python3` even for an `abi3` build and refuses an interpreter below the minimum that `abi3-py311` implies (3.11). The same would have hit every developer on that system. `.cargo/config.toml` now sets `PYO3_NO_PYTHON` and `PYO3_BUILD_EXTENSION_MODULE` as defaults, so that cargo neither looks for nor links a Python interpreter; maturin gives PyO3 its own configuration when it builds the wheel and is not affected. An extension module cannot be linked into a test program without Python, so `netscenario-py` has `test = false` and is covered by the Python tests in `python/`. This replaces the statement in D-018 that maturin alone sets the extension-module mode.

Alternatives: install Python 3.11 in the CI (fixes the CI but not the developer on Ubuntu 22.04), the `extension-module` feature in `Cargo.toml` (still needs the environment variable to avoid the interpreter search, and the feature is being replaced by the environment variable in PyO3).

