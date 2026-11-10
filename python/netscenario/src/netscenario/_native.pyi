"""Types of the native module (crates/netscenario-py)."""

from collections.abc import Callable

def version() -> str:
    """The version of the native library."""

def parse_scenario(
    text: str,
    source_name: str,
    base_dir: str | None = None,
    allow_outside_files: bool = False,
    params: list[tuple[str, str]] | None = None,
) -> str:
    """Read and check a scenario; return it as canonical JSON. Raises ``ScenarioError``.

    The files the scenario refers to or includes are read from ``base_dir``; what is made of the
    files it refers to becomes part of the result. ``params`` are the values of its parameters.
    """

def needs(text: str, source_name: str) -> list[str]:
    """The ids of the features that a scenario needs from a backend."""

def schema() -> str:
    """The JSON Schema of the scenario format."""

def doctor() -> str:
    """The JSON of ``netscenario doctor --json``."""

class CancelToken:
    """A request to stop a run; clones of the Python object share nothing, pass the same object."""

    def __init__(self) -> None:
        """A token that has not been cancelled."""
    def cancel(self) -> None:
        """Ask the run that holds this token to stop. Any thread, any number of times."""
    @property
    def is_cancelled(self) -> bool:
        """Whether ``cancel`` was called."""

def run_scenario(
    source: str,
    canonical: str,
    source_name: str,
    supervisor: str,
    agent: str,
    out_dir: str,
    seed: int | None = None,
    settle: float | None = None,
    observer: Callable[[str, dict[str, float]], None] | None = None,
    cancel: CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | None = None,
    capture: list[str] | None = None,
    packets: bool | None = None,
    repetitions: int | None = None,
    statistic: str | None = None,
    confidence: float | None = None,
    reading: tuple[str | None, bool, list[tuple[str, str]]] | None = None,
) -> str:
    """Run a scenario; return ``result.json``, or with ``repetitions`` the ``runset.json`` text."""

def replay_run(
    dir: str,
    supervisor: str,
    agent: str,
    out_dir: str,
    observer: Callable[[str, dict[str, float]], None] | None = None,
    cancel: CancelToken | None = None,
    working_dir: str | None = None,
) -> str:
    """Run a recorded run again; return the JSON of ``netscenario replay --json``."""

def make_report(dir: str, format: str) -> str:
    """The report of a result directory as text (``md``, ``junit`` or ``html``)."""

def result_properties(text: str) -> list[tuple[str, str]]:
    """The JUnit properties of a ``result.json``, as the report writes them."""

def runset_properties(text: str) -> list[tuple[str, str]]:
    """The JUnit properties of a ``runset.json``, as the report writes them."""

class LiveRun:
    """A run that is going and can be changed (``netscenario.start``)."""

    def change(self, link: str, action: str, change: str | None = None) -> str:
        """Change a link: ``action`` is ``"set"`` (the JSON of a ``set``), ``"down"`` or ``"up"``.

        Returns the JSON list of the records of the steps it made, once the backend has done them.
        """
    def elapsed(self) -> float | None:
        """Seconds since the workloads started, or ``None`` before."""
    def is_done(self) -> bool:
        """Whether the run has ended."""
    def wait_done(self, timeout: float) -> bool:
        """Wait at most ``timeout`` seconds for the run to end; whether it has."""
    def cancel(self) -> None:
        """Ask the run to stop."""
    def finish(self) -> str:
        """Wait for the run and return ``result.json``, or raise. Once."""

def start_scenario(
    source: str,
    canonical: str,
    source_name: str,
    supervisor: str,
    agent: str,
    out_dir: str,
    live: list[str],
    seed: int | None = None,
    settle: float | None = None,
    observer: Callable[[str, dict[str, float]], None] | None = None,
    cancel: CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | None = None,
    capture: list[str] | None = None,
    packets: bool | None = None,
    reading: tuple[str | None, bool, list[tuple[str, str]]] | None = None,
) -> LiveRun:
    """Start a run on its own thread; ``live`` names the links the code may change."""
