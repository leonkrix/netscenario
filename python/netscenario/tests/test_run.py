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


def test_the_series_adds_up_to_the_totals_and_the_bin_is_the_one_asked_for(tmp_path: Path) -> None:
    run = ns.run(
        scenario(link=ns.Impairments(delay="20ms")), out=tmp_path / "out", settle=0.3, bin=0.5
    )
    media = run["media"]
    assert media.series is not None
    assert media.series.bin == pytest.approx(0.5)
    bins = media.series.read()
    assert len(bins) == media.series.bins
    assert [b.start for b in bins] == [0.5 * i for i in range(len(bins))]
    assert sum(b.by_send.sent for b in bins) == media.sent == 417
    assert sum(b.by_send.received for b in bins) == media.received
    assert sum(b.by_arrival.received for b in bins) == media.received
    assert sum(b.by_arrival.bits for b in bins) == media.received * 1200 * 8
    # Every packet arrives 20 ms after its slot: the packets of a bin that arrive later are few
    assert all(b.by_send.lost == 0 for b in bins)
    assert media.recoveries == (), "no event, nothing to recover from"


@pytest.mark.parametrize(
    "options",
    [
        {"bin": 0.0},
        {"bin": -1.0},
        {"bin": float("nan")},
        {"recovery_basis": "both"},
        {"recovery_baseline": 0},
        {"recovery_window": 0},
        {"recovery_fraction": 0.0},
        {"recovery_fraction": 1.5},
    ],
)
def test_canary_options_that_make_no_sense_are_refused_before_anything_is_built(
    options: dict[str, object],
) -> None:
    with pytest.raises(ValueError, match=r"bin|recovery"):
        ns.run(scenario(), **options)  # type: ignore[arg-type]


def test_a_bin_shorter_than_a_millisecond_is_refused_with_the_path_of_the_key(
    tmp_path: Path,
) -> None:
    with pytest.raises(ns.ScenarioError, match=r"measure\.series\.bin"):
        ns.run(scenario(), out=tmp_path / "out", bin=0.0001)


def test_the_section_measure_of_the_file_is_used_and_an_argument_replaces_it(
    tmp_path: Path,
) -> None:
    measured = (
        ns.Scenario.builder("py-measure", duration="2s")
        .nodes("client", "server")
        .link("access", "client", "server")
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .measure(bin="500ms", packets=True)
        .build()
    )
    from_file = ns.run(measured, out=tmp_path / "file", settle=0.3)
    media = from_file["media"]
    assert media.series is not None
    assert media.series.bin == pytest.approx(0.5)
    assert media.packets_file is not None
    replaced = ns.run(measured, out=tmp_path / "arg", settle=0.3, bin=1.0)
    assert replaced["media"].series is not None
    assert replaced["media"].series.bin == pytest.approx(1.0)
    # The scenario that ran is the one with the argument in it, so the hash tells the two apart
    assert from_file.scenario_hash != replaced.scenario_hash


def test_a_link_is_captured_at_both_ends_by_the_file_or_by_an_argument(tmp_path: Path) -> None:
    captured = (
        ns.Scenario.builder("py-capture", duration="1s")
        .nodes("client", "server")
        .link("access", "client", "server")
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .measure(capture=["access"])
        .build()
    )
    run = ns.run(captured, out=tmp_path / "file", settle=0.3)
    assert [(c.link, c.node) for c in run.captures] == [("access", "client"), ("access", "server")]
    for capture in run.captures:
        assert capture.file == run.out_dir / f"capture-access-{capture.node}.pcap"
        assert capture.file.is_file()
        # 209 packets of the workload and nothing else on a link without other traffic
        assert capture.packets >= run["media"].sent
        assert capture.complete
        assert capture.first is not None
        assert capture.clock_offset > 1e9, "the real-time clock, in seconds since 1970"
    # The file of the other run has no capture, and the argument alone can switch it on
    plain = ns.run(scenario(duration="1s"), out=tmp_path / "plain", settle=0.3)
    assert plain.captures == ()
    asked = ns.run(scenario(duration="1s"), out=tmp_path / "asked", settle=0.3, capture=["access"])
    assert len(asked.captures) == 2


