"""Runs in real network namespaces. They stop where the machine cannot make them (and fail on the
CI)."""

from __future__ import annotations

import _thread
import itertools
import json
import threading
import time
from pathlib import Path

import pytest

import netscenario as ns
from netscenario import _binaries
from netscenario.events import RunEvent

pytestmark = pytest.mark.usefixtures("runnable")


def scenario(
    duration: str = "2s", link: ns.Impairments | None = None, seed: int | None = None
) -> ns.Scenario:
    return (
        ns.Scenario.builder("py-test", duration=duration, seed=seed)
        .nodes("client", "server")
        .link("access", "client", "server", forward=link)
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .build()
    )


def test_a_run_measures_the_link_in_the_units_of_the_package(tmp_path: Path) -> None:
    run = ns.run(scenario(link=ns.Impairments(delay="20ms")), out=tmp_path / "out", settle=0.3)
    media = run["media"]
    # 2 mbit with 1200 byte payloads is 208.33 packets per second: 417 in two seconds
    assert (media.sent, media.received, media.lost) == (417, 417, 0)
    assert media.delay is not None
    assert 0.020 <= media.delay.p50 < 0.022, "the median delay is in seconds"
    assert media.delay.min >= 0.020, "faster than the link allows"
    assert run.out_dir == tmp_path / "out"
    assert (run.out_dir / "result.json").is_file()
    assert media.records_file.is_file()
    assert run.seed.drawn
    assert run.provenance.backend == "kernel"
    assert run.settle == pytest.approx(0.3)
    # What the package returns is what is on disk
    on_disk = json.loads((run.out_dir / "result.json").read_text(encoding="utf-8"))
    assert dict(run.raw) == on_disk


def test_the_observer_hears_the_phases_in_order() -> None:
    events: list[RunEvent] = []
    ns.run(scenario(), settle=0.2, observer=events.append)
    names = [type(event).__name__ for event in events]
    assert names[:3] == ["Provisioned", "ReceiversReady", "Started"]
    assert names[-3:] == ["Collecting", "TearingDown", "Finished"]
    progress = [e for e in events if isinstance(e, ns.events.Progress)]
    assert len(progress) >= 2
    assert progress[-1].elapsed == progress[-1].total == 2.0
    assert all(a.elapsed <= b.elapsed for a, b in itertools.pairwise(progress))
    first = events[0]
    assert isinstance(first, ns.events.Provisioned)
    assert (first.nodes, first.links) == (2, 1)


TIMELINE = """
version: 0
name: py-timeline
duration: 2s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: 10ms, rate: 4mbit }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
timeline:
  - { at: 500ms, link: access, set: { forward: { delay: 30ms } } }
"""


def test_a_step_of_the_timeline_is_in_the_result_and_the_observer_hears_it(tmp_path: Path) -> None:
    events: list[RunEvent] = []
    run = ns.run(
        ns.Scenario.from_yaml(TIMELINE),
        out=tmp_path / "out",
        settle=0.3,
        observer=events.append,
    )
    (step,) = run.steps
    assert (step.event, step.kind, step.link, step.direction) == (0, "set", "access", "forward")
    assert step.scheduled == pytest.approx(0.5)
    assert 0.0 <= step.lateness < 0.05, "never early, and 50 ms is room for a busy machine"
    heard = [e for e in events if isinstance(e, ns.events.StepDone)]
    assert [e.event for e in heard] == [0]
    assert 0.0 <= heard[0].lateness < 0.05
    names = [type(event).__name__ for event in events]
    assert names.index("Started") < names.index("StepDone") < names.index("Settling")
    # The delay of the packets went from 10 to 30 ms at half a second: both are in the run
    media = run["media"]
    assert media.delay is not None
    assert media.delay.min >= 0.010
    assert media.delay.max >= 0.030


def test_the_same_seed_loses_the_same_packets(tmp_path: Path) -> None:
    lossy = scenario(link=ns.Impairments(loss="20%"), seed=42)

    def lost(name: str, seed: int | None = None) -> list[int]:
        run = ns.run(lossy, out=tmp_path / name, packets=True, settle=0.2, seed=seed)
        packets = run["media"].packets_file
        assert packets is not None
        lines = [json.loads(line) for line in packets.read_text(encoding="utf-8").splitlines()]
        return [line["sequence"] for line in lines if line.get("lost")]

    first, again = lost("a"), lost("b")
    assert first == again
    assert 0.10 < len(first) / 417 < 0.30
    # --seed replaces the seed of the scenario: another seed, other packets
    assert lost("c", seed=43) != first


