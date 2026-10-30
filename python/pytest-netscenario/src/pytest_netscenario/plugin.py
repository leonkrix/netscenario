"""The plugin: options, the marker, the fixtures and the report (PYT-02).

pytest finds this module through the ``pytest11`` entry point. What it offers:

- the fixture ``netscenario``: a :class:`pytest_netscenario.runner.Runner` for the test,
- the marker ``@pytest.mark.netscenario("file.yaml", seed=..., settle=...)`` and the fixture
  ``netscenario_result``, which runs the scenario of the marker and gives the result,
- the option ``--netscenario-environment`` and the ini option ``netscenario_scenarios``,
- an attachment to the report of every test that ran a scenario.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Generator
from pathlib import Path

import pytest

import netscenario as ns
from pytest_netscenario import runner as runner_module
from pytest_netscenario.runner import Runner, describe, properties

_RESULTS: pytest.StashKey[list[ns.Result]] = pytest.StashKey()
"""The results of the runs of a test, kept on the test item until its report is made."""

_DOCTOR: pytest.StashKey[ns.DoctorReport] = pytest.StashKey()
"""What ``doctor`` said about this machine, asked once per session."""

_MODES = ("auto", "skip", "fail")


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
        "netscenario(scenario, *, seed=None, settle=None, packets=False): run this scenario file "
        "for the test; the fixture netscenario_result gives the result",
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

    def record(result: ns.Result) -> None:
        item.stash.setdefault(_RESULTS, []).append(result)
        # On the item, as `record_property` does: the reports of the test copy them from there, and
        # the JUnit file takes them from the last of them
        item.user_properties.extend(properties(result))

    return Runner(
        out_root=tmp_path / "netscenario",
        search=search,
        guard=_guard(pytestconfig),
        record=record,
    )


@pytest.fixture
def netscenario_result(request: pytest.FixtureRequest, netscenario: Runner) -> ns.Result:
    """The result of the scenario that the marker ``netscenario`` names.

    Example:
        >>> @pytest.mark.netscenario("lte-like.yaml", seed=42)  # doctest: +SKIP
        ... def test_media(netscenario_result):
        ...     assert netscenario_result["media"].loss_ratio < 0.02
    """
    marker = request.node.get_closest_marker("netscenario")
    if marker is None:
        pytest.fail(
            "the fixture netscenario_result needs the marker: "
            '@pytest.mark.netscenario("scenario.yaml")',
            pytrace=False,
        )
    if len(marker.args) != 1:
        pytest.fail(
            "the marker netscenario takes the scenario file as its one argument: "
            '@pytest.mark.netscenario("scenario.yaml", seed=42)',
            pytrace=False,
        )
    unknown = set(marker.kwargs) - {"seed", "settle", "packets"}
    if unknown:
        pytest.fail(
            f"the marker netscenario does not know {', '.join(sorted(unknown))}; "
            "it takes seed, settle and packets",
            pytrace=False,
        )
    return netscenario.run(
        marker.args[0],
        seed=marker.kwargs.get("seed"),
        settle=marker.kwargs.get("settle"),
        packets=bool(marker.kwargs.get("packets", False)),
    )


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
            report.sections.append(("netscenario", describe(result)))
    return report
