# How the tests are organised

What is tested, where each kind of test runs, how to measure how good the tests are, and what the rules for a new test are. The commands are in [CONTRIBUTING.md](../CONTRIBUTING.md); the decisions behind the CI are in the comments of [`ci.yml`](../.github/workflows/ci.yml).

## The kinds of test

| Kind | Where | What it proves |
| --- | --- | --- |
| Unit tests | in the `src` of every crate (`mod tests`) | The logic of one module: the statistics, the judging of `expect`, the recovery, the capability checks, the readers |
| Tests of a crate | `crates/*/tests/` | The model and the core without a network: the scenario file, its errors (`scenarios/invalid/`, one fault per file with its message in a snapshot), the limits (`limits.rs`), the formats |
| Tests in namespaces | `crates/netscenario-backend-kernel/tests/` | A real run: the network does what the scenario says, the agent measures it, the result says so |
| Fidelity tests | `crates/netscenario-backend-kernel/tests/fidelity.rs` | The network against its configuration to within bounds that are derived and written next to them ([FIDELITY.md](FIDELITY.md)); each runs alone on its machine |
| Python tests | `python/*/tests/`, `examples/` | The Python layer and the plugin; the acceptance of the release (TCP fairness, the formula of Mathis, two paths, the queues) |
| Tests of the documents | `python/netscenario/tests/test_ledger.py`, `test_book.py`, `crates/netscenario-backend-kernel/tests/coverage.rs` | The ledger names a test for every `done` row, the matrix of coverage agrees with the code, the scenarios and the Python of the guide run |
| Fuzzing | `fuzz/` (weekly, [`fuzz.yml`](../.github/workflows/fuzz.yml)) | The readers of input that is not trusted do not panic |

A test that judges something (a tolerance, a statistic, a capability check, a limit) has a canary: a test that shows it can fail, with a value on the wrong side of the bound.

## Where a test runs

The CI runs what a change needs (`.github/scripts/ci_changes.py`). A test that does not depend on the version of Python or of Rust runs once.

| Job | Runs | Why there |
| --- | --- | --- |
| Tests (2 images, 2 parts) | the quick tests of all crates, on the current stable Rust | Behaviour of the kernel differs by image |
| Tests as root | the Kernel backend and agent tests again with the rights of root | The two ways of running a scenario, D-022; not cut down to chosen tests |
| Lowest supported Rust (1.89) | everything compiles; the tests of the model and the core | The compiler is what the version changes |
| Fidelity (2 images, 2 parts) | the fidelity tests | They measure the network, which does not depend on the Rust version |
| Python 3.11 to 3.14 | `just py-test-versions`: all but the tests marked `once` | Threads, `asyncio` and typing differ by version |
| Acceptance (2 jobs) | the tests marked `once`: TCP acceptance, the run of the guide, the release scripts | Long, statistical, and the same on every version |

## How good are the tests?

Two measures, and neither is a goal in itself: coverage says what the tests never run, a mutation test says whether what they run is checked.

### Coverage

```bash
just coverage        # Rust: lines of the crates as the CI runs the tests
uv run --with coverage coverage run --branch -m pytest python -q && uv run --with coverage coverage report -m   # Python
```

The tool counts the `#[cfg(test)]` modules in `src` as code that is covered, which flatters it; leave those lines out when you judge a crate. Measured on 2026-11-11 without the tests marked `once` and without the fidelity tests: **90.7 % of the production lines of the Rust crates** (agent 93.5, Kernel backend 91.6, core 91.9, model 90.4, command line 82.6), and **95 % of the Python layer** with branches. The gaps are mostly error branches (a failing read, a bad option at the command line), which are tested where a user meets them and not everywhere. Not measured: the code that runs only with the rights of root, and the Python extension (`netscenario-py`), which the Python tests cover.

### Mutation tests

A mutation test changes the code in small ways (`<` to `<=`, a result replaced by an empty string, a `match` arm deleted) and runs the tests of the crate on each change. A change that no test notices is a survivor: either a test is missing, or the change does not change what the code does (an equivalent mutant).

```bash
cargo install cargo-mutants --locked
just mutants netscenario-core crates/netscenario-core/src/stats.rs     # the survivors are in missed.txt
```

It is slow, so it is run on the files that judge, one at a time, by hand and not in the CI: `stats`, `expect`, `recovery` and `capability` of the core, and `validate`, `units` and `reserved` of the model (about 700 mutants, an hour and a half with two workers). The first run found 81 survivors; each one got a test or an explanation. The second run left 8, all explained below (the timeouts of two loop mutants in `stats.rs` are not survivors: the mutant never ends).

**Do not run it with the default settings on a machine with little memory.** Each worker builds its own copy of the tree, by default in `/tmp`, which is a `tmpfs` (memory) on many systems; three workers and their compilers used all memory of the development machine. `just mutants` puts the copies on the disk and keeps the build and the tests to a few threads; for a long run start it under `systemd-run --user` with `-p MemoryHigh=12G -p MemoryMax=16G -p MemorySwapMax=0 -p CPUQuota=800%`, which also lets it survive the end of a session.

**Equivalent mutants**, which no test can notice and which are left alone:

- `stats.rs`, the division by the width of a throughput bin: the bin is fixed at one second, so a division and a multiplication give the same number.
- `expect.rs`, the tolerance of `1e-12` in the comparison of the level of an interval: the comparison is made on levels that are 0.95 or differ by far more.
- `expect.rs`, the arm for `value` in the judging of a set: a file that asks a set for `value` is refused when it is read, so the arm is a defence that no scenario reaches.
- `validate.rs`, the check that an outage starts before another one ends: the steps are walked in time order, so the other outage never starts later than this one and the comparison is true whatever the operator.
- `validate.rs`, the arm for a `table` jitter without a table: the reach of a missing table is zero, which accepts every delay, as the arm does.
- `validate.rs`, the `&&` in the filter that decides whether the path of a workload is looked up: for a workload with an unknown node or the same node at both ends the lookup fails and gives no path, as the filter does.

A cargo-mutants result is relative to the tests it runs: by default the tests of the crate of the file. A survivor may be caught by the tests of another crate (the command line, Python), which is a weaker test than one next to the code.

## The rules for a new test

- **Name the bound.** Bounds are derived, not picked by eye: the model, its deviation and the size of the run are written next to the number. A delay has an exact lower bound and room only upward.
- **Test the edge.** For a limit, the value on the edge is accepted and the next one is refused with the reason (`crates/netscenario-model/tests/limits.rs`).
- **Look for the words of a refusal**, so that another fault in the same file cannot stand for it.
- **Say why a test is slow** if it takes longer than five seconds, in its doc comment, and mark it `once` (Python) or let it run alone (`.config/nextest.toml`) if it measures time.
- **A test that does not depend on the version runs once.** A new CI job goes into `needs` and, if code changes need it, into `must` of `ci-ok`.
- **Link the test where the ledger asks for it.** A row of [FEATURES.md](../FEATURES.md) that says `done` names the test that proves it, and a faithful mark in [COVERAGE.md](COVERAGE.md) links to it; tests check both.