def test_a_run_can_be_repeated_with_the_seed_that_it_drew() -> None:
    lossy = scenario(link=ns.Impairments(loss="20%"))
    first = ns.run(lossy, settle=0.2)
    assert first.seed.drawn
    assert first.seed.value is not None
    again = ns.run(lossy, seed=first.seed.value, settle=0.2)
    assert not again.seed.drawn
    assert again["media"].lost == first["media"].lost


def test_a_cancel_token_stops_a_run_and_leaves_nothing(tmp_path: Path) -> None:
    token = ns.CancelToken()
    assert not token.is_cancelled
    timer = threading.Timer(1.5, token.cancel)
    timer.start()
    began = time.monotonic()
    with pytest.raises(ns.Cancelled, match="cancelled"):
        ns.run(scenario("10s"), out=tmp_path / "out", cancel=token)
    assert time.monotonic() - began < 7, "the run of ten seconds was stopped"
    assert token.is_cancelled
    assert not (tmp_path / "out" / "result.json").exists()
    timer.join()


def test_ctrl_c_stops_a_run_with_a_keyboard_interrupt(tmp_path: Path) -> None:
    # `interrupt_main` is what Ctrl-C does to the main thread: the run is cancelled, tears down, and
    # the
    # interrupt is raised afterwards
    timer = threading.Timer(1.5, _thread.interrupt_main)
    timer.start()
    began = time.monotonic()
    with pytest.raises(KeyboardInterrupt):
        ns.run(scenario("10s"), out=tmp_path / "out")
    assert time.monotonic() - began < 7
    assert not (tmp_path / "out" / "result.json").exists()
    timer.join()


def test_an_error_in_the_observer_stops_the_run_and_is_raised() -> None:
    class Broken(Exception):  # noqa: N818
        pass

    def observer(event: RunEvent) -> None:
        if isinstance(event, ns.events.Started):
            message = "the observer broke"
            raise Broken(message)

    began = time.monotonic()
    with pytest.raises(Broken, match="the observer broke"):
        ns.run(scenario("10s"), observer=observer)
    assert time.monotonic() - began < 7, "the run did not go on for ten seconds"


def test_what_the_backend_cannot_reproduce_is_refused_with_the_line_of_the_file(
    tmp_path: Path,
) -> None:
    file = tmp_path / "ns.yaml"
    file.write_text(
        "version: 0\nname: ns\nduration: 2s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n"
        "      between: [a, b]\n      forward: { delay: 1500ns }\nworkloads:\n"
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n",
        encoding="utf-8",
    )
    with pytest.raises(ns.CapabilityError) as raised:
        ns.run(file)
    assert raised.value.backend == "kernel"
    assert "1500ns" in str(raised.value)
    assert f"{file}:9:" in str(raised.value)
    (refusal,) = raised.value.refusals
    assert refusal.feature == "fixed-delay"
    assert refusal.path == "topology.links[0].forward.delay"
    assert "multiple of 1us" in refusal.reason
    assert "Engine" in refusal.fix


def test_a_faulty_scenario_file_is_a_scenario_error(tmp_path: Path) -> None:
    file = tmp_path / "bad.yaml"
    file.write_text("version: 0\nname: bad\n", encoding="utf-8")
    with pytest.raises(ns.ScenarioError):
        ns.run(file)


def test_without_its_programs_a_run_is_a_backend_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(_binaries, "_candidates", list)
    with pytest.raises(ns.BackendError, match="were not found"):
        ns.run(scenario())


def test_every_error_is_a_netscenario_error() -> None:
    for error in (ns.ScenarioError, ns.CapabilityError, ns.BackendError, ns.RunError, ns.Cancelled):
        assert issubclass(error, ns.NetscenarioError)


def test_the_doctor_reports_this_machine() -> None:
    report = ns.doctor()
    assert report.ok
    assert report.problems == ()
    assert report.explain() == ""
    ids = {check.id for check in report.checks}
    assert {"tool-tc", "can-create-namespaces", "netem"} <= ids
