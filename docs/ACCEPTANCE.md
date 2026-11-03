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
| Tag, release, announcement | The release workflow checks the tag, builds and installs the wheels in clean environments and publishes with trusted publishing; two candidates (`0.1.0-rc.1`, `0.1.0-rc.2`) were published and installed from the registries in a clean environment before `0.1.0` | [RELEASING.md](RELEASING.md) |

### What this does not cover

- **Other machines than the CI images and the development machine.** The fidelity tests ran on the two Ubuntu images of GitHub (kernels 6.8 and 7.0, iproute2 5.15 and 6.1 with a newer `tc` built for the seed) and on Debian 13 (kernel 6.12). A virtual machine with a busy host may show more noise; the bounds leave room only where the noise can only go one way ([FIDELITY.md](FIDELITY.md)).
- **The wheel on the Arm runner** is built and installed into a clean environment and runs a scenario there, but the fidelity cases run on x86_64 only.
- **Containers, other CI systems, Fedora, Arch, Alpine.** [PERMISSIONS.md](PERMISSIONS.md) says what is measured and what is not.
- **Platforms of the release.** Only Linux has wheels and a backend; the candidates were installed and run on one machine (Debian 13, x86_64) by the maintainer. The Arm wheel is installed and run only by the CI.
- **What a person would call "complete".** The ledger lists what 0.1 contains; the README says what it does not do yet. Round trip time, your own program as a node, a timeline, bursty loss and reports are later releases.

## Release 0.2 "Time and noise" (in progress)

The section is completed with the release. What is proven so far:

| Criterion | Proof | Where |
| --- | --- | --- |
| An assertion with a confidence interval is stable over 100 CI runs | The acceptance test runs a scenario eight times with derived seeds and asserts on the upper end of the interval of the loss, as a user would, and a canary shows that a claim that is false for the link fails. The bounds are derived from the exact binomial tails (the stable claim fails with a probability of 6.6e-11 per call). The workflow `stability.yml` runs the test 100 times, each with a new seed, once a week and on request. The test also runs once in every `just py-check`. | [`test_acceptance.py`](../python/netscenario/tests/test_acceptance.py), [`stability.yml`](../.github/workflows/stability.yml), [STATISTICS.md](STATISTICS.md) |

**The first full run of the 100 rounds** (workflow `stability.yml`, started by hand, recorded on 2026-11-03): **0 of 100 rounds failed**. Runner image `ubuntu-24.04` (`ubuntu-24.04-x86_64-1`, runner version 2.338.2). A round (the two tests of the file) took about 32 seconds, the 100 rounds about 55 minutes. Every round drew a new seed, so the stable claim held for 100 different sets of eight runs and the canary failed its false claim every time.

What the proof does not cover: the interval is about the mean of a metric over runs on one machine ([STATISTICS.md](STATISTICS.md#what-the-interval-promises-and-what-it-does-not)); a machine with a busy neighbour can change the delay of a whole set, and the test asserts on the loss, which the seed fixes and the load of the machine does not move. The coverage of the interval for few runs of a skewed metric is below the level and is measured in the same page.
