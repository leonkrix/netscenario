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

Proposed. A small static Rust binary runs inside every node, so scenarios do not depend on tools installed in the node (iperf and so on) and per-packet records have one format. Your own programs are measured through their output and through packet captures.

## D-009: Call `ip` and `tc` first, netlink later

Proposed (confirm with spike S7). Getting a first, correct kernel backend is worth more than a native implementation. The calls sit behind the backend interface and are replaced by netlink in 0.4 (BKD-05).
Alternative: netlink from the start with `rtnetlink` or `nlink` (better in the end, slower to a first result).

## D-010: Two Python distributions

Proposed. `netscenario` (bindings, API, statistics) and `pytest-netscenario` (the plugin, which depends on it). This follows the naming convention of pytest plugins and lets the API be used without pytest.

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