def test_a_program_of_your_own_runs_in_the_directory_of_the_file_and_reports_its_metrics(
    tmp_path: Path,
) -> None:
    script = tmp_path / "app.sh"
    script.write_text(
        "#!/bin/sh\n"
        'echo \'{"metric":"answers","value":3,"unit":"1"}\'\n'
        'echo "here $(pwd) $NETSCENARIO_NODE"\n'
        "echo ready\n"
        "sleep 30\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    path = tmp_path / "scene.yaml"
    path.write_text(
        "version: 0\nname: py-program\nduration: 1s\n"
        "topology:\n  nodes: [client, server]\n  links:\n    - id: l\n"
        "      between: [client, server]\n"
        "workloads:\n  - id: app\n    kind: command\n    node: server\n"
        '    run: ["./app.sh"]\n    ready: { stdout-line: ready, within: 5s }\n'
        "    shutdown: { signal: TERM, within: 2s }\n",
        encoding="utf-8",
    )
    run = ns.run(ns.Scenario.load(path), out=tmp_path / "out", settle=0.2)
    program = run.programs["app"]
    assert (program.how_it_ended, program.exit_signal) == ("stopped", 15)
    assert program.metrics["answers"].last == 3.0
    stdout = program.stdout_file.read_text(encoding="utf-8")
    assert f"here {tmp_path.resolve()} server" in stdout
    assert run.workloads == {}


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


def test_a_set_of_repetitions_has_derived_seeds_and_an_interval_for_each_metric(
    tmp_path: Path,
) -> None:
    # 1 s at 2 mbit/s with 1200 byte payloads is 208 packets; at 10 % loss the lost packets of a
    # run have the mean 20.8 and the standard deviation 4.3, so the five runs are all alike with a
    # probability far below 1e-5
    lossy = scenario(duration="1s", link=ns.Impairments(delay="5ms", loss="10%"), seed=7)
    heard: list[RunEvent] = []
    runs = ns.run(lossy, repetitions=5, out=tmp_path / "set", settle=0.3, observer=heard.append)
    assert isinstance(runs, ns.RunSet)
    assert (runs.seed.value, runs.seed.drawn, runs.repetitions) == (7, False, 5)
    assert runs.out_dir == tmp_path / "set"
    assert (runs.out_dir / "runset.json").is_file()
    # Every run is a whole result in a directory of its own, with the seed the set derived for it
    results = runs.results
    assert [result.seed.value for result in results] == [run.seed for run in runs.runs]
    assert len({run.seed for run in runs.runs}) == 5
    lost = runs.metric("media.lost")
    assert lost.values == tuple(float(result["media"].lost) for result in results)
    assert len(set(lost.values)) >= 2, "five runs with different seeds lost the same number"
    assert lost.interval.low <= lost.mean <= lost.interval.high
    assert 0 < runs.metric("media.loss_ratio").ci95_high < 0.5
    # The observer heard each repetition begin, before the events of its run
    started = [e for e in heard if isinstance(e, ns.events.RepetitionStarted)]
    assert [(e.index, e.total) for e in started] == [(i, 5) for i in range(5)]
    assert isinstance(heard[0], ns.events.RepetitionStarted)
    # The same seed gives the same set: the lost packets, run by run
    again = ns.run(lossy, repetitions=5, out=tmp_path / "again", settle=0.3)
    assert again.metric("media.lost").values == lost.values
    assert [run.seed for run in again.runs] == [run.seed for run in runs.runs]


def test_a_replay_loses_the_same_packets_and_another_seed_does_not(tmp_path: Path) -> None:
    # 1 s at 2 mbit/s is 208 packets; at 10 % loss another seed loses the same packets with a
    # probability far below 1e-9
    lossy = scenario(duration="1s", link=ns.Impairments(delay="5ms", loss="10%"), seed=7)
    first = ns.run(lossy, out=tmp_path / "first", settle=0.3)
    heard: list[RunEvent] = []
    again = ns.replay(first.out_dir, out=tmp_path / "again", observer=heard.append)
    assert again.reproduced, (again.verdict, again.workloads)
    assert (again.seed, again.original_dir, again.replay_dir) == (
        7,
        first.out_dir,
        tmp_path / "again",
    )
    assert not again.other_machine
    assert again.provenance == ()
    assert again.workloads[0].identical
    replayed = again.result()
    assert replayed["media"].lost == first["media"].lost
    assert replayed.settle == pytest.approx(first.settle), (
        "the replay waits as long as the original"
    )
    assert any(isinstance(event, ns.events.Started) for event in heard)

    # The canary: the same directory with another recorded seed is a replay that differs
    path = first.out_dir / "result.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    data["seed"]["value"] = 8
    path.write_text(json.dumps(data), encoding="utf-8")
    other = ns.replay(first.out_dir, out=tmp_path / "other")
    assert other.verdict == "differs"
    assert not other.reproduced
    assert other.workloads[0].count_only_replay > 0


def test_the_reports_of_a_real_run_are_made_in_python(tmp_path: Path) -> None:
    run = ns.run(scenario(duration="1s", link=ns.Impairments(delay="5ms")), out=tmp_path / "out")
    assert run.report("md").startswith("## netscenario: py-test\n")
    junit = ns.report(run.out_dir, "junit")
    assert 'name="media"' in junit
    html = ns.report(run.out_dir, "html")
    assert "<svg " in html
    for needle in ("http://", "https://", "<script", "src=", "href=", "url("):
        assert needle not in html, needle
    assert html == run.report("html"), "the same directory gives the same bytes"
