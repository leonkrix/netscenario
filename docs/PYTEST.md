# The pytest plugin

`pytest-netscenario` makes a network scenario a normal test. It depends on the `netscenario` package ([PYTHON.md](PYTHON.md)) and is found by pytest through its entry point, so installing it is all that is needed: `pip install pytest-netscenario` installs both, in exactly the same version. In a checkout, `uv sync` installs both.

```python
# test_media.py
import pytest


def test_media_survives_the_lte_link(netscenario):
    result = netscenario.run("lte-like.yaml")          # looked for next to this file
    media = result["media"]
    assert media.loss_ratio < 0.02
    assert media.delay is not None
    assert media.delay.p99 < 0.150                      # seconds


@pytest.mark.netscenario("lte-like.yaml", seed=42)
def test_with_the_marker(netscenario_result):           # the result of the scenario of the marker
    assert netscenario_result["media"].sent > 0
```

Assertions are plain `assert` on the numbers of the [result](RESULTS.md), which are in SI units (seconds, bits per second, ratios from 0 to 1). The `expect` section of a scenario file (D-094) is judged by the core: a test that ran a file with checks **fails when a check does not hold**, with the checks that do not, also when its body asserts nothing; a file whose checks need a set of runs is run by `netscenario_runs` without `repetitions` in the marker, and `netscenario_result` refuses it. In the JUnit file the checks are in the failure of the test. Baselines come in a later release.

## The fixture `netscenario`

It gives a `Runner` for the test.

