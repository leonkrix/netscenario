"""netscenario: declarative network scenarios for tests.

Describe a network (nodes, links, impairments, changes over time), run a program in it, measure per
packet and assert with statistics. The semantics live in the Rust core; this package is its typed
Python interface. Nothing but the version exists yet; see ``FEATURES.md`` in the repository for the
plan.
"""

from netscenario import _native

__version__: str = _native.version()
"""The version of the package, the same as that of the native library."""

__all__ = ["__version__"]
