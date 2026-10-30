"""The exceptions of netscenario.

Every error that the library raises is a :class:`NetscenarioError`. They correspond to the exit
codes of the command line (``docs/CLI.md``): a wrong scenario or one that the backend cannot
reproduce exactly is 2 (:class:`ScenarioError`, :class:`CapabilityError`), a machine that does not
allow the run is 3 (:class:`BackendError`), a run that failed after it started is 4
(:class:`RunError`). A run that was stopped on request raises :class:`Cancelled`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import cast


class NetscenarioError(Exception):
    """The base of every error that netscenario raises. ``str(error)`` is the full message."""


@dataclass(frozen=True)
class ScenarioProblem:
    """One thing that is wrong with a scenario."""

    message: str
    """What is wrong: what was found and what was expected."""
    path: str | None
    """The path of the value in the scenario (``topology.links[0].between[1]``), if the problem is
    about a value."""
    line: int | None
    """The line in the file (counted from 1), if the scenario was read from a file."""
    column: int | None
    """The column in the line (counted from 1), if the scenario was read from a file."""
    help: str | None
    """How to fix it, if there is something useful to say."""


class ScenarioError(NetscenarioError):
    """A scenario is not valid. All of its problems are listed, not only the first."""

    problems: tuple[ScenarioProblem, ...]
    """The problems, in the order of the file."""

    def __init__(self, message: str, problems: tuple[ScenarioProblem, ...]) -> None:
        """Make the error from the rendered message and the problems."""
        super().__init__(message)
        self.problems = problems


@dataclass(frozen=True)
class Refusal:
    """A feature of a scenario that the backend does not reproduce exactly."""

    feature: str
    """The id of the feature (``seeded-loss``, ...); the rows of ``docs/COVERAGE.md``."""
    path: str
    """Where the scenario asks for it."""
    reason: str
    """Why the backend cannot do it."""
    fix: str
    """What to do about it."""


class CapabilityError(NetscenarioError):
    """The backend cannot reproduce what the scenario asks for exactly, so nothing was created.

    This is never an approximation that went through: a scenario is refused before anything is
    built, with the feature, the backend and a way out (``docs/COVERAGE.md``).
    """

    backend: str
    """The name of the backend that refused."""
    refusals: tuple[Refusal, ...]
    """Every refusal, not only the first."""

    def __init__(self, message: str, backend: str, refusals: tuple[Refusal, ...]) -> None:
        """Make the error from the rendered message, the backend and the refusals."""
        super().__init__(message)
        self.backend = backend
        self.refusals = refusals


class BackendError(NetscenarioError):
    """The machine does not allow the run: a tool or a permission is missing, or the backend failed.

    :func:`netscenario.doctor` says what this machine can do and how to fix what it cannot.
    """


class RunError(NetscenarioError):
    """A run failed after it started: an agent did not start, did not answer or ended badly.

    The network was torn down all the same.
    """


class Cancelled(NetscenarioError):  # noqa: N818 - it is not an error of the program but a request
    """The run was stopped by a :class:`netscenario.CancelToken`.

    The network was torn down; there is no result.
    """


def from_native(kind: str, message: str, details: object) -> NetscenarioError:
    """Make the exception that the native module asks for (called from Rust, not API)."""
    if kind == "scenario":
        problems = cast("list[tuple[str, str | None, int | None, int | None, str | None]]", details)
        return ScenarioError(message, tuple(ScenarioProblem(*problem) for problem in problems))
    if kind == "capability":
        backend, refusals = cast("tuple[str, list[tuple[str, str, str, str]]]", details)
        return CapabilityError(message, backend, tuple(Refusal(*refusal) for refusal in refusals))
    if kind == "backend":
        return BackendError(message)
    if kind == "cancelled":
        return Cancelled(message)
    return RunError(message)
