"""The Python layer: typed wrappers over the native module."""

from ._native import __rust_version__, busy_wait_ms, parse_duration_ns

__all__ = ["__rust_version__", "busy_wait_ms", "parse_duration_ns", "seconds"]


def seconds(text: str) -> float:
    """A duration as seconds, parsed by the Rust code."""
    return parse_duration_ns(text) / 1e9
