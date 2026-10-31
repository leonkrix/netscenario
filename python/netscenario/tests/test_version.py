import importlib.metadata

import pytest

import netscenario
from netscenario import _native
from netscenario._version import pep440


def test_version_comes_from_the_native_library_and_matches_the_package() -> None:
    # Cargo and pip write one version differently; the package says it the way pip does
    assert netscenario.__version__ == pep440(_native.version())
    assert netscenario.__version__ == importlib.metadata.version("netscenario")


@pytest.mark.parametrize(
    ("cargo", "pip"),
    [
        ("0.1.0", "0.1.0"),
        ("0.1.0-rc.1", "0.1.0rc1"),
        ("1.20.3-beta.12", "1.20.3b12"),
        ("0.2.0-alpha.2", "0.2.0a2"),
    ],
)
def test_pep440_spells_a_version_as_pip_does(cargo: str, pip: str) -> None:
    assert pep440(cargo) == pip


@pytest.mark.parametrize("text", ["", "1.2", "v1.2.3", "1.2.3-rc", "1.2.3-dev.1", "0.1.0rc1"])
def test_canary_a_text_that_is_not_a_version_of_cargo_is_refused(text: str) -> None:
    with pytest.raises(ValueError, match="not a version"):
        pep440(text)


def test_canary_a_wrong_version_is_detected() -> None:
    # The check above must be able to fail
    with pytest.raises(AssertionError):
        assert netscenario.__version__ == "not a version"
