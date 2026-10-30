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

## D-020: The scenario format of release 0.1, the YAML parser serde-saphyr, and Rust 1.89

Accepted. The research is from 2026-10-30 (crates.io, GitHub, RustSec) and a prototype that read faulty files.

- **The format of release 0.1 is the contract in [SCENARIO-FORMAT.md](SCENARIO-FORMAT.md).** Its rules: strict (unknown and duplicate keys are errors); one spelling per thing; units are lower case and stuck to the number; probabilities only with `%`; rates and sizes decimal, with `kib`, `mib`, `gib` for powers of 1,024; values must be whole numbers of the smallest unit; a value that cannot be applied exactly is refused, not rounded (a jitter that reaches below zero, more than 100,000 packets per second); anchors, aliases, merge keys and tags are refused; the keys of later releases are reserved and say in which release they arrive; `version: 0` until 1.0, and version 0 may change in any minor release. D-007 stays proposed until the model is implemented and this contract has met real files.
- **The parser is `serde-saphyr` 2.** It is written in Rust with `unsafe` forbidden, fuzzed and designed not to panic on malformed input, reads YAML 1.2 (so `no` stays the text "no"), rejects duplicate keys, limits the size of the input, is actively maintained (last change 2026-10-28) and is MIT or Apache-2.0. Every error of the prototype carried the line, the column and the source line with a mark: unknown unit, number without unit, misspelled key with the list of the expected keys, wrong type, duplicate key, broken indentation. Its tree of 40 crates (with serde and schemars) passes `deny.toml`. It gives no logical path for syntax errors, so the path of an error of content comes from our own validation, which keeps the position of every value. The rendering of errors in colour belongs to the command line, not to the model crate.
- **Rejected:** `serde_yaml` (archived), `serde_yml` (archived, RUSTSEC-2025-0068, unsound), `serde_yaml_ng` and `serde_norway` (built on `unsafe-libyaml`, RUSTSEC-2024-0449, unmaintained), `serde_yaml_bw` (hardly used).
- **The lowest supported Rust is 1.89** (D-014 said 1.85; the rest of D-014 stands). `serde-saphyr` needs 1.89. This affects only people who build the crates with an older Rust from their distribution; wheels, binaries and `rustup` users are not affected, and the CI runs 1.99. The version is changed in `Cargo.toml`, the `justfile`, the CI matrix, the README and the contributing guide.
- **The tools of the model** are `schemars` 1 for the JSON Schema (and a test that fails when the committed schema differs from the generated one), `thiserror` for the error types, `insta` for snapshots of the error messages, `proptest` for the round trip from value to text and back, a corpus of faulty files with their expected messages, and our own parsers for the units: `humantime` and `bytesize` accept more spellings than this format allows, and their messages could not be made to name the file.

Alternatives: TOML (D-007 considered it), a parser with an older Rust (an unmaintained foundation), a format with relaxed units (`0.02` as a probability), which would make the same file mean different things to different readers.

## D-021: The model has one set of types, and the positions in the file come from a second reading with the parser

Accepted. The scenario types are the file, the program interface, the builder and the schema in one: they read and write the format, `schemars` describes them, and the checks of the contents work on them whether they came from a file or from code. What a type cannot say is lost on the way from a file: where a value stood. The YAML library reports the position of an error it finds itself but not of a value that it read correctly, and a check of the contents (a node that does not exist) needs that. Wrapping every field in a type that carries its position (`Spanned`) would have given the model a second shape that every new field has to repeat in three places (the wrapped type, the conversion, the writer).

Instead, `source.rs` reads the file a second time with `granit-parser`, the parser that `serde-saphyr` is built on, and notes the position and the length of every key and value under its path (`topology.links[0].between[1]`). The checks of the contents report paths, and the position is looked up by path; a builder gives the same paths without positions. The same pass finds what the format refuses in YAML itself (anchors, aliases, tags, merge keys, nesting deeper than 32 levels), the keys and values of later releases, and the keys that a mapping lacks, each reported at the place that it concerns. The cost is a direct dependency on `granit-parser` (MIT or Apache-2.0, the same version that `serde-saphyr` brings), and the file is parsed twice, which a file of at most 1 MiB makes irrelevant. A test compares the table of required keys with the required keys of the generated schema, so the table cannot drift from the types; a snapshot test with 50 faulty files holds the messages and runs identically in every run.

