"""The book (``docs/book``): its scenarios, its Python, its commands and its links are checked.

``docs/book/check.py`` reads the fenced blocks of every page. The tests here run it on the book
itself, so that a page cannot drift away from the program, and give it faulty pages to prove that
it can fail.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

CHECK = Path(__file__).resolve().parents[3] / "docs" / "book" / "check.py"


def load_checker() -> ModuleType:
    spec = importlib.util.spec_from_file_location("book_check", CHECK)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # A dataclass looks its module up by name
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


check = load_checker()

GOOD = """\
version: 0
name: good
duration: 2s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
"""


def page_with(tmp_path: Path, text: str) -> Path:
    page = tmp_path / "page.md"
    page.write_text(text)
    return page


def problems_of(tmp_path: Path, text: str, run: bool = False) -> list[str]:
    problems: list[str] = []
    check.check_page(page_with(tmp_path, text), run, problems)
    return problems


# ---- the book itself ----


def test_every_block_and_link_of_the_book_is_good() -> None:
    assert check.check_book() == []


@pytest.mark.once
@pytest.mark.usefixtures("runnable")
def test_the_scenarios_and_the_python_the_book_marks_for_a_run_run() -> None:
    # It takes about three minutes and does not depend on the version of Python: the CI runs it once
    assert check.check_book(run=True) == []


# ---- the check can fail (canaries) ----


def test_a_good_scenario_block_passes(tmp_path: Path) -> None:
    assert problems_of(tmp_path, f"```yaml,scenario\n# file: good.yaml\n{GOOD}```\n") == []


def test_a_scenario_the_reader_refuses_is_a_problem_with_the_line_of_the_block(
    tmp_path: Path,
) -> None:
    text = (
        "intro\n\n```yaml,scenario\n# file: bad.yaml\n"
        + GOOD.replace("rate: 1mbit", "rate: 1")
        + "```\n"
    )
    (problem,) = problems_of(tmp_path, text)
    assert "page.md:3" in problem
    assert "bad.yaml is refused" in problem


def test_a_scenario_block_without_a_file_name_is_a_problem(tmp_path: Path) -> None:
    (problem,) = problems_of(tmp_path, f"```yaml,scenario\n{GOOD}```\n")
    assert "# file: name.yaml" in problem


def test_a_scenario_that_is_too_long_to_run_in_a_ci_is_a_problem(tmp_path: Path) -> None:
    long = GOOD.replace("duration: 2s", "duration: 60s")
    (problem,) = problems_of(
        tmp_path, f"```yaml,scenario,run\n# file: long.yaml\n{long}```\n", run=True
    )
    assert "at most" in problem


def test_python_that_does_not_parse_or_fails_when_run_is_a_problem(tmp_path: Path) -> None:
    (parse,) = problems_of(tmp_path, "```python\ndef broken(:\n```\n")
    assert "does not parse" in parse
    # Parsing is enough without the tag
    assert problems_of(tmp_path, "```python\nraise SystemExit(1)\n```\n", run=True) == []
    (failed,) = problems_of(tmp_path, "```python,run\nraise SystemExit(3)\n```\n", run=True)
    assert "failed" in failed
    assert problems_of(tmp_path, "```python,run\nprint('fine')\n```\n", run=True) == []


def test_a_command_that_does_not_exist_or_a_file_that_is_not_there_is_a_problem(
    tmp_path: Path,
) -> None:
    assert problems_of(tmp_path, "```bash\nnetscenario doctor --timers\n```\n") == []
    unknown = problems_of(tmp_path, "```bash\nnetscenario fly\n```\n")
    assert len(unknown) == 1
    assert "not a command" in unknown[0]
    (missing,) = problems_of(tmp_path, "```bash\n$ netscenario run nowhere.yaml\n```\n")
    assert "nowhere.yaml" in missing
    # A file the page made is there
    made = f"```yaml,scenario\n# file: here.yaml\n{GOOD}```\n"
    text = made + "\n```bash\nnetscenario run here.yaml --seed 1\n```\n"
    assert problems_of(tmp_path, text) == []


def test_a_file_named_in_a_block_is_written_for_the_scenarios_after_it(tmp_path: Path) -> None:
    trace = "```text,file=drive.csv\ntime,rate\n0s,10mbit\n1s,5mbit\n```\n"
    scenario = GOOD.replace(
        "      between: [a, b]\n",
        "      between: [a, b]\n      model:\n        kind: trace\n"
        "        both: { file: drive.csv }\n",
    )
    assert (
        problems_of(tmp_path, f"{trace}\n```yaml,scenario\n# file: t.yaml\n{scenario}```\n") == []
    )
    (problem,) = problems_of(tmp_path, f"```yaml,scenario\n# file: t.yaml\n{scenario}```\n")
    assert "t.yaml is refused" in problem


def test_an_include_brings_in_the_lines_of_a_file_and_a_missing_one_is_a_problem(
    tmp_path: Path,
) -> None:
    (tmp_path / "part.yaml").write_text("# first\n" + GOOD)
    ok = "```yaml,scenario\n{{#include part.yaml:2:}}\n```\n"
    assert problems_of(tmp_path, ok) == []
    problems = problems_of(tmp_path, "```yaml,scenario\n{{#include gone.yaml}}\n```\n")
    assert any("gone.yaml is not a file" in problem for problem in problems), problems


def test_a_link_that_leads_nowhere_is_a_problem(tmp_path: Path) -> None:
    page = page_with(
        tmp_path,
        "[a](page.md) [b](missing.md) [c](#top) [d](https://example.org/x.md)\n"
        "[e](https://github.com/leonkrix/netscenario/blob/main/no/such/file.md)\n"
        "[f](https://github.com/leonkrix/netscenario/blob/main/README.md)\n",
    )
    problems: list[str] = []
    check.check_links(page, problems)
    assert len(problems) == 2, problems
    assert "missing.md" in problems[0]
    assert "no/such/file.md" in problems[1]


def test_a_page_that_is_not_listed_or_a_listing_without_a_page_is_a_problem(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    src = tmp_path / "src"
    src.mkdir()
    (src / "SUMMARY.md").write_text("# Summary\n\n- [One](one.md)\n- [Two](two.md)\n")
    (src / "one.md").write_text("# One\n")
    (src / "three.md").write_text("# Three\n")
    monkeypatch.setattr(check, "BOOK", tmp_path)
    monkeypatch.setattr(check, "SRC", src)
    problems = check.check_book()
    assert any("two.md, which is not a file" in p for p in problems), problems
    assert any("three.md is not in SUMMARY.md" in p for p in problems), problems
    assert len(problems) == 2


def test_a_block_that_is_never_closed_is_a_problem(tmp_path: Path) -> None:
    (problem,) = problems_of(tmp_path, "```python\nx = 1\n")
    assert "never closed" in problem
