# Acceptance of the releases

Every release of the [ledger](../FEATURES.md) has criteria that all have to hold. This page records, for each release, how each criterion is proven and where to look, and what the proof does **not** cover. A criterion without a test or a CI job that can fail is not proven.

## Release 0.1 "First light"

| Criterion | Proof | Where |
| --- | --- | --- |
| A scenario file runs from the command line | `run_writes_the_result_and_prints_the_summary` and the tests of the exit codes run the real program in real namespaces | [`binary.rs`](../crates/netscenario-cli/tests/binary.rs); [`examples/udp-protocol/`](../examples/udp-protocol/) |
| ... and from a test | The Python package runs scenarios, and the plugin runs them as pytest tests; the example test runs in every `just py-check` | [`test_run.py`](../python/netscenario/tests/test_run.py), [`test_plugin.py`](../python/pytest-netscenario/tests/test_plugin.py), [`examples/python-test/`](../examples/python-test/) |
| The same seed twice gives comparable results | The same seed loses the same packets, with one queue and with two (a rate and a jitter), and another seed loses other ones; the run counts the packets of each queue and says if something else entered it (D-036) | [`run.rs`](../crates/netscenario-backend-kernel/tests/run.rs): `the_same_seed_gives_the_same_lost_packets`, `the_same_seed_loses_the_same_packets_through_a_jitter_and_a_rate`, `a_packet_that_no_workload_sent_makes_the_run_not_reproducible` |
| The measured delay, loss and rate match the configured values within the published tolerance, on the CI | Six cases (delay, loss, rate, both jitters, a jitter with a rate) with bounds that are explained, and canaries that must fail; the CI runs them on `ubuntu-22.04` and `ubuntu-24.04`, with the current Rust and with 1.89, and uploads the numbers of each runner | [`fidelity.rs`](../crates/netscenario-backend-kernel/tests/fidelity.rs), [FIDELITY.md](FIDELITY.md) |
| Nothing is left behind after a crash | The supervisor is made to die at each of 18 points of the life of a run, as a user and as root; nothing of the run may be left, and the next run works. A run that lost its supervisor is found and ended by `clean`. A watchdog ends a run that hangs | [`crash.rs`](../crates/netscenario-backend-kernel/tests/crash.rs), [`clean.rs`](../crates/netscenario-backend-kernel/tests/clean.rs), [`safety.rs`](../crates/netscenario-backend-kernel/tests/safety.rs) |
| The documentation is complete for everything in 0.1, the ledger and the coverage matrix are up to date, and each check mark has a test | The README, the guides for permissions and troubleshooting, the reference pages and two tested examples exist. A test of the Kernel backend fails if a faithful mark of the matrix has no link to an existing test, or if the matrix and the declaration of the backend disagree | [`coverage.rs`](../crates/netscenario-backend-kernel/tests/coverage.rs), [COVERAGE.md](COVERAGE.md), [FEATURES.md](../FEATURES.md) |
| Tag, release, announcement | The release workflow checks the tag and builds and installs the wheels in clean environments; a rehearsal runs it without publishing | [RELEASING.md](RELEASING.md) |

### What this does not cover

- **Other machines than the CI images and the development machine.** The fidelity tests ran on the two Ubuntu images of GitHub (kernels 6.8 and 7.0, iproute2 5.15 and 6.1 with a newer `tc` built for the seed) and on Debian 13 (kernel 6.12). A virtual machine with a busy host may show more noise; the bounds leave room only where the noise can only go one way ([FIDELITY.md](FIDELITY.md)).
- **The wheel on the Arm runner** is built and installed into a clean environment and runs a scenario there, but the fidelity cases run on x86_64 only.
- **Containers, other CI systems, Fedora, Arch, Alpine.** [PERMISSIONS.md](PERMISSIONS.md) says what is measured and what is not.
- **The release itself.** The workflow has never published: the rehearsal covers everything up to the upload, and the first publication is the test of trusted publishing on both registries. That is why `0.1.0-rc.1` comes first.
- **What a person would call "complete".** The ledger lists what 0.1 contains; the README says what it does not do yet. Round trip time, your own program as a node, a timeline, bursty loss and reports are later releases.
