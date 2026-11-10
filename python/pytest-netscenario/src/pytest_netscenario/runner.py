"""The runner that a test gets: it finds scenario files, guards the environment and runs.

The runner is a thin layer over :func:`netscenario.run`. It adds three things that a test wants:
scenario files are found next to the test (or in the directories of the ini option), every run goes
into the temporary directory of the test, and a machine that cannot run scenarios is reported once,
as a skip or a failure, before anything is started.
"""

from __future__ import annotations

import contextlib
import os
from collections.abc import Callable, Iterable, Mapping, Sequence
from pathlib import Path
from typing import overload

import netscenario as ns
from netscenario import AsyncRun, CancelToken, Result, Run, RunSet, Scenario
from netscenario.events import RunEvent
from netscenario.scenario import ParamValue

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
        record: Callable[[Result | RunSet], None],
    ) -> None:
        """Make the runner of a test (the plugin does this; use the fixture)."""
        self._out_root = out_root
        self._search = list(search)
        self._guard = guard
        self._record = record
        self._results: list[Result | RunSet] = []
        self._started: list[Run] = []

    def load(
        self,
        scenario: str | os.PathLike[str],
        *,
        params: Mapping[str, ParamValue] | None = None,
    ) -> Scenario:
        """Read a scenario file, looked for next to the test file and in the ini directories.

        This does not need a machine that can run scenarios: a test that only reads and checks a
        scenario works anywhere. ``params`` are the values of the parameters of the scenario
        (``params:`` in the file), by name.

        Raises:
            FileNotFoundError: There is no such file.
            netscenario.ScenarioError: The scenario is not valid.
        """
        return Scenario.load(find_scenario_file(scenario, self._search), params=params)

    @overload
    def run(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        repetitions: None = None,
        statistic: None = None,
        confidence: None = None,
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        cancel: CancelToken | None = None,
        bin: float | None = None,
        recovery_basis: str | None = None,
        recovery_baseline: int | None = None,
        recovery_window: int | None = None,
        recovery_fraction: float | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> Result: ...

    @overload
    def run(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        repetitions: int,
        statistic: str | None = None,
        confidence: float | None = None,
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        cancel: CancelToken | None = None,
        bin: float | None = None,
        recovery_basis: str | None = None,
        recovery_baseline: int | None = None,
        recovery_window: int | None = None,
        recovery_fraction: float | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> RunSet: ...

    def run(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        repetitions: int | None = None,
        statistic: str | None = None,
        confidence: float | None = None,
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        cancel: CancelToken | None = None,
        bin: float | None = None,
        recovery_basis: str | None = None,
        recovery_baseline: int | None = None,
        recovery_window: int | None = None,
        recovery_fraction: float | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> Result | RunSet:
        """Run a scenario and return its :class:`netscenario.Result`.

        With ``repetitions`` the scenario is run that many times with derived seeds and the call
        returns a :class:`netscenario.RunSet` with an interval for each metric
        (``docs/STATISTICS.md``).

        The result goes into the report of the test: a section with the numbers is shown when the
        test fails, and the counts and the delays are properties of the test in a JUnit file. For a
        set the section has the headline metrics with their intervals, and the properties are the
        mean and the ends of the interval of those metrics.

        Args:
            scenario: A :class:`netscenario.Scenario`, or the name of a scenario file (see
                :meth:`load`).
            repetitions: Run the scenario this many times, from 5 to 1000.
            statistic: With ``repetitions``: ``"mean"`` (default) or ``"median"``.
            confidence: With ``repetitions``: the level of the intervals, above 0 and below 1
                (default 0.95).
            seed: Use this seed and not the one in the scenario (for a set: the seed of the set).
            settle: How long to wait for the packets still in the network, in seconds.
            packets: ``True``: also write the JSON Lines file with a line for each packet.
            capture: The links to capture as pcap files, at both ends.
            observer: Called with each :class:`netscenario.events.RunEvent`.
            cancel: A :class:`netscenario.CancelToken` that stops the run.
            out: The directory for the result (for a set: of the set). Default: a new one in the
                temporary directory of the test.
            bin: The width of a bin of the series, in seconds (see :func:`netscenario.run`).
            recovery_basis: ``"send"`` or ``"arrival"``: the view of the time to recover.
            recovery_baseline: How many bins make the baseline of the recovery.
            recovery_window: How many bins in a row have to be back at the level.
            recovery_fraction: The share of the baseline to reach again, above 0 to 1.
            params: The values of the parameters of the scenario, by name (``{"rtt": "40ms"}``);
                the others keep their defaults. The result records them.

        Raises:
            Skipped: The machine cannot run scenarios (locally; on the CI that is a failure).
            netscenario.NetscenarioError: Any error of :func:`netscenario.run`.
        """
        if repetitions is None and (statistic is not None or confidence is not None):
            message = "statistic and confidence need repetitions"
            raise ValueError(message)
        loaded = self._scenario(scenario, params)
        self._guard()
        directory = self._directory(loaded, out)
        result: Result | RunSet
        if repetitions is None:
            result = ns.run(
                loaded,
                out=directory,
                seed=seed,
                settle=settle,
                packets=packets,
                capture=capture,
                observer=observer,
                cancel=cancel,
                bin=bin,
                recovery_basis=recovery_basis,
                recovery_baseline=recovery_baseline,
                recovery_window=recovery_window,
                recovery_fraction=recovery_fraction,
            )
        else:
            result = ns.run(
                loaded,
                repetitions=repetitions,
                statistic=statistic,
                confidence=confidence,
                out=directory,
                seed=seed,
                settle=settle,
                packets=packets,
                capture=capture,
                observer=observer,
                cancel=cancel,
                bin=bin,
                recovery_basis=recovery_basis,
                recovery_baseline=recovery_baseline,
                recovery_window=recovery_window,
                recovery_fraction=recovery_fraction,
            )
        self._results.append(result)
        self._record(result)
        return result

    def _scenario(
        self,
        scenario: Scenario | str | os.PathLike[str],
        params: Mapping[str, ParamValue] | None,
    ) -> Scenario:
        """The scenario to run; a file is looked for as :meth:`load` does."""
        if isinstance(scenario, Scenario):
            return scenario.with_params(**params) if params else scenario
        return self.load(scenario, params=params)

    def _directory(self, scenario: Scenario, out: str | os.PathLike[str] | None) -> Path:
        return (
            Path(out)
            if out is not None
            else self._out_root
            / f"run-{len(self._results) + len(self._started) + 1}-{scenario.name}"
        )

    def _kept(self, result: Result) -> None:
        self._results.append(result)
        self._record(result)

    def start(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        live: Iterable[str] = (),
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        cancel: CancelToken | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> Run:
        """Start a run that the test can talk to while it goes: :func:`netscenario.start`.

        ``live`` names the links the test may change (``run.link("access").set(...)``). The result
        goes into the report of the test when the test asks for it (``run.result()``, or leaving the
        ``with`` block). A run that is still going when the test ends is cancelled and torn down.
        """
        loaded = self._scenario(scenario, params)
        self._guard()
        run = ns.start(
            loaded,
            live=live,
            out=self._directory(loaded, out),
            seed=seed,
            settle=settle,
            packets=packets,
            capture=capture,
            observer=observer,
            cancel=cancel,
            on_result=self._kept,
        )
        self._started.append(run)
        return run

    async def run_async(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> Result:
        """Run a scenario from an ``async`` test: :func:`netscenario.run_async`."""
        async with await self.start_async(
            scenario,
            out=out,
            seed=seed,
            settle=settle,
            packets=packets,
            capture=capture,
            observer=observer,
            params=params,
        ) as run:
            return await run.result()

    async def start_async(
        self,
        scenario: Scenario | str | os.PathLike[str],
        *,
        live: Iterable[str] = (),
        out: str | os.PathLike[str] | None = None,
        seed: int | None = None,
        settle: float | None = None,
        packets: bool | None = None,
        capture: Sequence[str] | None = None,
        observer: Callable[[RunEvent], None] | None = None,
        params: Mapping[str, ParamValue] | None = None,
    ) -> AsyncRun:
        """Start a run for an ``async`` test: :func:`netscenario.start_async`."""
        loaded = self._scenario(scenario, params)
        self._guard()
        run = await ns.start_async(
            loaded,
            live=live,
            out=self._directory(loaded, out),
            seed=seed,
            settle=settle,
            packets=packets,
            capture=capture,
            observer=observer,
            on_result=self._kept,
        )
        self._started.append(run.run)
        return run

    def close(self) -> None:
        """End what the test left going: a run not waited for is cancelled and torn down."""
        for run in self._started:
            if not run.done:
                run.cancel()
            with contextlib.suppress(ns.NetscenarioError):
                run.result()

    @property
    def results(self) -> tuple[Result | RunSet, ...]:
        """The results of the runs of this test so far, in order (a set is one entry)."""
        return tuple(self._results)


_HEADLINE = ("loss_ratio", "delay.p50", "delay.p95", "delay.p99", "throughput.mean")
"""The metrics of a workload that the report and the JUnit file of a set show."""


def _show(name: str, value: float) -> str:
    """A value of a metric as text, in the unit that reads best."""
    if name.endswith("loss_ratio"):
        return f"{value:.2%}"
    if ".delay." in name:
        return f"{value * 1000:.3f} ms"
    if name.endswith("throughput.mean"):
        return f"{value / 1e6:.3f} mbit/s"
    return f"{value:g}"


def describe_set(runs: RunSet) -> str:
    """The numbers of a set of runs as text for the report of a test."""
    seed = f"seed {runs.seed.value}" + (" (drawn)" if runs.seed.drawn else "")
    confidence = runs.confidence
    lines = [
        f"{runs.scenario_name}: {runs.repetitions} repetitions, {seed}; interval of the "
        f"{confidence.statistic}, {confidence.level:.0%}, {confidence.method}"
    ]
    unreproducible = sum(1 for run in runs.runs if not run.loss_reproducible)
    if unreproducible:
        lines.append(
            f"  warning: the loss of {unreproducible} of the runs cannot be repeated from its seed"
        )
    for metric in runs:
        if not metric.name.endswith(_HEADLINE):
            continue
        interval = metric.interval
        lines.append(
            f"  {metric.name}: {_show(metric.name, interval.estimate)} "
            f"[{_show(metric.name, interval.low)} to {_show(metric.name, interval.high)}], "
            f"spread {_show(metric.name, metric.stddev)}"
        )
    if runs.partial:
        names = ", ".join(p.name for p in runs.partial[:3])
        lines.append(
            f"  {len(runs.partial)} metrics are missing in some runs, no interval: {names}"
        )
    if runs.expect is not None:
        lines.extend(_expect_lines(runs.expect))
    lines.append(f"  set: {runs.out_dir / 'runset.json'}")
    return "\n".join(lines)


def _expect_lines(expect: ns.Expectations) -> list[str]:
    """The checks of the scenario file: how many hold, and a line for each that does not."""
    held = len(expect.checks) - len(expect.failures)
    lines = [f"  checks of the file: {held} of {len(expect.checks)} hold"]
    lines.extend(f"    FAIL {check}" for check in expect.failures)
    return lines


def set_properties(runs: RunSet) -> list[tuple[str, object]]:
    """The headline numbers of a set as properties of the test, for a JUnit file.

    The names are ``netscenario.<scenario>.<metric>.<mean|low|high>`` for the headline metrics, and
    ``netscenario.<scenario>.repetitions``, ``.confidence`` and ``.seed``. Times are in seconds. The
    list is the one of ``netscenario report --format junit``: it comes from the native library.
    """
    return list(ns.junit_properties(runs))


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
    lines.extend(_timeline_lines(result))
    lines.extend(_program_lines(result))
    if result.expect is not None:
        lines.extend(_expect_lines(result.expect))
    lines.append(f"  result: {result.out_dir / 'result.json'}")
    return "\n".join(lines)


def _timeline_lines(result: Result) -> list[str]:
    """What the run did to the links, and how long each workload took to recover."""
    lines: list[str] = []
    if result.steps:
        lines.append("  timeline:")
    for step in result.steps:
        where = step.direction or "both ways"
        how = (
            f"before packet {step.packet}"
            if step.packet is not None
            else f"{step.lateness * 1000:.1f} ms late"
        )
        lines.append(
            f"    event {step.event} at {step.scheduled:g} s: link {step.link} {where} {step.kind}"
            f" ({how})"
        )
    for workload in result:
        for recovery in workload.recoveries:
            time = (
                f"recovered in {recovery.time:g} s"
                if recovery.time is not None
                else f"not recovered ({recovery.reason})"
            )
            lines.append(
                f"    {workload.id} after event {recovery.event} ({recovery.kind}): {time}"
            )
    return lines


def _program_lines(result: Result) -> list[str]:
    """How each program of your own ended, and the last value of its metrics."""
    lines: list[str] = []
    for program in result.programs.values():
        code = "" if program.exit_code is None else f", exit {program.exit_code}"
        last = ", ".join(f"{name} {metric.last:g}" for name, metric in program.metrics.items())
        lines.append(
            f"  program {program.id}: {program.how_it_ended}{code}" + (f"; {last}" if last else "")
        )
    return lines


def properties(result: Result) -> list[tuple[str, object]]:
    """The numbers of a run as properties of the test, for a JUnit file.

    The names are ``netscenario.<scenario>.<workload>.<metric>`` (``sent``, ``received``,
    ``lost``, ``loss_ratio``, ``reordered``, ``duplicates``, ``delay_min_s``, ``delay_p50_s``,
    ``delay_p95_s``, ``delay_p99_s``, ``delay_max_s``, ``jitter_mean_s``,
    ``throughput_mean_bps`` and ``loss_runs_longest``, times in seconds) and
    ``netscenario.<scenario>.<program>.<metric>`` for the metrics of a program of your own. The
    list is the one of ``netscenario report --format junit``: it comes from the native library, so
    the names cannot differ.
    """
    return list(ns.junit_properties(result))
