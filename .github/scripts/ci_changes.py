"""Decides which jobs of the CI a change needs (ci.yml, job `changes`).

The rule is a safe one: a change is *light*, and skips the tests, only if every file in it is on the short list of files that no test, no
build and no package reads. A file that is on no list is heavy, so a new kind of file runs everything until somebody decides otherwise
(tests: python/release/tests/test_ci_changes.py).

Outputs, written to the file that GitHub gives in GITHUB_OUTPUT:

    heavy   the tests and the checks of the code run (everything but the cheap checks of workflows and dependencies)
    wheels  the wheels are built and installed into clean environments
    warm    only build, to fill the caches of the default branch: the run after a merge. The tests were run on the pull request,
            and the ruleset has `strict_required_status_checks_policy`, so the merged tree is the one that was tested.
"""

from __future__ import annotations

import fnmatch
import os
import subprocess
import sys

#: Files that nothing builds, tests or packages. Prefixes end in a slash.
LIGHT = (
    "docs/book/",
    ".github/ISSUE_TEMPLATE/",
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".github/CODEOWNERS",
    ".github/workflows/book.yml",
    "CODE_OF_CONDUCT.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "CHANGELOG.md",
)

#: Files that are not in a wheel and cannot change how one is built. A change of these only (and of the light ones) skips the wheels.
NOT_IN_A_WHEEL = (
    "docs/",
    "examples/",
    "scenarios/",
    "fuzz/",
    "spikes/",
    "FEATURES.md",
    "README.md",
    "crates/*/tests/*",
    "python/*/tests/*",
    ".github/workflows/fuzz.yml",
    ".github/workflows/stability.yml",
    ".github/workflows/security.yml",
    ".github/workflows/codeql.yml",
    ".github/workflows/spikes.yml",
    ".github/dependabot.yml",
)


def matches(path: str, patterns: tuple[str, ...]) -> bool:
    """Whether `path` is one of the files or below one of the directories in `patterns`."""
    for pattern in patterns:
        if pattern.endswith("/"):
            if path.startswith(pattern):
                return True
        elif fnmatch.fnmatchcase(path, pattern) or path == pattern:
            return True
    return False


def classify(paths: list[str]) -> dict[str, bool]:
    """What a change of these files needs. An empty list is not a light change: nothing is known about it."""
    if not paths:
        return {"heavy": True, "wheels": True}
    heavy = not all(matches(path, LIGHT) for path in paths)
    wheels = not all(matches(path, LIGHT + NOT_IN_A_WHEEL) for path in paths)
    return {"heavy": heavy, "wheels": wheels}


def changed_files(base_sha: str) -> list[str] | None:
    """The files a pull request changes: its merge commit against the base. `None` if git cannot say."""
    try:
        subprocess.run(
            ["git", "fetch", "--no-tags", "--depth=1", "origin", base_sha],
            check=True,
            capture_output=True,
        )
        done = subprocess.run(
            ["git", "diff", "--name-only", base_sha, "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return [line for line in done.stdout.splitlines() if line]


def decide(event: str, requested: str, files: list[str] | None) -> dict[str, bool]:
    """The outputs for an event.

    A pull request is classified by its files; when they cannot be listed it is heavy. A schedule runs everything. A push to the
    default branch only warms the caches. A manual run is full unless it asks for `warm`.
    """
    if event == "push" or (event == "workflow_dispatch" and requested == "warm"):
        return {"heavy": True, "wheels": False, "warm": True}
    if event == "pull_request":
        return {**classify(files or []), "warm": False}
    return {"heavy": True, "wheels": True, "warm": False}


def main() -> int:
    """Reads the event from the environment and writes the outputs."""
    event = os.environ.get("EVENT", "")
    files = (
        changed_files(os.environ["BASE_SHA"])
        if event == "pull_request" and os.environ.get("BASE_SHA")
        else None
    )
    result = decide(event, os.environ.get("REQUESTED", ""), files)
    lines = [f"{key}={str(value).lower()}" for key, value in result.items()]
    print(f"event {event}: {', '.join(lines)}")
    if files is not None:
        print(f"{len(files)} files changed")
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8") as out:
            out.write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