Alternatives: `Spanned` wrappers in the model (rejected, see above), the validation interface of `serde-saphyr` with `garde` or `validator` (more dependencies, and its paths are mapped back to the file by fuzzy matching), no positions for errors of the contents (the path alone is much harder to use in a long file).


## D-022: A run lives in a PID namespace with the supervisor as its first process, and the supervisor is a child process

Accepted. A run needs a cleanup guarantee that does not depend on the supervisor being able to clean up (BKD-04). Named objects (`ip netns add`, bind mounts) need an exit handler and a `clean` command, and both fail when the process is killed. Instead the run happens inside a PID namespace (`unshare --user --map-root-user --mount --pid --fork --kill-child --mount-proc`, or without the user namespace as root) in which the supervisor is the first process. The nodes are network namespaces held by small processes (`unshare --net` with a program that waits) and addressed by their process id (`ip link set ... netns <pid>`, `nsenter --net`). When the supervisor dies, even by `kill -9`, the kernel ends every process in the namespace, and a network namespace that no process holds is removed with them. Checked on the development machine: in such a namespace the first process has the id 1, and a network namespace can be created and configured in it.

- **The supervisor is a child process** (the same binary with an internal subcommand, started through `unshare`), never a thread of the caller. A process with several threads, such as Python with the bindings, cannot move itself into a new user namespace, and the workspace forbids `unsafe`. The caller talks to the child through a pipe. The command line and the Python package use the same structure.
- **The backend depends on the tools** `unshare` and `nsenter` (util-linux), `ip` and `tc` (iproute2), as D-009 says for the last two. `doctor` checks them.
- **`netscenario clean` is a safety net, not the mechanism.** With no named objects there is nothing to find by name in the normal case. The command looks for processes of a run that lost its supervisor (a run started by an older version, or outside a PID namespace), and a test covers it. This replaces "finds leftovers by name prefix" in the plan.
- **Crash tests** let the supervisor die at every step of the setup and the teardown (`NETSCENARIO_FAIL_AT=<step>`, only in test builds), then check that no process of the run is left and that the next run works, as root and unprivileged.
- **Limits and watchdog (SAF-02, SAF-03):** at most 8 namespaces in a run (release 0.1 needs 2), a maximum run time of the duration of the scenario plus 60 seconds (an option changes the margin), and the watchdog sits in the outer process so that it works when the supervisor hangs. The memory limit of agents comes with the agent and uses `prlimit` because cgroups need rights that the unprivileged mode does not have.

- **What building it settled** (PR 2 of BKD): (1) The supervisor is also in a *network namespace of its own* (`unshare --net`), and at start it refuses to run unless it is process 1 and sees only `lo` (SAF-01). Started in the namespace of the host it does nothing, which a test proves. (2) A node is a network namespace held by a process of the same binary (`hold`) that switches IPv6 off for the new interfaces and says when it is ready; it needs no `sleep` of the system. (3) The failpoints of the crash tests are compiled in all builds and do nothing unless `NETSCENARIO_FAIL_AT` is set: a feature flag would let `cargo test` run no crash tests. (4) `teardown()` reports success when the supervisor died before it could answer, because the kernel has then done what the teardown is for; a death anywhere else is an error that says what the supervisor wrote on its standard error (an `unshare` that is refused says so there, and the message names the AppArmor case). (5) The watchdog kills the `unshare` process of the run; `--kill-child` and the PID namespace end the rest. (6) The links use `10.100.n.0/30` (`.1` the first node of `between`, `.2` the second), fixed MAC addresses and fixed neighbour entries, so that no ARP packet shifts the loss pattern. (7) A direction's `netem` sits on the interface of its sender. (8) `netem` is given `limit` and microsecond times: a time that is not a whole number of microseconds is refused before anything is created (`Backend::check_values`), and the queue limit is derived (see `docs/COVERAGE.md`, "Queue of the Kernel backend before 0.3"). (9) The seed of a direction is derived from the seed of the scenario (`derive_seed`, SplitMix64 mixing), and the values are pinned by a test because a change would give old seeds new patterns.
- **Not done yet:** the memory limit of agents (comes with the agent, 3.5); the result format that records `seed`, `seed_drawn` and `loss_reproducible` (RPT-01, 3.5). Closed after this entry: the root code path is tested (locally as root in a user namespace, on the CI with `sudo`), the tools answer in English (`LC_ALL=C`) because the hints look for their words, and a scenario without `seed` gets a drawn one (D-023).

