"""Fixtures of the tests of the package."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

import netscenario as ns

REPOSITORY = Path(__file__).resolve().parents[3]
"""The root of the repository, where ``scenarios/`` is."""


@pytest.fixture(scope="session")
def repository() -> Path:
    """The root of the repository."""
    return REPOSITORY


@pytest.fixture(scope="session")
def runnable() -> None:
    """Stop a test that needs network namespaces where this machine cannot make them.

    Locally the test is skipped, with the reason from ``doctor``. On the CI (``CI`` is set) it
    fails, because
    a test that never runs there proves nothing.
    """
    report = ns.doctor()
    if report.ok:
        return
    reason = f"this machine cannot run scenarios:\n{report.explain()}"
    if os.environ.get("CI"):
        pytest.fail(reason)
    pytest.skip(reason)
