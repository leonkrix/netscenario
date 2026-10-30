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
            seconds. Default: one second and twice the longest delay of the scenario.
        packets: Also write one line of JSON for each packet of each workload
            (``Workload.packets_file``).
        observer: Called with a :class:`netscenario.events.RunEvent` for each event of the run, on
            the thread that runs, between two steps. An exception in it stops the run and is raised.
        cancel: A :class:`CancelToken` that stops the run.

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
    )
    return Result.from_json(text, directory)


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
