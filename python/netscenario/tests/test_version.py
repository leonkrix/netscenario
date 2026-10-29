import importlib.metadata

import pytest

import netscenario


def test_version_comes_from_the_native_library_and_matches_the_package() -> None:
    assert netscenario.__version__ == importlib.metadata.version("netscenario")


def test_canary_a_wrong_version_is_detected() -> None:
    # The check above must be able to fail
    with pytest.raises(AssertionError):
        assert netscenario.__version__ == "not a version"
