"""The plugin: the fixtures that pytest finds through the ``pytest11`` entry point (PYT-02)."""

from types import ModuleType

import pytest

import netscenario as _netscenario


@pytest.fixture
def netscenario() -> ModuleType:
    """The ``netscenario`` package. The fixtures for scenarios and runs arrive with release 0.1."""
    return _netscenario
