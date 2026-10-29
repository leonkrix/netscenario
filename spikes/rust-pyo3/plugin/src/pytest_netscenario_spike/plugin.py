"""The plugin: a fixture that hands the test a runner from the bindings."""

import pytest

import netscenario_spike


@pytest.fixture
def scenario_runner():
    return netscenario_spike
