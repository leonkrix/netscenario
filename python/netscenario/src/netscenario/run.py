"""Running a scenario, and asking what this machine can do."""

from __future__ import annotations

import json
import os
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from netscenario import _binaries, _native
from netscenario.errors import BackendError
from netscenario.events import RunEvent, make_event
from netscenario.results import Result
from netscenario.scenario import Scenario

CancelToken = _native.CancelToken
"""A request to stop a run.

Pass one to :func:`run` and call ``cancel()`` from any thread: the run tears the network down and
raises :class:`netscenario.Cancelled`.
"""


def run(
    scenario: Scenario | str | os.PathLike[str],
    *,
    out: str | os.PathLike[str] | None = None,
    seed: int | None = None,
    settle: float | None = None,
    packets: bool = False,
    observer: Callable[[RunEvent], None] | None = None,
    cancel: _native.CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | os.PathLike[str] | None = None,
) -> Result:
    """Run a scenario in network namespaces and return the result.

    The network is built, the agents start in the nodes, the workloads run for the duration of the
    scenario and everything is removed again, also after an error. Nothing outside the namespaces
    that it creates is touched. The call blocks, gives up the GIL, and can be interrupted: Ctrl-C
    (``KeyboardInterrupt``) and ``cancel`` stop the run, which tears the network down first.

    Args:
        scenario: A :class:`Scenario`, or the path of a scenario file.
        out: The directory for the result and the files that belong to it. Default: a new directory
            in the temporary directory of the system (``result.out_dir`` says which).
        seed: Use this seed and not the one in the scenario. This is how a run that drew its own
            seed is repeated.
        settle: How long to wait after the last packet for the packets still in the network, in
            seconds. Default: until the queues of the links are empty and 0.3 s more.
        packets: Also write one line of JSON for each packet of each workload
            (``Workload.packets_file``).
        observer: Called with a :class:`netscenario.events.RunEvent` for each event of the run, on
            the thread that runs, between two steps. An exception in it stops the run and is raised.
        cancel: A :class:`CancelToken` that stops the run.
        bin: The width of a bin of the series of each workload (``Workload.series``), in seconds.
            Default 1 s. At least a millisecond, and the measured phase may have at most a million
            bins. It is also the resolution of the time to recover.
        recovery_basis: The view the level of a bin is made from for ``Workload.recoveries``:
            ``"send"`` (default), the packets sent in the bin that arrived, or ``"arrival"``, the
            packets that arrived in the bin over those sent in it. ``"arrival"`` sees the silence of
            an ``outage`` at the receiver at the moment it happens.
        recovery_baseline: How many bins before the disturbance make the baseline. Default 5.
        recovery_window: How many bins in a row have to be back at the level. Default 3.
        recovery_fraction: The share of the baseline that has to be reached again, above 0 and at
            most 1. Default 0.9.
        working_dir: The directory the programs of your own (``kind: command``) run in, and that a
            relative ``ready: { file: ... }`` is counted from. Default: the directory of the
            scenario file, or the current directory for a scenario that was not read from a file.

    Returns:
        The :class:`Result`.

    Raises:
        ScenarioError: The scenario is not valid.
        CapabilityError: The backend cannot reproduce what the scenario asks for exactly; nothing
            was created.
        BackendError: The machine does not allow the run (see :func:`doctor`).
        RunError: The run failed after it started.
        Cancelled: ``cancel`` was cancelled.
        KeyboardInterrupt: The run was interrupted.
        ValueError: An option of the series or the recovery makes no sense.
    """
    loaded = scenario if isinstance(scenario, Scenario) else Scenario.load(scenario)
    try:
        supervisor, agent = _binaries.find()
    except _binaries.ProgramsNotFoundError as error:
        raise BackendError(str(error)) from None
    directory = Path(out) if out is not None else Path(tempfile.mkdtemp(prefix="netscenario-"))

    def notify(kind: str, fields: dict[str, float]) -> None:
        event = make_event(kind, fields)
        if event is not None and observer is not None:
            observer(event)

    text = _native.run_scenario(
        loaded.source,
        loaded.source_name,
        str(supervisor),
        str(agent),
        str(directory),
        seed=seed,
        settle=settle,
        packets=packets,
        observer=notify if observer is not None else None,
        cancel=cancel,
        bin=bin,
        recovery_basis=recovery_basis,
        recovery_baseline=recovery_baseline,
        recovery_window=recovery_window,
        recovery_fraction=recovery_fraction,
        working_dir=str(_working_dir(loaded, working_dir)),
    )
    return Result.from_json(text, directory)


def _working_dir(scenario: Scenario, given: str | os.PathLike[str] | None) -> Path:
    """The directory of the programs: the one asked for, the scenario file's, or this one."""
    if given is not None:
        return Path(given).resolve()
    source = Path(scenario.source_name)
    return source.resolve().parent if source.is_file() else Path.cwd()


@dataclass(frozen=True)
class Check:
    """One line of the report of :func:`doctor`."""

    section: str
    """The group: ``tools``, ``rights``, ``kernel`` or ``features``."""
    id: str
    """A stable name, for example ``tool-tc`` or ``seed``."""
    status: str
    """``ok``, ``warn``, ``fail`` or ``info``."""
    title: str
    """A short name."""
    detail: str
    """What was found."""
    fix: str | None
    """What to do about it, when something is not as it should be."""


@dataclass(frozen=True)
class DoctorReport:
    """What this machine can do for the Kernel backend (``docs/DOCTOR.md``)."""

    ok: bool
    """Whether scenarios can run here: no check failed."""
    checks: tuple[Check, ...]
    """Every check, in the order they are shown."""

    @property
    def problems(self) -> tuple[Check, ...]:
        """The checks that failed."""
        return tuple(check for check in self.checks if check.status == "fail")

    def explain(self) -> str:
        """The failed checks and how to fix them, as text for a message."""
        lines: list[str] = []
        for check in self.problems:
            lines.append(f"{check.title}: {check.detail}")
            if check.fix:
                lines.append(f"  fix: {check.fix}")
        return "\n".join(lines)


def doctor() -> DoctorReport:
    """Look at this machine: the tools, the rights to create namespaces, what the kernel accepts.

    It starts a few processes and takes a moment; nothing outside its own namespaces is touched.
    """
    raw = json.loads(_native.doctor())
    return DoctorReport(
        ok=bool(raw["ok"]),
        checks=tuple(
            Check(
                section=c["section"],
                id=c["id"],
                status=c["status"],
                title=c["title"],
                detail=c["detail"],
                fix=c["fix"],
            )
            for c in raw["checks"]
        ),
    )
