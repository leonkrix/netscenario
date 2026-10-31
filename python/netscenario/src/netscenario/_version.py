"""The version of the package as Python spells it.

The Rust library and the programs know the version as Cargo writes it (``0.1.0-rc.1``); PyPI and
``pip`` know it as PEP 440 does (``0.1.0rc1``). ``netscenario.__version__`` is the Python spelling,
so it is the same as ``importlib.metadata.version("netscenario")``. The release script has the same
rule (``python/release/version.py``), and a test compares the two.
"""

import re

_SEMVER = re.compile(r"(?P<core>\d+\.\d+\.\d+)(?:-(?P<kind>alpha|beta|rc)\.(?P<number>\d+))?")
_KIND = {"alpha": "a", "beta": "b", "rc": "rc"}


def pep440(version: str) -> str:
    """Write a version as Cargo writes it in the spelling of PEP 440.

    Args:
        version: A version such as ``0.1.0`` or ``0.1.0-rc.1``.

    Returns:
        ``0.1.0`` or ``0.1.0rc1``.

    Raises:
        ValueError: The text is not a version this project uses.
    """
    match = _SEMVER.fullmatch(version)
    if match is None:
        message = f"not a version (X.Y.Z, or X.Y.Z-rc.N, -beta.N, -alpha.N): {version!r}"
        raise ValueError(message)
    if match["kind"] is None:
        return match["core"]
    return f"{match['core']}{_KIND[match['kind']]}{match['number']}"
