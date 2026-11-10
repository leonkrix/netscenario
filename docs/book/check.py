"""Checks the fenced blocks and the links of the book (DOC-02).

Every scenario in the book is read and checked by the same reader as a run, the short ones are run, the Python is parsed and the
marked blocks are run, and the commands in the shell blocks name commands and files that exist. A page cannot drift away from the
program without this failing.

The tags of a block follow the language, separated by commas (mdBook turns them into classes and ignores the ones it does not know):

    ```yaml,scenario           a whole scenario: read and checked; the file is named by a first line `# file: name.yaml`,
                               or by the file that an include brings in
    ```yaml,scenario,run       the same, and run (at most MAX_RUN_SECONDS of duration)
    ```python                  parsed
    ```python,run              executed in a directory that holds the scenario files of the page so far
    ```bash                    the lines that start with `netscenario` name a command that exists and files that exist
    ```text,file=name.csv      any block with `file=name` is written to that name, so that a scenario can refer to it

A line `{{#include path}}` or `{{#include path:first:last}}` inside a block is replaced by the lines of the file, as mdBook does it.

Run `python docs/book/check.py` (checks everything but does not run anything), `--run` also runs the marked blocks, and page names (`first-run.md`) limit it to those pages. It needs the
`netscenario` package; the run needs a machine that can make namespaces (`netscenario doctor`).
"""

from __future__ import annotations

import argparse
import ast
import re
import shlex
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

BOOK = Path(__file__).resolve().parent
SRC = BOOK / "src"
ROOT = BOOK.parents[1]

#: The longest scenario a block may run: the book runs in the CI of every pull request.
MAX_RUN_SECONDS = 10.0
#: The longest a block of Python may take.
PYTHON_TIMEOUT_SECONDS = 180

#: The commands of the program (`netscenario -h`).
COMMANDS = frozenset(
    "run replay report validate expand show schema profiles capabilities doctor clean".split()
)

FENCE = re.compile(r"^(?P<indent>\s*)```(?P<info>[^`]*)$")
INCLUDE = re.compile(
    r"^\s*\{\{#include\s+(?P<path>[^:}\s]+)(?::(?P<first>\d*))?(?::(?P<last>\d*))?\s*\}\}\s*$"
)
FILE_MARK = re.compile(r"^#\s*file:\s*(?P<name>[\w.-]+)\s*$")
LINK = re.compile(r"(?<!!)\[[^\]]*\]\((?P<target>[^)\s]+)\)")
SUMMARY_LINK = re.compile(r"\]\((?P<target>[^)\s]+\.md)\)")
GITHUB = re.compile(
    r"https://github\.com/leonkrix/netscenario/(?:blob|tree)/main/(?P<path>[^)\s#]+)"
)


def shown(page: Path) -> str:
    """The path of a page as a message shows it: relative to the book where it is in it."""
    try:
        return str(page.relative_to(BOOK))
    except ValueError:
        return str(page)


@dataclass(frozen=True)
class Block:
    """A fenced block of a page."""

    page: Path
    line: int
    tokens: tuple[str, ...]
    text: str

    @property
    def language(self) -> str:
        """The first word of the info string."""
        return self.tokens[0] if self.tokens else ""

    def where(self) -> str:
        """`page:line` for a message."""
        return f"{shown(self.page)}:{self.line}"


def expand_includes(
    lines: list[str], page: Path, problems: list[str], line_no: int
) -> tuple[list[str], str | None]:
    """The lines of a block with the includes replaced, and the name of the file of the last include."""
    out: list[str] = []
    included: str | None = None
    for line in lines:
        match = INCLUDE.match(line)
        if match is None:
            out.append(line)
            continue
        path = (page.parent / match["path"]).resolve()
        if not path.is_file():
            problems.append(f"{shown(page)}:{line_no}: the include {match['path']} is not a file")
            continue
        text = path.read_text().split("\n")
        if text and text[-1] == "":
            text.pop()
        first = int(match["first"]) if match["first"] else 1
        last = int(match["last"]) if match["last"] else len(text)
        out.extend(text[first - 1 : last])
        included = path.name
    return out, included


def blocks(page: Path, problems: list[str]) -> Iterator[tuple[Block, str | None]]:
    """The fenced blocks of a page, in order, with the name of an included file if the block brought one in."""
    lines = page.read_text().split("\n")
    index = 0
    while index < len(lines):
        match = FENCE.match(lines[index])
        if match is None:
            index += 1
            continue
        start = index + 1
        indent = match["indent"]
        index += 1
        body: list[str] = []
        while index < len(lines) and not lines[index].startswith(f"{indent}```"):
            body.append(
                lines[index][len(indent) :] if lines[index].startswith(indent) else lines[index]
            )
            index += 1
        if index >= len(lines):
            problems.append(f"{shown(page)}:{start}: a block is never closed")
        index += 1
        tokens = tuple(token.strip() for token in match["info"].strip().split(",") if token.strip())
        expanded, included = expand_includes(body, page, problems, start)
        yield Block(page, start, tokens, "\n".join(expanded) + "\n"), included


def seconds(text: str) -> float | None:
    """A duration such as `5s`, `250ms` or `2m` in seconds."""
    match = re.fullmatch(r"(\d+(?:\.\d+)?)(ms|s|m)", text.strip())
    if match is None:
        return None
    return float(match[1]) * {"ms": 0.001, "s": 1.0, "m": 60.0}[match[2]]


