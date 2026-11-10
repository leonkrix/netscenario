"""Tests of .github/scripts/ci_changes.py (which jobs a change needs) and of ci.yml around it.

The script decides what is skipped, so a mistake in it lets a change through untested. Each test
below is a canary for one way that could happen.
"""

import importlib.util
import json
import re
import shutil
import subprocess
from pathlib import Path
from types import ModuleType

import pytest

_ROOT = Path(__file__).resolve().parents[3]


def _load() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "ci_changes", _ROOT / ".github/scripts/ci_changes.py"
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


changes = _load()
CI = (_ROOT / ".github/workflows/ci.yml").read_text()


def test_a_change_of_the_book_only_skips_the_tests_and_the_wheels() -> None:
    assert changes.classify(["docs/book/src/ci.md", "docs/book/book.toml"]) == {
        "heavy": False,
        "wheels": False,
    }
    assert changes.classify([".github/workflows/book.yml"]) == {"heavy": False, "wheels": False}


@pytest.mark.parametrize(
    "path",
    [
        "crates/netscenario-core/src/run.rs",
        "Cargo.lock",
        "python/netscenario/src/netscenario/run.py",
        # Tests read these: the documents of coverage, fidelity and profiles, the scenarios
        "docs/COVERAGE.md",
        "docs/FIDELITY.md",
        "scenarios/minimal.yaml",
        "README.md",
        ".github/workflows/ci.yml",
        ".github/scripts/ci_changes.py",
        ".github/actions/setup/action.yml",
        # A file nobody has thought of is heavy
        "something/new.txt",
        "LICENSE-MIT",
    ],
)
def test_a_change_of_anything_else_runs_the_tests(path: str) -> None:
    assert changes.classify([path])["heavy"] is True
    # ... and so does the light change that has it next to it
    assert changes.classify(["docs/book/src/ci.md", path])["heavy"] is True


def test_nothing_known_about_a_change_is_heavy() -> None:
    assert changes.classify([]) == {"heavy": True, "wheels": True}
    assert changes.decide("pull_request", "", None) == {
        "heavy": True,
        "wheels": True,
        "warm": False,
    }


def test_the_wheels_are_skipped_only_for_files_that_are_not_in_one() -> None:
    # The tests and the documents are heavy for the tests and do not touch a wheel
    assert changes.classify(["docs/COVERAGE.md", "crates/netscenario-core/tests/x.rs"]) == {
        "heavy": True,
        "wheels": False,
    }
    # The source of a program the wheel carries is built in an old glibc: a new call can break it
    assert changes.classify(["crates/netscenario-agent/src/sender.rs"])["wheels"] is True
    assert changes.classify(["python/netscenario/pyproject.toml"])["wheels"] is True
    assert changes.classify(["pyproject.toml"])["wheels"] is True


def test_each_event_gets_its_own_outputs() -> None:
    assert changes.decide("push", "", None) == {"heavy": True, "wheels": False, "warm": True}
    assert changes.decide("schedule", "", None) == {"heavy": True, "wheels": True, "warm": False}
    assert changes.decide("workflow_dispatch", "", None) == {
        "heavy": True,
        "wheels": True,
        "warm": False,
    }
    assert changes.decide("workflow_dispatch", "full", None)["warm"] is False
    assert changes.decide("workflow_dispatch", "warm", None) == {
        "heavy": True,
        "wheels": False,
        "warm": True,
    }
    light = changes.decide("pull_request", "", ["docs/book/src/ci.md"])
    assert light == {"heavy": False, "wheels": False, "warm": False}


def test_a_directory_prefix_does_not_match_a_file_that_only_starts_alike() -> None:
    assert not changes.matches("docs/book-notes.md", ("docs/book/",))
    assert changes.matches("docs/book/src/a.md", ("docs/book/",))
    assert changes.matches("crates/x/tests/a.rs", ("crates/*/tests/*",))
    assert not changes.matches("crates/x/src/a.rs", ("crates/*/tests/*",))


# ---- ci.yml ----


def _jobs() -> list[str]:
    block = CI.split("\njobs:\n", 1)[1]
    return re.findall(r"^  ([a-z][a-z0-9-]*):\s*$", block, flags=re.MULTILINE)


def test_every_job_of_ci_yml_is_collected_by_the_job_that_the_ruleset_requires() -> None:
    jobs = _jobs()
    assert "ci-ok" in jobs
    assert len(jobs) >= 10
    needs = re.search(
        r"^    needs: \[(?P<names>[^\]]*)\]", CI.split("\n  ci-ok:\n", 1)[1], flags=re.MULTILINE
    )
    assert needs is not None
    collected = {name.strip() for name in needs["names"].split(",")}
    assert collected == set(jobs) - {"ci-ok"}, (
        "a job that ci-ok does not wait for is never required"
    )


