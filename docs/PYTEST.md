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

Assertions are plain `assert` on the numbers of the [result](RESULTS.md), which are in SI units (seconds, bits per second, ratios from 0 to 1). The `expect` section of a scenario file and statistical assertions (confidence intervals, baselines) come in later releases.

## The fixture `netscenario`

It gives a `Runner` for the test.

| Call | What it does |
| --- | --- |
| `netscenario.run(scenario, ...)` | Runs a scenario and returns the `netscenario.Result`. `scenario` is a `Scenario` or the name of a file. The options are those of `netscenario.run`: `seed`, `settle`, `packets`, `observer`, `cancel`, and `out` for the directory. |
| `netscenario.load(name)` | Reads and checks a scenario file and returns the `Scenario`, without running it. This works on any machine. |
| `netscenario.results` | The results of the runs of this test so far. |

**Scenario files** are looked for in this order: next to the test file, in the directories of the ini option `netscenario_scenarios` (relative to the ini file), and in the root directory of the project. An absolute path is taken as it is. A file that is not found raises `FileNotFoundError` that lists the places it looked in.

**Every run goes into the temporary directory of the test** (`tmp_path/netscenario/run-<n>-<name>`), so parallel tests never share one, and pytest's own handling of temporary directories decides how long it is kept. The result is a directory with `result.json` and the files that belong to it ([RESULTS.md](RESULTS.md)).

A run can be interrupted: Ctrl-C and `pytest-timeout` stop it, the network is torn down first, and the interrupt is raised afterwards.

## The marker `netscenario` and the fixture `netscenario_result`

`@pytest.mark.netscenario("file.yaml", seed=None, settle=None, packets=False)` names a scenario file for the test, and `netscenario_result` runs it and gives the result. The marker takes exactly one argument and the three options; anything else is an error of the test with a message that says what is wrong.

## A machine that cannot run scenarios

A run needs network namespaces. Before the first run of a session the plugin asks `netscenario.doctor()` (once) and, if the machine cannot run scenarios, **skips** the test with the reason and the fix, or **fails** it. `--netscenario-environment` chooses:

| Value                | Meaning                                            |
| -------------------- | -------------------------------------------------- |
| `auto` (the default) | fail when the variable `CI` is set, skip otherwise |
| `skip`               | always skip                                        |
| `fail`               | always fail                                        |

The rule behind `auto` is the one of the project's own tests: on a laptop without the right to create namespaces a test is skipped with a reason, but on a CI a test that never runs proves nothing, so it fails. Tests that only `load` scenarios are not affected.

## The report

- **Failing tests** show a section `netscenario` with the numbers of every run of the test (the scenario, the backend, the seed, and for each workload the packets, the loss, the reordering and the delay) and the path of `result.json`. Run with `-rA` to see it for passing tests too.
- **JUnit files** (`--junitxml`) get the numbers as properties of the test: `netscenario.<scenario>.<workload>.sent`, `received`, `lost`, `loss_ratio`, `reordered`, `delay_p50_s` and `delay_p99_s`. A CI that reads JUnit can chart them.
- The header of the session names the version of netscenario.

A run that drew its own seed says so (`seed 123 (drawn)`), and a test can repeat it with `netscenario.run(..., seed=result.seed.value)`.

## Options

| Option                                       | Meaning                                                          |
| -------------------------------------------- | ---------------------------------------------------------------- |
| `--netscenario-environment {auto,skip,fail}` | What a test does on a machine that cannot run scenarios (above). |
| `netscenario_scenarios` (ini option)         | Directories in which scenario files are looked for.              |

## Not yet

Parallel tests with `pytest-xdist` work in principle (every run owns its namespaces, and the names include the process), but are not tested before release 0.5 (PYT-07). Repetitions and confidence intervals come with 0.2.
