"""Check an installed wheel in a clean environment (run it with ``python -I``, from any directory).

It proves what the unit tests cannot: that the wheel carries everything a run needs and that
nothing is borrowed from a checkout. The package must come from ``site-packages``, the two programs
from inside the package, the command ``netscenario`` from the ``bin`` directory of the environment,
and a run must work with the plugin installed next to it.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import netscenario as ns
from netscenario import _binaries

SCENARIO = textwrap.dedent(
    """\
    version: 0
    name: smoke
    duration: 1s
    topology:
      nodes: [client, server]
      links:
        - id: access
          between: [client, server]
          forward: { delay: 20ms }
    workloads:
      - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
    """
)

TEST = textwrap.dedent(
    """\
    def test_the_plugin_runs_a_scenario(netscenario):
        result = netscenario.run("smoke.yaml", settle=0.3)
        assert result["media"].sent == 209
        assert result["media"].lost == 0
    """
)


def step(text: str) -> None:
    sys.stdout.write(f"- {text}\n")
    sys.stdout.flush()


def run(*command: str, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(  # noqa: S603 - the programs of the wheel under test
        command, capture_output=True, text=True, check=False, cwd=cwd
    )


def main() -> None:
    package = Path(ns.__file__).resolve().parent
    step(f"netscenario {ns.__version__} from {package}")
    assert "site-packages" in package.parts, "the package must come from site-packages"

    supervisor, agent = _binaries.find()
    step(f"programs: {supervisor}, {agent}")
    assert supervisor.parent == agent.parent == package / "_bin", "the programs come from the wheel"
    for program in (supervisor, agent):
        assert program.stat().st_mode & 0o111, f"{program} is not executable"

    version = run(str(supervisor), "--version")
    assert version.stdout.strip() == f"netscenario {ns.__version__}", version
    command = Path(sys.executable).parent / "netscenario"
    step(f"the command: {command}")
    assert run(str(command), "--version").stdout == version.stdout, "the command starts the program"
    assert run(sys.executable, "-m", "netscenario", "--version").stdout == version.stdout

    with tempfile.TemporaryDirectory(prefix="netscenario-smoke-") as directory:
        folder = Path(directory)
        (folder / "smoke.yaml").write_text(SCENARIO, encoding="utf-8")
        checked = run(str(command), "validate", str(folder / "smoke.yaml"))
        assert checked.returncode == 0, checked
        assert checked.stdout.startswith("ok: smoke"), checked

        report = ns.doctor()
        step(f"doctor: {'ok' if report.ok else 'cannot run scenarios'}")
        assert report.ok, f"this machine cannot run scenarios:\n{report.explain()}"

        result = ns.run(ns.Scenario.load(folder / "smoke.yaml"), out=folder / "out", settle=0.3)
        media = result["media"]
        step(f"a run: sent {media.sent}, lost {media.lost}")
        assert (media.sent, media.lost) == (209, 0), media
        assert media.delay is not None
        assert media.delay.min >= 0.020, "faster than the link allows"

        (folder / "test_smoke.py").write_text(TEST, encoding="utf-8")
        tested = run(sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", cwd=folder)
        step("the plugin: " + (tested.stdout.strip().splitlines() or ["no output"])[-1])
        assert tested.returncode == 0, tested.stdout + tested.stderr

    sys.stdout.write("wheel smoke test passed\n")


if __name__ == "__main__":
    main()
