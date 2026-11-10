"""The plugin: options, the marker, the fixtures and the report (PYT-02).

pytest finds this module through the ``pytest11`` entry point. What it offers:

- the fixture ``netscenario``: a :class:`pytest_netscenario.runner.Runner` for the test,
- the marker ``@pytest.mark.netscenario("file.yaml", seed=..., settle=...)`` and the fixtures
  ``netscenario_result`` and ``netscenario_runs`` (with ``repetitions=``), which run the scenario of
  the marker and give the result or the set of results,
- the option ``--netscenario-environment`` and the ini option ``netscenario_scenarios``,
- an attachment to the report of every test that ran a scenario,
- the verdict of the ``expect`` section of a scenario file (ASR-07, D-094): a test that ran a
  file with checks fails when a check does not hold, with the checks that do not.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Generator
from pathlib import Path
from typing import Any, cast

import pytest

import netscenario as ns
from pytest_netscenario import runner as runner_module
from pytest_netscenario.runner import Runner, describe, describe_set, properties, set_properties

_RESULTS: pytest.StashKey[list[ns.Result | ns.RunSet]] = pytest.StashKey()
"""The results of the runs of a test, kept on the test item until its report is made."""

_DOCTOR: pytest.StashKey[ns.DoctorReport] = pytest.StashKey()
"""What ``doctor`` said about this machine, asked once per session."""

_MODES = ("auto", "skip", "fail")
_MARKER_OPTIONS = frozenset(
    {
        "repetitions",
        "statistic",
        "confidence",
        "seed",
        "settle",
        "packets",
        "capture",
        "bin",
        "recovery_basis",
        "recovery_baseline",
        "recovery_window",
        "recovery_fraction",
    }
)
"""The options of ``Runner.run`` that a marker can give (not the observer, token or directory)."""


def pytest_addoption(parser: pytest.Parser) -> None:
    """Add the option and the ini option of the plugin."""
    group = parser.getgroup("netscenario", "network scenarios")
    group.addoption(
        "--netscenario-environment",
        choices=_MODES,
        default="auto",
        dest="netscenario_environment",
        help=(
            "what a test does when this machine cannot run scenarios: 'skip' it, 'fail' it, or "
            "'auto' (the default): fail on a CI (the variable CI is set), skip elsewhere"
        ),
    )
    parser.addini(
        "netscenario_scenarios",
        type="paths",
        default=[],
        help="directories, relative to this file, in which scenario files are looked for",
    )


def pytest_configure(config: pytest.Config) -> None:
    """Register the marker."""
    config.addinivalue_line(
        "markers",
        "netscenario(scenario, *, repetitions=None, statistic=None, confidence=None, "
        "seed=None, settle=None, packets=None, capture=None, bin=None, "
        "recovery_basis=None, recovery_baseline=None, recovery_window=None, "
        "recovery_fraction=None): run this scenario file for the test; the fixture "
        "netscenario_result gives the result, and netscenario_runs the set of results with "
        "repetitions",
    )


def pytest_report_header(config: pytest.Config) -> str:
    """Name the version in the header of the report."""
    return f"netscenario: {ns.__version__}"


def _mode(config: pytest.Config) -> str:
    chosen = str(config.getoption("netscenario_environment"))
    if chosen == "auto":
        return "fail" if os.environ.get("CI") else "skip"
    return chosen


def _guard(config: pytest.Config) -> Callable[[], None]:
    """Make the check of the environment for a runner: asked once, skip or fail if it is not ok."""

    def check() -> None:
        report = config.stash.get(_DOCTOR, None)
        if report is None:
            report = runner_module.doctor()
            config.stash[_DOCTOR] = report
        if report.ok:
            return
        reason = f"this machine cannot run scenarios:\n{report.explain()}"
        if _mode(config) == "fail":
            pytest.fail(reason, pytrace=False)
        pytest.skip(reason)

    return check


@pytest.fixture
def netscenario(
    request: pytest.FixtureRequest, tmp_path: Path, pytestconfig: pytest.Config
) -> Runner:
    """The runner of this test.

    ``netscenario.run("file.yaml")`` runs a scenario file that is looked for next to the test file,
    in the directories of the ini option ``netscenario_scenarios`` and in the root directory.
    ``netscenario.load("file.yaml")`` only reads and checks it. Every run goes into the temporary
    directory of the test.
    """
    item = request.node
    here = request.path.parent
    search = [here, *pytestconfig.getini("netscenario_scenarios"), pytestconfig.rootpath]

    def record(result: ns.Result | ns.RunSet) -> None:
        item.stash.setdefault(_RESULTS, []).append(result)
        # On the item, as `record_property` does: the reports of the test copy them from there, and
        # the JUnit file takes them from the last of them
        found = set_properties(result) if isinstance(result, ns.RunSet) else properties(result)
        item.user_properties.extend(found)

    return Runner(
        out_root=tmp_path / "netscenario",
        search=search,
        guard=_guard(pytestconfig),
        record=record,
    )


def _marker_run(request: pytest.FixtureRequest, fixture: str) -> tuple[str, dict[str, Any]]:
    """The scenario and the options of the marker, or a failure that says what is wrong."""
    marker = request.node.get_closest_marker("netscenario")
    if marker is None:
        pytest.fail(
            f'the fixture {fixture} needs the marker: @pytest.mark.netscenario("scenario.yaml")',
            pytrace=False,
        )
    if len(marker.args) != 1:
        pytest.fail(
            "the marker netscenario takes the scenario file as its one argument: "
            '@pytest.mark.netscenario("scenario.yaml", seed=42)',
            pytrace=False,
        )
    unknown = set(marker.kwargs) - _MARKER_OPTIONS
    if unknown:
        pytest.fail(
            f"the marker netscenario does not know {', '.join(sorted(unknown))}; "
            f"it takes {', '.join(sorted(_MARKER_OPTIONS))}",
            pytrace=False,
        )
    return marker.args[0], dict(marker.kwargs)


@pytest.fixture
def netscenario_result(request: pytest.FixtureRequest, netscenario: Runner) -> ns.Result:
    """The result of the scenario that the marker ``netscenario`` names.

    Example:
        >>> @pytest.mark.netscenario("lte-like.yaml", seed=42)  # doctest: +SKIP
        ... def test_media(netscenario_result):
        ...     assert netscenario_result["media"].loss_ratio < 0.02
    """
    scenario, options = _marker_run(request, "netscenario_result")
    if "repetitions" in options:
        pytest.fail(
            "the marker netscenario has repetitions, so the result is a set: use the fixture "
            "netscenario_runs",
            pytrace=False,
        )
    needed = netscenario.load(scenario).expect_repetitions
    if needed is not None:
        pytest.fail(
            f"the checks in {scenario} need a set of {needed} runs, so the result is a set: use "
            "the fixture netscenario_runs",
            pytrace=False,
        )
    # No repetitions in the marker, so a single run
    return cast("ns.Result", netscenario.run(scenario, **options))


@pytest.fixture
def netscenario_runs(request: pytest.FixtureRequest, netscenario: Runner) -> ns.RunSet:
    """The set of runs of the scenario that the marker ``netscenario`` names, with ``repetitions``.

    Example:
        >>> @pytest.mark.netscenario("lte-like.yaml", repetitions=10, seed=42)  # doctest: +SKIP
        ... def test_media(netscenario_runs):
        ...     assert netscenario_runs.metric("media.loss_ratio").ci95_high < 0.05
    """
    scenario, options = _marker_run(request, "netscenario_runs")
    # The checks of a file with `expect: {repetitions: n}` say how many runs they need (D-094)
    if "repetitions" not in options and netscenario.load(scenario).expect_repetitions is None:
        pytest.fail(
            "the fixture netscenario_runs needs repetitions in the marker: "
            '@pytest.mark.netscenario("scenario.yaml", repetitions=10)',
            pytrace=False,
        )
    # Repetitions are in the marker, so a set
    return cast("ns.RunSet", netscenario.run(scenario, **options))


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(
    item: pytest.Item, call: pytest.CallInfo[None]
) -> Generator[None, pytest.TestReport, pytest.TestReport]:
    """Attach the numbers of the runs of a test to its report as a section.

    The properties for a JUnit file are set on the item when the run is recorded.
    """
    report = yield
    if call.when == "call":
        results = item.stash.get(_RESULTS, None)
        for result in results or ():
            text = describe_set(result) if isinstance(result, ns.RunSet) else describe(result)
            report.sections.append(("netscenario", text))
        # The checks of a scenario file are claims about the runs of the test: when the test body
        # passed and a claim does not hold, the test fails with the claims that do not (D-094)
        broken = [
            result.expect
            for result in results or ()
            if result.expect is not None and not result.expect.passed
        ]
        if broken and report.passed:
            report.outcome = "failed"
            report.longrepr = "\n".join(expect.explain() for expect in broken)
    return report
