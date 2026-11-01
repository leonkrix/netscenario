"""The runner that a test gets: it finds scenario files, guards the environment and runs.

The runner is a thin layer over :func:`netscenario.run`. It adds three things that a test wants:
scenario files are found next to the test (or in the directories of the ini option), every run goes
into the temporary directory of the test, and a machine that cannot run scenarios is reported once,
as a skip or a failure, before anything is started.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Sequence
from pathlib import Path

import netscenario as ns
from netscenario import CancelToken, Result, Scenario
from netscenario.events import RunEvent

doctor = ns.doctor
"""What is asked about the machine; the tests of the plugin replace it."""


def find_scenario_file(name: str | os.PathLike[str], search: Sequence[Path]) -> Path:
    """Find a scenario file.

    An absolute path is taken as it is. A relative one is looked for in each of the directories of
    ``search``, in order.

    Raises:
        FileNotFoundError: There is no such file; the message lists the places that were looked in.
    """
    path = Path(name)
    if path.is_absolute():
        if path.is_file():
            return path
        message = f"the scenario file {path} does not exist"
        raise FileNotFoundError(message)
    for directory in search:
        candidate = directory / path
        if candidate.is_file():
            return candidate
    places = ", ".join(str(directory) for directory in search)
    message = f"the scenario file '{name}' was not found in: {places}"
    raise FileNotFoundError(message)


class Runner:
    """Runs scenarios for one test. Get it with the fixture ``netscenario``.

    Example:
        >>> def test_media(netscenario):  # doctest: +SKIP
        ...     result = netscenario.run("lte-like.yaml")
        ...     assert result["media"].delay.p99 < 0.150  # seconds
    """

    def __init__(
        self,
        *,
        out_root: Path,
        search: Sequence[Path],
        guard: Callable[[], None],
        record: Callable[[Result], None],
    ) -> None:
        """Make the runner of a test (the plugin does this; use the fixture)."""
        self._out_root = out_root
        self._search = list(search)
        self._guard = guard
        self._record = record
        self._results: list[Result] = []

    def load(self, scenario: str | os.PathLike[str]) -> Scenario:
        """Read a scenario file, looked for next to the test file and in the ini directories.

        This does not need a machine that can run scenarios: a test that only reads and checks a
        scenario works anywhere.

        Raises:
            FileNotFoundError: There is no such file.
            netscenario.ScenarioError: The scenario is not valid.
        """
        return Scenario.load(find_scenario_file(scenario, self._search))

    def run(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool = False,
        observer: Callable[[RunEvent], None] | None = None,
        cancel: CancelToken | None = None,
        out: str | os.PathLike[str] | None = None,
    ) -> Result:
        """Run a scenario and return its :class:`netscenario.Result`.

        The result goes into the report of the test: a section with the numbers is shown when the
        test fails, and the counts and the delays are properties of the test in a JUnit file.

        Args:
            scenario: A :class:`netscenario.Scenario`, or the name of a scenario file (see
                :meth:`load`).
            seed: Use this seed and not the one in the scenario.
            settle: How long to wait for the packets still in the network, in seconds.
            packets: Also write the JSON Lines file with a line for each packet.
            observer: Called with each :class:`netscenario.events.RunEvent`.
            cancel: A :class:`netscenario.CancelToken` that stops the run.
            out: The directory for the result. Default: a new one in the temporary directory of the
                test.

        Raises:
            Skipped: The machine cannot run scenarios (locally; on the CI that is a failure).
            netscenario.NetscenarioError: Any error of :func:`netscenario.run`.
        """
        loaded = scenario if isinstance(scenario, Scenario) else self.load(scenario)
        self._guard()
        directory = (
            Path(out)
            if out is not None
            else self._out_root / f"run-{len(self._results) + 1}-{loaded.name}"
        )
        result = ns.run(
            loaded,
            out=directory,
            seed=seed,
            settle=settle,
            packets=packets,
            observer=observer,
            cancel=cancel,
        )
        self._results.append(result)
        self._record(result)
        return result

    @property
    def results(self) -> tuple[Result, ...]:
        """The results of the runs of this test so far, in order."""
        return tuple(self._results)


def describe(result: Result) -> str:
    """The numbers of a run as text for the report of a test."""
    details = ", ".join(result.provenance.backend_details.values())
    seed = "no seed"
    if result.seed.value is not None:
        seed = f"seed {result.seed.value}" + (" (drawn)" if result.seed.drawn else "")
    lines = [f"{result.scenario_name}: {result.provenance.backend} ({details}), {seed}"]
    if not result.seed.loss_reproducible:
        reasons = result.seed.not_reproducible_because or ("no reason given",)
        lines.extend(f"  warning: the loss of this run cannot be repeated: {r}" for r in reasons)
    for workload in result:
        delay = "no packet arrived"
        if workload.delay is not None:
            delay = (
                f"delay p50 {workload.delay.p50 * 1000:.1f} ms, "
                f"p99 {workload.delay.p99 * 1000:.1f} ms"
            )
        lines.append(
            f"  {workload.id} {workload.from_node} -> {workload.to_node}: sent {workload.sent}, "
            f"received {workload.received}, lost {workload.loss_ratio:.2%}, "
            f"reordered {workload.reordered}, {delay}"
        )
    lines.append(f"  result: {result.out_dir / 'result.json'}")
    return "\n".join(lines)


def properties(result: Result) -> list[tuple[str, object]]:
    """The numbers of a run as properties of the test, for a JUnit file.

    The names are ``netscenario.<scenario>.<workload>.<metric>``; times are in seconds.
    """
    found: list[tuple[str, object]] = []
    for workload in result:
        prefix = f"netscenario.{result.scenario_name}.{workload.id}"
        found.extend(
            [
                (f"{prefix}.sent", workload.sent),
                (f"{prefix}.received", workload.received),
                (f"{prefix}.lost", workload.lost),
                (f"{prefix}.loss_ratio", workload.loss_ratio),
                (f"{prefix}.reordered", workload.reordered),
            ]
        )
        if workload.delay is not None:
            found.extend(
                [
                    (f"{prefix}.delay_p50_s", workload.delay.p50),
                    (f"{prefix}.delay_p99_s", workload.delay.p99),
                ]
            )
    return found
