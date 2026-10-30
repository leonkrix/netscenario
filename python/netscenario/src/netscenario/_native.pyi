"""Types of the native module (crates/netscenario-py)."""

from collections.abc import Callable

def version() -> str:
    """The version of the native library."""

def parse_scenario(text: str, source_name: str) -> str:
    """Read and check a scenario; return it as canonical JSON. Raises ``ScenarioError``."""

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
    source_name: str,
    supervisor: str,
    agent: str,
    out_dir: str,
    seed: int | None = None,
    settle: float | None = None,
    packets: bool = False,
    observer: Callable[[str, dict[str, float]], None] | None = None,
    cancel: CancelToken | None = None,
) -> str:
    """Run a scenario; return the text of ``result.json``."""
