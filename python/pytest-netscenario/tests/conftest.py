"""Fixtures of the tests of the plugin."""

from __future__ import annotations

import os

import pytest

import netscenario as ns


@pytest.fixture(scope="session")
def runnable() -> None:
    """Stop a test that runs a scenario where this machine cannot make network namespaces.

    Locally the test is skipped, with the reason from ``doctor``. On the CI (``CI`` is set) it
    fails.
    """
    report = ns.doctor()
    if report.ok:
        return
    reason = f"this machine cannot run scenarios:\n{report.explain()}"
    if os.environ.get("CI"):
        pytest.fail(reason)
    pytest.skip(reason)
