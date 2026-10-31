"""The example in the README of the package is what PyPI shows, so it must run."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

README = Path(__file__).resolve().parents[1] / "README.md"


def _python_example() -> str:
    blocks = re.findall(r"```python\n(.*?)```", README.read_text(encoding="utf-8"), re.DOTALL)
    assert blocks, "the README has no Python example"
    return blocks[0]


@pytest.mark.usefixtures("runnable")
def test_the_example_of_the_readme_runs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(tmp_path)
    exec(compile(_python_example(), str(README), "exec"), {})  # noqa: S102 - our own README
    assert (tmp_path / "video-call-result" / "result.json").is_file()


def test_canary_the_example_is_found_and_a_broken_one_fails() -> None:
    example = _python_example()
    assert "ns.run(" in example
    with pytest.raises(NameError):
        exec(compile(example.replace("import netscenario as ns", ""), "x", "exec"), {})  # noqa: S102