def test_the_job_that_collects_names_the_jobs_that_must_have_run() -> None:
    # ci-ok accepts a skipped job only when the plan says so; a typo would let one through
    script = CI.split("\n  ci-ok:\n", 1)[1]
    must = set(re.findall(r"must\+=\( ([^)]*) \)", script))
    named = {name for group in must for name in group.split()}
    assert {"test", "rights", "msrv", "fidelity", "python", "acceptance", "clippy"} <= named
    assert named <= set(_jobs())


# ---- the verdict of ci-ok, run as the CI runs it ----

ALL_JOBS = [job for job in _jobs() if job != "ci-ok"]
LIGHT_SKIPS = {
    "fmt", "clippy", "test", "rights", "msrv", "fidelity", "docs", "package", "semver", "python",
    "acceptance", "wheels",
}  # fmt: skip
WARM_SKIPS = {"fmt", "semver", "wheels", "deny", "actionlint", "dependency-review"}


def _verdict(event: str, plan: dict[str, str], results: dict[str, str]) -> int:
    """The exit code of the script of ci-ok for an event, the plan and the results of the jobs."""
    block = CI.split("      - name: Check the result of every job\n", 1)[1]
    script = "\n".join(line[10:] for line in block.split("        run: |\n", 1)[1].splitlines())
    needs = {job: {"result": results.get(job, "success")} for job in ALL_JOBS}
    env = {
        "NEEDS": json.dumps(needs),
        "EVENT": event,
        "HEAVY": plan["heavy"],
        "WHEELS": plan["wheels"],
        "WARM": plan["warm"],
        "PATH": "/usr/bin:/bin",
    }
    # The script is the one in ci.yml of this repository, run with a fixed environment
    bash = shutil.which("bash") or "/bin/bash"
    done = subprocess.run(  # noqa: S603
        [bash, "-c", script], env=env, capture_output=True, text=True, check=False
    )
    return done.returncode


needs_jq = pytest.mark.skipif(shutil.which("jq") is None, reason="jq is on the runners of the CI")
HEAVY = {"heavy": "true", "wheels": "true", "warm": "false"}
LIGHT = {"heavy": "false", "wheels": "false", "warm": "false"}
WARM = {"heavy": "true", "wheels": "false", "warm": "true"}


@needs_jq
def test_ci_ok_passes_what_the_plan_asks_for() -> None:
    assert _verdict("pull_request", HEAVY, {}) == 0
    assert _verdict("pull_request", LIGHT, dict.fromkeys(LIGHT_SKIPS, "skipped")) == 0
    assert _verdict("push", WARM, dict.fromkeys(WARM_SKIPS, "skipped")) == 0
    assert _verdict("schedule", HEAVY, {"semver": "skipped", "dependency-review": "skipped"}) == 0
    assert _verdict("pull_request", {**HEAVY, "wheels": "false"}, {"wheels": "skipped"}) == 0


@needs_jq
@pytest.mark.parametrize(
    "job", ["test", "rights", "msrv", "fidelity", "python", "acceptance", "clippy"]
)
def test_ci_ok_fails_when_a_job_the_plan_asks_for_was_skipped(job: str) -> None:
    # The canary of the gating: an `if` that is wrong must not let a pull request through untested
    assert _verdict("pull_request", HEAVY, {job: "skipped"}) == 1
    assert _verdict("push", WARM, {**dict.fromkeys(WARM_SKIPS, "skipped"), job: "skipped"}) == 1


@needs_jq
def test_ci_ok_fails_on_a_failure_and_when_the_plan_could_not_be_made() -> None:
    assert _verdict("pull_request", HEAVY, {"python": "failure"}) == 1
    assert _verdict("pull_request", HEAVY, {"fidelity": "cancelled"}) == 1
    assert (
        _verdict(
            "pull_request", LIGHT, {**dict.fromkeys(LIGHT_SKIPS, "skipped"), "clippy": "failure"}
        )
        == 1
    )
    # `changes` failed: no outputs, nothing is known
    empty = {"heavy": "", "wheels": "", "warm": ""}
    assert (
        _verdict(
            "pull_request", empty, {"changes": "failure", **dict.fromkeys(LIGHT_SKIPS, "skipped")}
        )
        == 1
    )
    # The cheap checks of a light pull request still have to run
    assert (
        _verdict(
            "pull_request",
            LIGHT,
            {**dict.fromkeys(LIGHT_SKIPS, "skipped"), "actionlint": "skipped"},
        )
        == 1
    )
    assert (
        _verdict(
            "pull_request",
            LIGHT,
            {**dict.fromkeys(LIGHT_SKIPS, "skipped"), "dependency-review": "skipped"},
        )
        == 1
    )
