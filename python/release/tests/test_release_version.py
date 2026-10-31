"""Tests of python/release/version.py, which keeps the version of the workspace in step."""

import importlib.util
import shutil
import time
from pathlib import Path
from types import ModuleType

import pytest

_ROOT = Path(__file__).resolve().parents[3]
_FILES = ["Cargo.toml", "Cargo.lock", "uv.lock", "python/pytest-netscenario/pyproject.toml"]


def _load() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "release_version", _ROOT / "python/release/version.py"
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


version = _load()


@pytest.fixture
def copy(tmp_path: Path) -> Path:
    """The files that hold a version, copied to a place where a test may change them."""
    for name in _FILES:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(_ROOT / name, target)
    return tmp_path


def test_the_repository_agrees_with_itself() -> None:
    assert version.check(_ROOT) == []


@pytest.mark.parametrize(
    ("semantic", "python"),
    [
        ("0.1.0", "0.1.0"),
        ("1.20.3", "1.20.3"),
        ("0.1.0-rc.1", "0.1.0rc1"),
        ("0.1.0-beta.12", "0.1.0b12"),
        ("0.1.0-alpha.2", "0.1.0a2"),
    ],
)
def test_python_spells_a_version_as_pep_440(semantic: str, python: str) -> None:
    assert version.python_version(semantic) == python


@pytest.mark.parametrize("text", ["0.1.0", "0.1.0-rc.1", "9.8.7-beta.12", "0.0.1-alpha.3"])
def test_the_package_and_the_release_script_spell_a_version_alike(text: str) -> None:
    # Two copies of one rule (the package cannot import a build tool): they must not drift apart
    from netscenario._version import pep440

    assert pep440(text) == version.python_version(text)


@pytest.mark.parametrize("text", ["1", "1.2", "v1.2.3", "1.2.3-dev", "1.2.3-rc", "01.2.3.4", ""])
def test_a_version_that_is_not_one_is_refused(text: str) -> None:
    with pytest.raises(ValueError, match="not a version"):
        version.python_version(text)


def test_set_writes_every_place_that_is_written_by_hand(copy: Path) -> None:
    version.set_version(copy, "0.2.0-rc.1")
    found = version.collect(copy)
    assert found["Cargo.toml workspace.package"] == "0.2.0-rc.1"
    assert found["python/pytest-netscenario/pyproject.toml version"] == "0.2.0rc1"
    assert found["python/pytest-netscenario/pyproject.toml pin on netscenario"] == "0.2.0rc1"
    dependencies = [place for place in found if place.startswith("Cargo.toml dependency")]
    assert len(dependencies) == 5
    assert all(found[place] == "0.2.0-rc.1" for place in dependencies)


def test_the_lock_files_are_reported_until_they_are_refreshed(copy: Path) -> None:
    version.set_version(copy, "9.9.9")
    problems = version.check(copy)
    assert problems
    assert all(problem.startswith(("Cargo.lock", "uv.lock")) for problem in problems)


def _current(root: Path) -> str:
    """The version the files of `root` have: the tests must not depend on which release this is."""
    return version.collect(root)["Cargo.toml workspace.package"]


# Canaries: the check can fail. `{v}` is the version of the files, `{p}` its spelling in Python.
@pytest.mark.parametrize(
    ("name", "old", "new", "place"),
    [
        (
            "Cargo.toml",
            'netscenario-core = {{ path = "crates/netscenario-core", version = "{v}"',
            'netscenario-core = {{ path = "crates/netscenario-core", version = "9.9.9"',
            "dependency netscenario-core",
        ),
        (
            "python/pytest-netscenario/pyproject.toml",
            '"netscenario=={p}"',
            '"netscenario==9.9.9"',
            "pin on netscenario",
        ),
        (
            "python/pytest-netscenario/pyproject.toml",
            '\nversion = "{p}"',
            '\nversion = "9.9.9"',
            "pyproject.toml version",
        ),
        (
            "Cargo.lock",
            'name = "netscenario-cli"\nversion = "{v}"',
            'name = "netscenario-cli"\nversion = "9.9.9"',
            "netscenario-cli",
        ),
    ],
)
def test_a_place_that_disagrees_is_found(
    copy: Path, name: str, old: str, new: str, place: str
) -> None:
    current = _current(copy)
    spelled = {"v": current, "p": version.python_version(current)}
    old, new = old.format(**spelled), new.format(**spelled)
    path = copy / name
    text = path.read_text(encoding="utf-8")
    assert old in text
    path.write_text(text.replace(old, new, 1), encoding="utf-8")
    problems = version.check(copy)
    assert len(problems) == 1
    assert place in problems[0]


def test_a_tag_that_does_not_name_the_version_is_found(copy: Path) -> None:
    current = _current(copy)
    assert version.check(copy, tag=f"v{current}") == []
    assert len(version.check(copy, tag="v9.9.9")) == 1


def test_the_command_line_exit_codes(copy: Path, capsys: pytest.CaptureFixture[str]) -> None:
    current = _current(copy)
    assert version.main(["--root", str(copy), "check"]) == 0
    assert version.main(["--root", str(copy), "check", "--tag", "v9.9.9"]) == 1
    assert "v9.9.9" in capsys.readouterr().err
    assert version.main(["--root", str(copy), "set", "banana"]) == 2
    assert version.main(["--root", str(copy), "show"]) == 0
    assert capsys.readouterr().out.strip() == current
    version.set_version(copy, "0.3.0-rc.2")
    assert version.main(["--root", str(copy), "show", "--python"]) == 0
    assert capsys.readouterr().out.strip() == "0.3.0rc2"


def test_the_pattern_for_the_workspace_version_finds_the_right_line() -> None:
    text = (
        "[workspace]\nmembers = []\n\n"
        '[workspace.package]\nrust-version = "1.89"\nversion = "1.2.3"\n\n'
        '[other]\nversion = "9"\n'
    )
    match = version._WORKSPACE_VERSION.search(text)
    assert match is not None
    assert match.group(2) == "1.2.3"
    # Canary: a `version` in a later table is not the version of the workspace
    assert (
        version._WORKSPACE_VERSION.search('[workspace.package]\n\n[other]\nversion = "9"\n') is None
    )


def test_the_pattern_for_the_workspace_version_does_not_backtrack() -> None:
    # CodeQL found a pattern here that took exponential time on empty lines (py/redos): 40 lines
    # would run for days. A table without a `version` is the worst case.
    text = "[workspace.package]\n" + "\n" * 40
    started = time.perf_counter()
    assert version._WORKSPACE_VERSION.search(text) is None
    assert time.perf_counter() - started < 1.0


def test_every_published_package_carries_the_licence_texts_of_the_repository() -> None:
    # cargo and the Python build tools package only what is inside the folder of a package, so each
    # one has copies (not links: a link out of the folder is refused when an sdist is unpacked)
    packages = [
        *sorted((_ROOT / "crates").glob("netscenario-*")),
        _ROOT / "python/netscenario",
        _ROOT / "python/pytest-netscenario",
    ]
    published = [p for p in packages if p.name != "netscenario-py"]
    assert len(published) == 7
    for package in published:
        for name in ("LICENSE-MIT", "LICENSE-APACHE"):
            copy = package / name
            assert copy.is_file(), f"{copy} is missing"
            assert not copy.is_symlink(), f"{copy} is a link"
            assert copy.read_bytes() == (_ROOT / name).read_bytes(), f"{copy} differs from the root"
