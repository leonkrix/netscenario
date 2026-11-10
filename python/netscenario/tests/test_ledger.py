"""The ledger (``FEATURES.md``): a row that says ``done`` names the test that proves it.

``CLAUDE.md`` and ``CONTRIBUTING.md`` ask for it: a feature has a line before it is built, and a
check mark needs a link to the test that proves it. This test keeps the ledger honest. A row
without a proof, a link that leads nowhere, and a proof that is only a document for a feature that
runs, each fail here (and the tests below prove that they would).
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
LEDGER = ROOT / "FEATURES.md"

#: The rows whose proof is a document, a configuration or a workflow and not a test: there is
#: no behaviour of the program to run. Each still needs a reference that exists.
PROOF_IS_A_FILE = frozenset(
    {
        "DOC-01",  # the README, the architecture, the decisions
        "DOC-04",  # `missing_docs` and ruff `D` are errors: the configuration is the proof
        "DOC-05",  # the troubleshooting page
        "CIX-04",  # the permissions page
        "REL-01",  # the release workflow, which runs on a tag
        "REL-02",  # the configuration of the changelog
        "REL-03",  # the versioning policy
        "PYT-03",  # pyright in strict mode over the package
        "PYT-08",  # the wheels workflow builds and installs them
        "QA-07",  # the semver and Rust-version jobs of the CI
        "QA-08",  # `cargo deny`, Scorecard, pinned actions
    }
)

#: Where a reference may point to. A backtick word is a reference only if it looks like a path.
TOP = ("crates", "python", "docs", "examples", "scenarios", "fuzz", "spikes", ".github")
PATH_WORD = re.compile(
    r"^(?:[\w./-]+\.(?:rs|py|yml|yaml|toml|md)|(?:" + "|".join(TOP) + r")/[\w./-]*)(?:#.*)?$"
)
LINK = re.compile(r"\]\(([^)\s]+)\)")
WORD = re.compile(r"`([^`\s]+)`")


@dataclass(frozen=True)
class Row:
    """One row of the ledger."""

    id: str
    status: str


def rows(text: str) -> list[Row]:
    """The rows of the tables of the ledger: an id like ``SCN-01`` in the first column."""
    found: list[Row] = []
    for line in text.splitlines():
        cells = re.split(r"(?<!\\)\|", line)
        if len(cells) >= 7 and re.fullmatch(r"\s*[A-Z]+-\d+\s*", cells[1]):
            found.append(Row(cells[1].strip(), cells[5].strip()))
    return found


def repo_files() -> list[str]:
    """The files of the repository that a reference may name, relative to the root."""
    files: list[str] = []
    for top in TOP:
        for directory, names, filenames in os.walk(ROOT / top):
            built = Path(directory) / "book" if Path(directory) == ROOT / "docs" / "book" else None
            names[:] = [
                n
                for n in names
                if n not in {"target", ".venv", "__pycache__"} and Path(directory) / n != built
            ]
            files.extend(str((Path(directory) / name).relative_to(ROOT)) for name in filenames)
    files.extend(p.name for p in ROOT.iterdir() if p.is_file())
    return files


def references(status: str) -> list[str]:
    """What the status of a row points to: links to files, and backtick words that are paths."""
    links = [t for t in LINK.findall(status) if not t.startswith(("http", "#", "mailto"))]
    words = [w for w in WORD.findall(status) if PATH_WORD.match(w)]
    return [r.split("#")[0] for r in links + words if r.split("#")[0]]


def resolve(reference: str, files: list[str]) -> list[str]:
    """The files a reference names: the file itself, the files in a directory, or the files whose
    path ends with it (a word like ``run.rs`` stands for the file of that name)."""
    path = os.path.normpath(reference)
    if path in files:
        return [path]
    inside = [f for f in files if f.startswith(path.rstrip("/") + "/")]
    if inside:
        return inside
    return [f for f in files if f.endswith("/" + path)]


def is_test(path: str) -> bool:
    """Whether a file is a test: in a ``tests`` directory, named ``test_*``, a fuzz target, or a
    source file with a ``#[cfg(test)]`` module."""
    parts = path.split("/")
    name = parts[-1]
    if "tests" in parts or "test" in parts or name.startswith("test_") or name == "tests.rs":
        return True
    if path.startswith("fuzz/fuzz_targets/"):
        return True
    if path.endswith(".rs") and "/src/" in path:
        return "#[cfg(test)]" in (ROOT / path).read_text()
    return False