| Call | What it does |
| --- | --- |
| `netscenario.run(scenario, ...)` | Runs a scenario and returns the `netscenario.Result`. `scenario` is a `Scenario` or the name of a file. The options are those of `netscenario.run`: `seed`, `settle`, `observer`, `cancel` and `out` for the directory, and the arguments that replace a key of the section `measure` of the scenario for this run (`bin`, `recovery_basis`, `recovery_baseline`, `recovery_window`, `recovery_fraction`, `capture`, `packets`; [SCENARIO-FORMAT.md](SCENARIO-FORMAT.md#what-is-measured)). |
| `netscenario.load(name)` | Reads and checks a scenario file and returns the `Scenario`, without running it. This works on any machine. |
| `netscenario.run(scenario, repetitions=n, ...)` | Runs the scenario `n` times (5 to 1000) with derived seeds and returns a `netscenario.RunSet` with an interval for each metric, see [PYTHON.md](PYTHON.md#repetitions-and-confidence-intervals) and [STATISTICS.md](STATISTICS.md). `statistic` (`"mean"` or `"median"`) and `confidence` (0.95) go with it. |
| `netscenario.results` | The results of the runs of this test so far (a set is one entry). |

**Scenario files** are looked for in this order: next to the test file, in the directories of the ini option `netscenario_scenarios` (relative to the ini file), and in the root directory of the project. An absolute path is taken as it is. A file that is not found raises `FileNotFoundError` that lists the places it looked in.

**Every run goes into the temporary directory of the test** (`tmp_path/netscenario/run-<n>-<name>`), so parallel tests never share one, and pytest's own handling of temporary directories decides how long it is kept. The result is a directory with `result.json` and the files that belong to it ([RESULTS.md](RESULTS.md)).

A run can be interrupted: Ctrl-C and `pytest-timeout` stop it, the network is torn down first, and the interrupt is raised afterwards.

## The marker `netscenario` and the fixtures `netscenario_result` and `netscenario_runs`

`@pytest.mark.netscenario("file.yaml", seed=None, settle=None, packets=False)` names a scenario file for the test, and `netscenario_result` runs it and gives the result. The marker takes exactly one argument and the options `seed`, `settle`, and those that replace a key of `measure` (`packets`, `capture`, `bin`, `recovery_basis`, `recovery_baseline`, `recovery_window`, `recovery_fraction`; [RESULTS.md](RESULTS.md)); anything else is an error of the test with a message that says what is wrong.

With `repetitions` (and optionally `statistic` and `confidence`) in the marker the scenario is run as a set, and the fixture `netscenario_runs` gives the `netscenario.RunSet`; `netscenario_result` is then an error that says so, and `netscenario_runs` without `repetitions` is one too:

```python
@pytest.mark.netscenario("lte-like.yaml", repetitions=10, seed=42)
def test_the_loss_is_low(netscenario_runs):
    assert netscenario_runs.metric("media.loss_ratio").ci95_high < 0.05
```

## Parameters, live changes and `asyncio`

- **Parameters.** `netscenario.run("file.yaml", params={"rtt": "40ms"})`, `netscenario.load("file.yaml", params=...)` and the marker `@pytest.mark.netscenario("file.yaml", params={"rtt": "40ms"})` give the values of the parameters of the scenario ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md#parameters)); the result records them (`result.scenario_params`). A refusal of a backend for a scenario that was expanded from a template is shown at the line of the file where the value was written.
- **Changing a link while the run goes** (TML-05, [PYTHON.md](PYTHON.md#changing-a-link-while-the-run-goes)). `netscenario.start("file.yaml", live=["access"])` is `netscenario.start` with the file looked for and the directory chosen as for `run`. The result goes into the report of the test when the test asks for it (`run.result()`, or leaving the `with` block). A run that the test left going is cancelled and torn down when the test ends.
- **`asyncio`.** `await netscenario.run_async(...)` and `await netscenario.start_async(...)` in a test that runs its own loop (`asyncio.run`, or any plugin that does): the same arguments, the same report. The plugin needs no async plugin and does not start a loop.

## A machine that cannot run scenarios

A run needs network namespaces. Before the first run of a session the plugin asks `netscenario.doctor()` (once) and, if the machine cannot run scenarios, **skips** the test with the reason and the fix, or **fails** it. `--netscenario-environment` chooses:

| Value                | Meaning                                            |
| -------------------- | -------------------------------------------------- |
| `auto` (the default) | fail when the variable `CI` is set, skip otherwise |
| `skip`               | always skip                                        |
| `fail`               | always fail                                        |

The rule behind `auto` is the one of the project's own tests: on a laptop without the right to create namespaces a test is skipped with a reason, but on a CI a test that never runs proves nothing, so it fails. Tests that only `load` scenarios are not affected.

## The report

- **Failing tests** show a section `netscenario` with the numbers of every run of the test (the scenario, the backend, the seed, and for each workload the packets, the loss, the reordering and the delay; the **events of the timeline** with how late each was, the time to recover after each event, and how each program of your own ended with the last value of its metrics) and the path of `result.json`; for a set the headline metrics with their intervals and the path of `runset.json`. Run with `-rA` to see it for passing tests too.
- **JUnit files** (`--junitxml`) get the numbers as properties of the test: `netscenario.<scenario>.<workload>.<metric>`: `sent`, `received`, `lost`, `loss_ratio`, `reordered`, `duplicates`, `delay_min_s`, `delay_p50_s`, `delay_p95_s`, `delay_p99_s`, `delay_max_s`, `jitter_mean_s`, `throughput_mean_bps` and `loss_runs_longest` (times in seconds), and `netscenario.<scenario>.<program>.<metric>` for the metrics of a program of your own. The list is the one of `netscenario report --format junit` ([REPORTS.md](REPORTS.md)): it comes from the same native function, so the names cannot differ. A CI that reads JUnit can chart them. For a set of runs they are `netscenario.<scenario>.<metric>.mean`, `.low` and `.high` for the loss, the delay percentiles 50, 95 and 99 and the throughput, and `netscenario.<scenario>.repetitions` and `.confidence`.
- The header of the session names the version of netscenario.

A run that drew its own seed says so (`seed 123 (drawn)`), and a test can repeat it with `netscenario.run(..., seed=result.seed.value)`.

## Options

| Option                                       | Meaning                                                          |
| -------------------------------------------- | ---------------------------------------------------------------- |
| `--netscenario-environment {auto,skip,fail}` | What a test does on a machine that cannot run scenarios (above). |
| `netscenario_scenarios` (ini option)         | Directories in which scenario files are looked for.              |

## Not yet

Parallel tests with `pytest-xdist` work in principle (every run owns its namespaces, and the names include the process), but are not tested before release 0.5 (PYT-07).
