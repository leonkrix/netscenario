"""The command ``netscenario`` and ``python -m netscenario``."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from netscenario import __main__ as launcher
from netscenario import _binaries


def test_python_dash_m_netscenario_runs_the_program_of_the_package() -> None:
    done = subprocess.run(
        [sys.executable, "-m", "netscenario", "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    assert done.stdout.startswith("netscenario ")


def test_the_arguments_and_the_exit_code_are_those_of_the_program(repository: Path) -> None:
    good = repository / "scenarios" / "minimal.yaml"
    bad = repository / "scenarios" / "invalid" / "empty.yaml"
    run = [sys.executable, "-m", "netscenario", "validate"]
    ok = subprocess.run([*run, str(good)], capture_output=True, text=True, check=False)  # noqa: S603
    assert (ok.returncode, ok.stdout.startswith("ok: minimal")) == (0, True)
    refused = subprocess.run([*run, str(bad)], capture_output=True, text=True, check=False)  # noqa: S603
    assert refused.returncode == 2
    assert refused.stderr.startswith("error: ")


def test_the_launcher_replaces_the_process_and_passes_the_arguments(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    started: list[tuple[str, list[str]]] = []

    def fake_execv(path: str | os.PathLike[str], argv: list[str]) -> None:
        started.append((str(path), argv))
        raise SystemExit(0)

    monkeypatch.setattr(os, "execv", fake_execv)
    monkeypatch.setattr(sys, "argv", ["netscenario", "validate", "a.yaml"])
    with pytest.raises(SystemExit):
        launcher.main()
    ((path, argv),) = started
    assert Path(path).name == "netscenario"
    assert argv[1:] == ["validate", "a.yaml"]
    assert argv[0] == path


def test_without_its_program_the_launcher_says_where_it_looked_and_exits_with_three(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(_binaries, "_candidates", list)
    with pytest.raises(SystemExit) as raised:
        launcher.main()
    assert raised.value.code == 3
    assert "were not found" in capsys.readouterr().err
