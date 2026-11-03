"""Reports from a result directory: Markdown, JUnit and HTML (CLI-02, RPT-02, RPT-03, RPT-04).

``report`` is ``netscenario report`` as a function: it reads the files of a directory that
``netscenario.run`` or ``netscenario run`` wrote (a run, or a set of runs made with ``repetitions``)
and returns the report as text. The formats and what they contain are in ``docs/REPORTS.md``.

Example:
    >>> result = ns.run("lte-like.yaml")  # doctest: +SKIP
    >>> summary = ns.report(result.out_dir, "md")  # doctest: +SKIP
    >>> Path("report.html").write_text(ns.report(result.out_dir, "html"))  # doctest: +SKIP
"""

from __future__ import annotations

import json
import os
from typing import TYPE_CHECKING, Literal

from netscenario import _native

if TYPE_CHECKING:
    from netscenario.results import Result
    from netscenario.runset import RunSet

ReportFormat = Literal["md", "junit", "html"]
"""The formats of a report: Markdown, JUnit XML and one HTML file."""


def report(directory: str | os.PathLike[str], format: ReportFormat = "md") -> str:
    """Make the report of a result directory.

    The report is made from the files of the directory and nothing else, so the same directory gives
    the same text. ``"md"`` is the summary for a CI job, ``"junit"`` has one test case for each
    workload with the metrics as properties, ``"html"`` is one file with inline charts and nothing
    to fetch. The HTML of a run needs the series and the records of the run in the directory.

    Args:
        directory: The directory of a run (``Result.out_dir``) or of a set (``RunSet.out_dir``).
        format: ``"md"`` (default), ``"junit"`` or ``"html"``.

    Returns:
        The report.

    Raises:
        ValueError: The directory is not a result directory, a file of it cannot be read, or the
            format is not one of the three.
    """
    return _native.make_report(os.fspath(directory), format)


def junit_properties(result: Result | RunSet) -> tuple[tuple[str, str], ...]:
    """The properties that a result gives to its test cases in a JUnit report, as ``(name, value)``.

    The names are ``netscenario.<scenario>.<workload>.<metric>`` (times in seconds), and for a set
    ``netscenario.<scenario>.<metric>.mean``, ``.low`` and ``.high``. The list is the one that the
    report and the pytest plugin use: it is made by the native library, so the names cannot differ.
    """
    text = json.dumps(dict(result.raw))
    if "workloads" in result.raw:
        return tuple(_native.result_properties(text))
    return tuple(_native.runset_properties(text))
