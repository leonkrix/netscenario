"""The command ``netscenario`` and ``python -m netscenario``: they start the program of the wheel.

The command line is a Rust program (``docs/CLI.md``) that the wheel carries next to the agent.
The launcher replaces this Python process with it, so that signals, exit codes and the terminal are
those of the program and nothing of Python stays in between.
"""

from __future__ import annotations

import os
import sys
from typing import NoReturn

from netscenario import _binaries

_EXIT_ENVIRONMENT = 3
"""The exit code of the command line for a machine that does not allow the run."""


def main() -> NoReturn:
    """Start ``netscenario`` with the arguments of this process, which becomes that program."""
    try:
        supervisor, _agent = _binaries.find()
    except _binaries.ProgramsNotFoundError as error:
        sys.stderr.write(f"netscenario: {error}\n")
        sys.exit(_EXIT_ENVIRONMENT)
    os.execv(supervisor, [str(supervisor), *sys.argv[1:]])  # noqa: S606 - own program, no shell


if __name__ == "__main__":
    main()