Alternatives: named namespaces with `ip netns add` and an exit handler (a `kill -9` leaves them behind), cgroups for the cleanup (not available without rights), a library for `unshare` and `setns` such as `nix` in the caller (does not work in a process with threads), a root daemon (against the principle of working without root).

## D-023: A backend declares its capabilities as data, and what depends on the machine is part of the declaration

Accepted. `netscenario-core` defines the interface of a backend (`Backend`), the list of features (`Feature`, one per row of `docs/COVERAGE.md`) and `Capabilities`: for each feature `Exact`, `Limited` (with the restriction) or `Unsupported` (with the reason and the way out). `check` derives the features a scenario needs, each with the first path where it asks for it, and compares them with the declaration. It lists everything that is refused, before anything is created; `provision` calls it first, and tears the backend down if a later step fails. `Limited` is refused unless the scenario accepts it (`allow-limited`, release 0.3), and an accepted restriction is returned so that the result can record it.

- **A declaration depends on the machine.** The Kernel backend builds its declaration from an `Environment` (the version of `tc`). With a `tc` older than iproute2 6.6.0 the feature "same seed, same loss pattern" is `Unsupported`, with the way out: a newer iproute2, leaving out `seed`, or the Engine backend. Where the scenario does not write a `seed`, it asks for no reproducibility and the run is allowed; the result will record that the loss is not reproducible (RPT-01). Refusing every scenario with loss on a stock Ubuntu would be too hard, and recording what happened is not a silent approximation.
- **A scenario without a `seed` gets a drawn one** (`provision`), if the backend can apply a seed, so that every run can be repeated; the seed is part of what `provision` returns, for the result. If the backend cannot, the scenario runs and `loss_reproducible` is `false` for one with loss. This replaces the first wording above ("the result will record that the loss is not reproducible") with what the code does. The seed of a direction is `derive_seed(seed, link, direction)` and only directions with loss get one.
- **The page and the code are compared by a test.** Each feature has the label of its row; the test reads `docs/COVERAGE.md`, and a feature that is `Exact` on a complete machine must be a `✔ 0.1` in the Kernel column. The cell of "same seed, same loss pattern" therefore changed from `~ 0.1` to `✔ 0.1 (needs iproute2 6.6.0 or newer)`: a `~` would make every seeded scenario fail without `allow-limited`, and the restriction (the pattern depends on the number of packets in the queue) is handled by quiet namespaces and a packet counter that marks a run as not reproducible if it was disturbed.
- **Capability errors show the path, and the line when the text is at hand.** `Display` shows `= path: seed`. `CapabilityError::render(name, text)` gives the form of the errors of reading a file, with the line and a mark, through `netscenario_model::Error::in_source`, which looks up the positions of the paths (D-021). The command line will use it.
- **The interface is small and grows.** Waiting for programs and streaming records arrive with the agent, packet capture with release 0.2.

Alternatives: a trait method `supports(feature)` that every backend implements by hand (no table to compare with the page), refusing every `Limited` feature forever (no way to accept a known restriction), a version check in the scenario reader (the model has no access to the machine, by design).
