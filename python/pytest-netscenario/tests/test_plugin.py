import importlib.metadata

import pytest


def test_the_fixture_is_found_through_the_entry_point(pytester: pytest.Pytester) -> None:
    pytester.makepyfile(
        """
        def test_it(netscenario):
            assert netscenario.__version__
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


def test_canary_a_failing_test_is_reported_as_failed(pytester: pytest.Pytester) -> None:
    # The check above must be able to fail
    pytester.makepyfile(
        """
        def test_it(netscenario):
            assert netscenario.__version__ == "not a version"
        """
    )
    pytester.runpytest().assert_outcomes(failed=1)


def test_plugin_and_bindings_have_the_same_version() -> None:
    # They are released together (D-004)
    assert importlib.metadata.version("pytest-netscenario") == importlib.metadata.version(
        "netscenario"
    )
