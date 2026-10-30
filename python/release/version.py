"""Keep the version of netscenario in one place, and check that every file agrees with it.

The crates, the bindings and the plugin are released together (D-004), so there is one
version. It is written in ``Cargo.toml`` (``workspace.package.version``); the Python package
``netscenario`` takes it from there (maturin), and this script writes the other places: the
internal dependencies of the workspace, the version of ``pytest-netscenario`` and its exact pin
on ``netscenario``.

Usage, from the root of the repository::

    python3 python/release/version.py show [--python]
    python3 python/release/version.py check [--tag vX.Y.Z]
    python3 python/release/version.py set X.Y.Z

A version is semantic (``0.1.0``, or ``0.1.0-rc.1`` for a pre-release). Python writes the same
version differently (``0.1.0rc1``); the script converts. ``Cargo.lock`` and ``uv.lock`` are not
written by hand: ``set`` says so, and ``check`` compares them with the rest.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

_SEMVER = re.compile(r"(?P<core>\d+\.\d+\.\d+)(?:-(?P<kind>alpha|beta|rc)\.(?P<number>\d+))?")
_PEP440_KIND = {"alpha": "a", "beta": "b", "rc": "rc"}

_CARGO = "Cargo.toml"
_PLUGIN = "python/pytest-netscenario/pyproject.toml"
_CARGO_LOCK = "Cargo.lock"
_UV_LOCK = "uv.lock"

# `version = "x"` in the table [workspace.package]
_WORKSPACE_VERSION = re.compile(
    r'(?ms)(^\[workspace\.package\]\n(?:(?!^\[).*\n)*?version = ")([^"]*)(")'
)
# `netscenario-core = { path = "crates/netscenario-core", version = "x" }`
_INTERNAL_DEPENDENCY = re.compile(
    r'(?m)(^netscenario-[a-z-]+ = \{ path = "[^"]+", version = ")([^"]*)(")'
)
# `version = "x"` in the table [project] of the plugin, and its pin on the package
_PLUGIN_VERSION = re.compile(r'(?m)(^version = ")([^"]*)(")')
_PLUGIN_PIN = re.compile(r'("netscenario==)([^"]*)(")')


def python_version(version: str) -> str:
    """Write a semantic version as Python does (``0.1.0-rc.1`` becomes ``0.1.0rc1``).

    Args:
        version: A version such as ``0.1.0`` or ``0.1.0-rc.1``.

    Returns:
        The version as PEP 440 spells it.

    Raises:
        ValueError: The text is not a version this project uses.
    """
    match = _SEMVER.fullmatch(version)
    if match is None:
        message = f"not a version (X.Y.Z, or X.Y.Z-rc.N, -beta.N, -alpha.N): {version!r}"
        raise ValueError(message)
    if match["kind"] is None:
        return match["core"]
    return f"{match['core']}{_PEP440_KIND[match['kind']]}{match['number']}"


def _read(root: Path, name: str) -> str:
    return (root / name).read_text(encoding="utf-8")


def _sub_once(pattern: re.Pattern[str], text: str, value: str, where: str) -> str:
    result, count = pattern.subn(lambda m: f"{m[1]}{value}{m[3]}", text)
    if count != 1:
        message = f"{where}: expected one place for the version, found {count}"
        raise ValueError(message)
    return result


def collect(root: Path) -> dict[str, str]:
    """Read the version from every place that holds one, by the name of the place.

    Args:
        root: The root of the repository.

    Returns:
        Place name to the version found there, each written as the place spells it.
    """
    found: dict[str, str] = {}
    cargo = _read(root, _CARGO)
    match = _WORKSPACE_VERSION.search(cargo)
    if match is None:
        message = f"{_CARGO}: no version in [workspace.package]"
        raise ValueError(message)
    found[f"{_CARGO} workspace.package"] = match[2]
    for dependency in _INTERNAL_DEPENDENCY.finditer(cargo):
        name = dependency[0].split(" ", 1)[0]
        found[f"{_CARGO} dependency {name}"] = dependency[2]
    plugin = _read(root, _PLUGIN)
    found[f"{_PLUGIN} version"] = tomllib.loads(plugin)["project"]["version"]
    pin = _PLUGIN_PIN.search(plugin)
    found[f"{_PLUGIN} pin on netscenario"] = pin[2] if pin else "(missing)"
    for package in tomllib.loads(_read(root, _CARGO_LOCK))["package"]:
        if package["name"].startswith("netscenario-"):
            found[f"{_CARGO_LOCK} {package['name']}"] = package["version"]
    for package in tomllib.loads(_read(root, _UV_LOCK))["package"]:
        if package["name"] == "pytest-netscenario":
            found[f"{_UV_LOCK} pytest-netscenario"] = package["version"]
    return found


def check(root: Path, tag: str | None = None) -> list[str]:
    """Compare every place with the version of the workspace.

    Args:
        root: The root of the repository.
        tag: A tag such as ``v0.1.0`` that must name the same version, or ``None``.

    Returns:
        One line for each place that disagrees; empty when everything agrees.
    """
    found = collect(root)
    expected = found[f"{_CARGO} workspace.package"]
    expected_python = python_version(expected)
    problems: list[str] = []
    for place, version in found.items():
        # Python places spell the version as PEP 440; the places of Cargo as semver
        python_place = place.startswith((_PLUGIN, _UV_LOCK))
        want = expected_python if python_place else expected
        if version != want:
            problems.append(f"{place}: {version}, but the workspace has {want}")
    if tag is not None and tag != f"v{expected}":
        problems.append(f"the tag is {tag}, but the workspace has v{expected}")
    return problems


def set_version(root: Path, version: str) -> None:
    """Write a new version into the places that are written by hand.

    Args:
        root: The root of the repository.
        version: The new semantic version.

    Raises:
        ValueError: The version is not one this project uses, or a file does not have its place.
    """
    python = python_version(version)
    cargo = _read(root, _CARGO)
    cargo = _sub_once(_WORKSPACE_VERSION, cargo, version, _CARGO)
    cargo = _INTERNAL_DEPENDENCY.sub(lambda m: f"{m[1]}{version}{m[3]}", cargo)
    (root / _CARGO).write_text(cargo, encoding="utf-8")
    plugin = _read(root, _PLUGIN)
    plugin = _sub_once(_PLUGIN_VERSION, plugin, python, _PLUGIN)
    plugin = _sub_once(_PLUGIN_PIN, plugin, python, _PLUGIN)
    (root / _PLUGIN).write_text(plugin, encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    """Run the command line.

    Args:
        argv: The arguments after the program name; ``None`` takes them from the process.

    Returns:
        The exit code: 0 when the command succeeded, 1 when ``check`` found a
        disagreement, 2 for a bad call.
    """
    parser = argparse.ArgumentParser(
        prog="version.py", description="Keep the version of netscenario in one place."
    )
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="the root of the repository")
    commands = parser.add_subparsers(dest="command", required=True)
    showing = commands.add_parser("show", help="print the version of the workspace")
    showing.add_argument("--python", action="store_true", help="spell it as Python does (PEP 440)")
    checking = commands.add_parser("check", help="fail when a file disagrees with the workspace")
    checking.add_argument("--tag", help="a tag (vX.Y.Z) that must name the version")
    setting = commands.add_parser("set", help="write a new version")
    setting.add_argument("version")
    args = parser.parse_args(argv)
    root: Path = args.root
    try:
        if args.command == "show":
            shown = collect(root)[f"{_CARGO} workspace.package"]
            sys.stdout.write((python_version(shown) if args.python else shown) + "\n")
        elif args.command == "check":
            problems = check(root, args.tag)
            for problem in problems:
                sys.stderr.write(f"version: {problem}\n")
            return 1 if problems else 0
        else:
            set_version(root, args.version)
            sys.stdout.write(
                f"version {args.version} written. Now refresh the lock files: "
                "cargo update --workspace && uv lock\n"
            )
    except ValueError as error:
        sys.stderr.write(f"version: {error}\n")
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
