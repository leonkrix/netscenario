"""Where the two programs of a run are.

``netscenario`` is also the supervisor of a run, and ``netscenario-agent`` runs in the nodes.
Python cannot be the supervisor itself: it has several threads and cannot move into a user
namespace (D-022). So a wheel carries the two programs, and the native module is told where they
are. The search order:

1. the directory in the environment variable ``NETSCENARIO_BIN_DIR``,
2. the directory ``_bin`` inside this package (where a wheel puts them),
3. in a checkout of the repository, ``target/release`` or ``target/debug``, whichever was built
   last, so that an editable install works after ``cargo build``.
"""

from __future__ import annotations

import os
from pathlib import Path

_SUPERVISOR = "netscenario"
_AGENT = "netscenario-agent"


class ProgramsNotFoundError(FileNotFoundError):
    """The two programs of a run are not where they are looked for."""


def _candidates() -> list[Path]:
    places: list[Path] = []
    from_environment = os.environ.get("NETSCENARIO_BIN_DIR")
    if from_environment:
        places.append(Path(from_environment))
    places.append(Path(__file__).resolve().parent / "_bin")
    # python/netscenario/src/netscenario/_binaries.py in a checkout: the repository is four levels
    # up
    parents = Path(__file__).resolve().parents
    if len(parents) > 4:
        repository = parents[4]
        built = [repository / "target" / profile for profile in ("release", "debug")]
        existing = [p for p in built if (p / _SUPERVISOR).is_file()]
        existing.sort(key=lambda p: (p / _SUPERVISOR).stat().st_mtime, reverse=True)
        places.extend(existing)
    return places


def find() -> tuple[Path, Path]:
    """The paths of ``netscenario`` and ``netscenario-agent``.

    Raises:
        ProgramsNotFoundError: They are not in any of the places that are looked in; the message
            lists them.
    """
    places = _candidates()
    for place in places:
        supervisor, agent = place / _SUPERVISOR, place / _AGENT
        if supervisor.is_file() and agent.is_file():
            return supervisor, agent
    searched = ", ".join(str(place) for place in places)
    message = (
        f"the programs '{_SUPERVISOR}' and '{_AGENT}' were not found in: {searched}. "
        "Install the wheel, or build them with `cargo build` and set NETSCENARIO_BIN_DIR to the "
        "directory."
    )
    raise ProgramsNotFoundError(message)