def problems(text: str, files: list[str]) -> list[str]:
    """What is wrong with a ledger: a reference to nothing, a ``done`` row with no proof, a
    ``done`` row of a feature that runs whose proof is no test."""
    found: list[str] = []
    for row in rows(text):
        resolved: list[str] = []
        for reference in references(row.status):
            matches = resolve(reference, files)
            if not matches:
                found.append(f"{row.id}: {reference} is not in the repository")
            resolved.extend(matches)
        if not row.status.lower().startswith("done"):
            continue
        if not resolved:
            found.append(f"{row.id}: done, and names no test or file that proves it")
        elif row.id not in PROOF_IS_A_FILE and not any(is_test(p) for p in resolved):
            found.append(f"{row.id}: done, and none of {sorted(set(resolved))[:3]} is a test")
    return found


def test_every_done_row_of_the_ledger_names_a_test_that_exists() -> None:
    text = LEDGER.read_text()
    assert len(rows(text)) > 100, "the table was not read"
    assert problems(text, repo_files()) == []


def test_a_row_that_is_a_file_is_only_one_where_there_is_nothing_to_run() -> None:
    # The exceptions must be rows of the ledger, and must not hide a feature that runs
    ids = {row.id for row in rows(LEDGER.read_text())}
    assert ids >= PROOF_IS_A_FILE
    assert not {i for i in PROOF_IS_A_FILE if i.startswith(("WRK", "LNK", "PRB", "TML"))}


# ---- the check can fail (canaries) ----

FILES = [
    "crates/x/tests/run.rs",
    "crates/x/src/units.rs",
    "docs/COVERAGE.md",
    "python/netscenario/tests/test_a.py",
]


def ledger(*lines: str) -> str:
    return "\n".join(f"| {line} |" for line in lines)


def test_a_done_row_with_no_reference_is_a_problem() -> None:
    text = ledger("WRK-99 | A stream | M | 0.1 | done | ", "WRK-98 | Another | M | 0.1 | planned")
    assert problems(text, FILES) == ["WRK-99: done, and names no test or file that proves it"]


def test_a_reference_to_a_file_that_is_not_there_is_a_problem_in_any_row() -> None:
    text = ledger(
        "WRK-99 | A stream | M | 0.1 | done ([test](crates/x/tests/gone.rs))",
        "WRK-98 | Another | M | 0.1 | planned (see `docs/GONE.md`)",
    )
    found = problems(text, FILES)
    assert any(p == "WRK-99: crates/x/tests/gone.rs is not in the repository" for p in found)
    assert any(p == "WRK-98: docs/GONE.md is not in the repository" for p in found)


def test_a_document_is_no_proof_of_a_feature_that_runs() -> None:
    text = ledger("WRK-99 | A stream | M | 0.1 | done ([the page](docs/COVERAGE.md))")
    (problem,) = problems(text, FILES)
    assert problem.startswith("WRK-99: done, and none of")
    # ... but it is the proof of a row that is a document
    assert (
        problems(
            ledger("DOC-05 | Troubleshooting | M | 0.1 | done ([page](docs/COVERAGE.md))"), FILES
        )
        == []
    )


def test_a_test_in_a_tests_directory_a_test_file_or_a_source_with_tests_is_a_proof() -> None:
    for status in (
        "done ([t](crates/x/tests/run.rs))",
        "done (`run.rs` and `test_a.py`)",
        "done ([t](python/netscenario/tests/test_a.py))",
    ):
        assert problems(ledger(f"WRK-99 | A | M | 0.1 | {status}"), FILES) == [], status


def test_words_in_backticks_are_references_only_if_they_look_like_paths() -> None:
    status = (
        "done (`netscenario run`, `result.json` is made by a run, `10.100.n.0/30`, `tests/run.rs`)"
    )
    # A file a run makes (`result.json`), an address and a command are not files of the repository
    assert references(status) == ["tests/run.rs"]