def check_scenario(
    block: Block, included: str | None, work: Path, run: bool, problems: list[str]
) -> None:
    """Reads the scenario of a block with the reader of a run, and runs it if the block says so."""
    import netscenario as ns

    first = block.text.split("\n", 1)[0]
    mark = FILE_MARK.match(first)
    name = mark["name"] if mark else included
    if name is None:
        problems.append(
            f"{block.where()}: a scenario block needs a first line `# file: name.yaml` or an include"
        )
        return
    path = work / name
    path.write_text(block.text)
    try:
        scenario = ns.Scenario.load(path)
    except ns.ScenarioError as error:
        problems.append(
            f"{block.where()}: the scenario {name} is refused: {str(error).splitlines()[0]}"
        )
        return
    if "run" not in block.tokens or not run:
        return
    length = seconds(str(scenario.data.get("duration", "")))
    if length is None or length > MAX_RUN_SECONDS:
        problems.append(
            f"{block.where()}: a scenario that is run lasts at most {MAX_RUN_SECONDS:g} s"
        )
        return
    try:
        ns.run(scenario, out=work / f"out-{block.line}", settle=0.5)
    except ns.NetscenarioError as error:
        problems.append(
            f"{block.where()}: the scenario {name} did not run: {str(error).splitlines()[0]}"
        )


def check_python(block: Block, work: Path, run: bool, problems: list[str]) -> None:
    """Parses the Python of a block, and runs it in the directory of the page if the block says so."""
    try:
        ast.parse(block.text)
    except SyntaxError as error:
        problems.append(
            f"{block.where()}: the Python does not parse: {error.msg} (line {error.lineno})"
        )
        return
    if "run" not in block.tokens or not run:
        return
    script = work / f"block-{block.line}.py"
    script.write_text(block.text)
    try:
        done = subprocess.run(
            [sys.executable, str(script)],
            cwd=work,
            capture_output=True,
            text=True,
            timeout=PYTHON_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        problems.append(f"{block.where()}: the Python took more than {PYTHON_TIMEOUT_SECONDS} s")
        return
    if done.returncode != 0:
        tail = (done.stderr or done.stdout).strip().splitlines()[-3:]
        problems.append(f"{block.where()}: the Python failed: {' | '.join(tail)}")


def check_shell(block: Block, work: Path, problems: list[str]) -> None:
    """The commands of a shell block: a command that exists, and files that are there."""
    for raw in block.text.splitlines():
        line = raw.strip().removeprefix("$ ")
        if not line.startswith("netscenario "):
            continue
        try:
            words = shlex.split(line)
        except ValueError:
            problems.append(f"{block.where()}: cannot read the line `{line}`")
            continue
        if len(words) < 2:
            continue
        command = words[1]
        if not command.startswith("-") and command not in COMMANDS:
            problems.append(f"{block.where()}: `netscenario {command}` is not a command")
        for word in words[2:]:
            if re.search(r"\.(ya?ml)$", word) and not (
                (work / word).is_file() or (ROOT / word).is_file()
            ):
                problems.append(
                    f"{block.where()}: the file {word} is neither in this page nor in the repository"
                )


def check_page(page: Path, run: bool, problems: list[str]) -> None:
    """All the blocks of one page."""
    with tempfile.TemporaryDirectory(prefix="netscenario-book-") as directory:
        work = Path(directory)
        for block, included in blocks(page, problems):
            named = [
                token.removeprefix("file=") for token in block.tokens if token.startswith("file=")
            ]
            if named:
                (work / named[0]).write_text(block.text)
            elif block.language == "yaml" and "scenario" in block.tokens:
                check_scenario(block, included, work, run, problems)
            elif block.language == "python":
                check_python(block, work, run, problems)
            elif block.language in {"bash", "sh", "console"}:
                check_shell(block, work, problems)


def summary_pages() -> list[Path]:
    """The pages that `SUMMARY.md` lists, in order."""
    text = (SRC / "SUMMARY.md").read_text()
    return [(SRC / match["target"]).resolve() for match in SUMMARY_LINK.finditer(text)]


def check_links(page: Path, problems: list[str]) -> None:
    """Links to other pages and to files of the repository must lead somewhere."""
    text = page.read_text()
    for match in LINK.finditer(text):
        target = match["target"]
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        path = (page.parent / target.split("#", 1)[0]).resolve()
        if not path.exists():
            problems.append(f"{shown(page)}: the link {target} leads nowhere")
    for match in GITHUB.finditer(text):
        if not (ROOT / match["path"]).exists():
            problems.append(f"{shown(page)}: {match['path']} is not in the repository")


def check_book(run: bool = False, only: frozenset[str] = frozenset()) -> list[str]:
    """Checks the whole book, or only the pages named in `only` (`first-run.md`); one line per problem."""
    problems: list[str] = []
    listed = summary_pages()
    for page in listed:
        if not page.is_file():
            problems.append(f"SUMMARY.md lists {page.relative_to(SRC)}, which is not a file")
    on_disk = {path.resolve() for path in SRC.rglob("*.md")} - {(SRC / "SUMMARY.md").resolve()}
    for page in sorted(on_disk - set(listed)):
        problems.append(f"{page.relative_to(SRC)} is not in SUMMARY.md, so it is never built")
    for page in listed:
        if page.is_file() and (not only or page.name in only):
            check_links(page, problems)
            check_page(page, run, problems)
    return problems


def main() -> int:
    """The command line."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--run", action="store_true", help="also run the marked scenarios and Python"
    )
    parser.add_argument(
        "pages", nargs="*", help="only these pages (file names such as first-run.md)"
    )
    arguments = parser.parse_args()
    problems = check_book(arguments.run, frozenset(arguments.pages))
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"book: {len(problems)} problems" if problems else "book: ok")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
