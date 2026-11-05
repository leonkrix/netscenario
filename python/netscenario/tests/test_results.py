"""The typed result, from the text of a result.json."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

import netscenario as ns

RESULT = {
    "format": "netscenario-result",
    "version": 0,
    "scenario": {"name": "lte-like-link", "hash": "sha256:00"},
    "provenance": {
        "netscenario": "0.0.0",
        "started_at": "2026-10-30T12:27:30Z",
        "backend": "kernel",
        "backend_details": {"rights": "user namespace", "tc": "iproute2 6.15.0"},
        "kernel": "6.12.113",
        "cpu_model": None,
        "cpus": 16,
    },
    "seed": {"value": 42, "drawn": True, "loss_reproducible": True},
    "accepted_restrictions": [],
    "addresses": [{"node": "client", "link": "access", "address": "10.100.0.1"}],
    "timing": {"duration_ns": 3_000_000_000, "settle_ns": 1_120_000_000},
    "workloads": [
        {
            "id": "media",
            "kind": "udp-cbr",
            "from": "client",
            "to": "server",
            "from_address": "10.100.0.1",
            "to_address": "10.100.0.2:5000",
            "rate_bits_per_second": 2_000_000,
            "packet_size_bytes": 1200,
            "start_ns": 0,
            "duration_ns": 3_000_000_000,
            "sent": 625,
            "received": 620,
            "lost": 5,
            "loss_ratio": 0.008,
            "duplicates": 0,
            "reordered": 3,
            "unexpected": 0,
            "delay": {
                "min_ns": 31_168_696,
                "mean_ns": 42_869_547,
                "p50_ns": 42_615_470,
                "p90_ns": 48_602_048,
                "p95_ns": 50_225_591,
                "p99_ns": 54_035_378,
                "p999_ns": 56_984_904,
                "max_ns": 56_984_904,
            },
            "throughput": {"bin_ns": 1_000_000_000, "bits_per_second": [1910400.0, 76800.0]},
            "sender": {"max_lateness_ns": 189, "send_errors": 0, "first_error": None},
            "foreign_datagrams": 0,
            "records_file": "flow-0.records",
            "packets_file": None,
        }
    ],
}


def result(**changes: object) -> ns.Result:
    data = json.loads(json.dumps(RESULT))
    data.update(changes)
    return ns.Result.from_json(json.dumps(data), Path("/out"))


def test_times_are_seconds_and_rates_are_bits_per_second() -> None:
    media = result()["media"]
    assert media.delay is not None
    assert media.delay.p50 == pytest.approx(0.04261547)
    assert media.delay.median == media.delay.p50
    assert media.delay.p99 < 0.150
    assert media.rate == 2_000_000.0
    assert media.duration == 3.0
    assert media.throughput.bin == 1.0
    assert media.throughput.bits_per_second == (1910400.0, 76800.0)
    assert media.sender.max_lateness == pytest.approx(189e-9)


def test_the_counts_the_ratios_and_the_names_of_the_nodes() -> None:
    media = result()["media"]
    assert (media.sent, media.received, media.lost, media.reordered) == (625, 620, 5, 3)
    assert media.loss_ratio == pytest.approx(0.008)
    assert media.delivery_ratio == pytest.approx(620 / 625)
    assert (media.from_node, media.to_node) == ("client", "server")
    assert media.records_file == Path("/out/flow-0.records")
    assert media.packets_file is None


def test_the_run_as_a_whole() -> None:
    run = result()
    assert (run.duration, run.settle) == (3.0, pytest.approx(1.12))
    assert run.seed.value == 42
    assert run.seed.drawn
    assert run.seed.queue_checks == ()
    assert run.provenance.backend_details["tc"] == "iproute2 6.15.0"
    assert run.addresses[0].address == "10.100.0.1"
    assert len(run) == 1
    assert [w.id for w in run] == ["media"]
    assert run.raw["version"] == 0


def test_an_unknown_workload_names_the_known_ones() -> None:
    with pytest.raises(KeyError, match="the workloads are: media"):
        result()["video"]


def test_a_workload_without_arrivals_has_no_delay() -> None:
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["delay"] = None
    media = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert media.delay is None


def test_the_damaged_packets_of_a_stream_without_the_checksum_are_counted() -> None:
    # A result of this program has the key; one from before the key was added has not, and means 0
    assert result()["media"].corrupted == 0
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["corrupted"] = 7
    media = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert media.corrupted == 7
    assert media.lost == 5, "a damaged packet is not an intact one: it stays in lost"


def test_the_names_of_the_damaged_packets_are_read_and_their_absence_is_visible() -> None:
    media = result()["media"]
    assert (media.corrupted_named, media.corrupted_seq, media.corrupted_seq_unavailable) == (
        None,
        (),
        None,
    )
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0].update(corrupted_named=3, corrupted_seq=[4, 9, 17])
    named = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert (named.corrupted_named, named.corrupted_seq) == (3, (4, 9, 17))
    data["workloads"][0].update(
        corrupted_named=None, corrupted_seq=[], corrupted_seq_unavailable="CAP_NET_RAW is missing"
    )
    unavailable = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert unavailable.corrupted_named is None
    assert unavailable.corrupted_seq_unavailable == "CAP_NET_RAW is missing"


def test_the_patterns_of_the_arrivals_are_in_seconds_and_a_result_without_them_means_none() -> None:
    media = result()["media"]
    # A result from before the patterns were counted: nothing to say, and no run of lost packets
    assert (media.reordering, media.jitter) == (None, None)
    assert (media.loss_runs.runs, media.loss_runs.histogram) == (0, ())
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0].update(
        reordering={
            "extent": {"max": 4, "mean": 1.5, "p50": 1, "p95": 4, "p99": 4},
            "late_time": {
                "max_ns": 3_000_000,
                "mean_ns": 1_000_000,
                "p50_ns": 1_000_000,
                "p95_ns": 3_000_000,
                "p99_ns": 3_000_000,
            },
        },
        jitter={"last_ns": 180_000, "mean_ns": 150_000, "max_ns": 210_000},
        loss_runs={
            "runs": 3,
            "longest": 2,
            "mean": 5 / 3,
            "histogram": [{"length": 1, "runs": 2}, {"length": 2, "runs": 1}],
        },
    )
    media = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert media.reordering is not None
    assert media.jitter is not None
    assert (media.reordering.extent.max, media.reordering.extent.mean) == (4, 1.5)
    assert media.reordering.late_time.max == pytest.approx(0.003)
    assert (media.jitter.last, media.jitter.mean, media.jitter.max) == pytest.approx(
        (0.00018, 0.00015, 0.00021)
    )
    assert (media.loss_runs.runs, media.loss_runs.longest) == (3, 2)
    assert [(b.length, b.runs) for b in media.loss_runs.histogram] == [(1, 2), (2, 1)]


def test_the_steps_of_the_timeline_are_read_in_seconds_and_none_means_no_timeline() -> None:
    assert result().steps == ()
    data = json.loads(json.dumps(RESULT))
    data["events"] = [
        {
            "event": 0,
            "kind": "set",
            "link": "access",
            "direction": "forward",
            "scheduled_ns": 3_000_000_000,
            "started_ns": 3_000_100_000,
            "applied_ns": 3_001_900_000,
        },
        {
            "event": 1,
            "kind": "down",
            "link": "access",
            "direction": None,
            "scheduled_ns": 5_000_000_000,
            "started_ns": 4_999_999_000,
            "applied_ns": 4_999_999_500,
        },
    ]
    data["events"].append(
        {
            "event": 2,
            "kind": "set",
            "link": "access",
            "direction": "reverse",
            "scheduled_ns": 6_000_000_000,
            "started_ns": 5_997_000_000,
            "applied_ns": 5_999_000_000,
            "packet": 2250,
            "held_ns": 1_500_000,
        }
    )
    first, second, third = ns.Result.from_json(json.dumps(data), Path("/out")).steps
    assert (first.kind, first.link, first.direction) == ("set", "access", "forward")
    assert first.scheduled == pytest.approx(3.0)
    assert first.lateness == pytest.approx(0.0019)
    # A step is never early: a reading a little before its moment gives 0 and not a negative number
    assert second.direction is None
    assert second.lateness == 0.0
    assert (first.packet, first.held, first.at_barrier) == (None, None, False)
    # A step at a barrier is done before the packet, and may be done before its moment
    assert (third.packet, third.at_barrier) == (2250, True)
    assert third.held == pytest.approx(0.0015)
    assert third.lateness == 0.0


def test_the_packets_the_kernel_refused_are_read_as_runs() -> None:
    assert result().workloads["media"].sender.refused == ()
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["sender"]["refused"] = [[750, 1124], [1300, 1300]]
    read = ns.Result.from_json(json.dumps(data), Path("/out"))
    assert read.workloads["media"].sender.refused == ((750, 1124), (1300, 1300))


def test_an_interrupted_run_says_when_the_senders_were_stopped() -> None:
    assert result().interrupted is None
    data = json.loads(json.dumps(RESULT))
    data["timing"]["interrupted_ns"] = 2_500_000_000
    read = ns.Result.from_json(json.dumps(data), Path("/out"))
    assert read.interrupted == pytest.approx(2.5)


def test_the_notes_of_the_backend_are_read_and_none_means_the_run_went_as_usual() -> None:
    assert result().notes == ()
    data = json.loads(json.dumps(RESULT))
    data["notes"] = ["the changes were made the slow way"]
    read = ns.Result.from_json(json.dumps(data), Path("/out"))
    assert read.notes == ("the changes were made the slow way",)


def test_canary_a_file_that_is_not_a_result_or_of_another_version_is_refused() -> None:
    with pytest.raises(ValueError, match="not a netscenario result"):
        ns.Result.from_json('{"format": "something-else"}', Path())
    with pytest.raises(ValueError, match="version 1"):
        result(version=1)


def test_the_queue_checks_are_read_and_a_disagreement_is_visible() -> None:
    seed = {
        "value": 42,
        "drawn": True,
        "loss_reproducible": False,
        "queue_checks": [
            {"link": "access", "direction": "forward", "expected": 417, "counted": 418},
            {"link": "access", "direction": "reverse", "expected": 0, "counted": 0},
        ],
    }
    checked = result(seed=seed).seed
    assert not checked.loss_reproducible
    forward, reverse = checked.queue_checks
    assert (forward.link, forward.direction, forward.expected, forward.counted) == (
        "access",
        "forward",
        417,
        418,
    )
    assert not forward.agrees
    assert reverse.agrees
    assert checked.not_reproducible_because == ()
    with_reason = {**seed, "not_reproducible_because": ["the scenario corrupts packets: ..."]}
    assert result(seed=with_reason).seed.not_reproducible_because == (
        "the scenario corrupts packets: ...",
    )


def test_the_tally_of_a_direction_adds_up_and_a_missing_one_is_empty() -> None:
    assert result().directions == ()
    tally = {
        "link": "access",
        "direction": "forward",
        "sent": 625,
        "reached_node": 600,
        "delivered": 560,
        "lost_in_queue": 25,
        "dropped_corrupted": 40,
        "receive_buffer_drops": 0,
    }
    (forward,) = result(directions=[tally]).directions
    assert (forward.link, forward.direction, forward.sent) == ("access", "forward", 625)
    assert forward.lost_in_queue == 25
    assert forward.dropped_corrupted == 40
    assert forward.lost_in_queue + forward.dropped_corrupted + forward.delivered == forward.sent
    # A direction that duplicates cannot tell what the queue dropped
    duplicating = dict(tally, lost_in_queue=None)
    assert result(directions=[duplicating]).directions[0].lost_in_queue is None


def test_the_queues_of_the_run_with_their_samples() -> None:
    assert result().queues == ()
    queue = {
        "link": "bottleneck",
        "direction": "forward",
        "build": "fabric",
        "discipline": "codel",
        "kind": "codel",
        "packets": 900,
        "bytes": 1_300_000,
        "drops": 17,
        "overlimits": 0,
        "requeues": 1,
        "backlog_packets": 3,
        "backlog_bytes": 3600,
        "details": {"ecn_mark": 0, "drop_overlimit": 2},
        "samples": [
            {
                "t_ns": 1_000_000_000,
                "packets": 450,
                "drops": 5,
                "backlog_packets": 30,
                "backlog_bytes": 36_000,
            }
        ],
    }
    (stats,) = result(queues=[queue]).queues
    assert (stats.link, stats.direction, stats.discipline, stats.kind) == (
        "bottleneck",
        "forward",
        "codel",
        "codel",
    )
    assert (stats.packets, stats.drops, stats.backlog_packets) == (900, 17, 3)
    assert stats.details["drop_overlimit"] == 2
    (sample,) = stats.samples
    assert (sample.t, sample.drops, sample.backlog_bytes) == (1.0, 5, 36_000)
    # A queue without samples (a short run) and without details is still a queue
    bare = {k: v for k, v in queue.items() if k not in ("details", "samples")}
    (plain,) = result(queues=[bare]).queues
    assert plain.samples == ()
    assert dict(plain.details) == {}


def _line(index: int, sent: int, received: int, arrived: int, bits: int) -> dict[str, object]:
    delay = {"mean_ns": 25_000_000, "p50_ns": 24_000_000, "p95_ns": 30_000_000}
    delay |= {"p99_ns": 31_000_000, "max_ns": 32_000_000}
    return {
        "workload": "media",
        "bin": index,
        "start_ns": index * 500_000_000,
        "by_send": {
            "sent": sent,
            "received": received,
            "lost": sent - received,
            "delivered_ratio": None if sent == 0 else received / sent,
            "delay": delay if received else None,
        },
        "by_arrival": {"received": arrived, "bits": bits, "delay": delay if arrived else None},
    }


def test_the_series_is_read_from_its_file_in_seconds_and_a_result_without_one_has_none(
    tmp_path: Path,
) -> None:
    assert result()["media"].series is None
    assert result()["media"].recoveries == ()
    lines = [_line(0, 100, 100, 98, 98 * 9600), _line(1, 100, 0, 0, 0), _line(2, 0, 0, 2, 2 * 9600)]
    (tmp_path / "series-media.jsonl").write_text(
        "".join(json.dumps(line) + "\n" for line in lines), encoding="utf-8"
    )
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["series"] = {
        "bin_ns": 500_000_000,
        "bins": 3,
        "file": "series-media.jsonl",
    }
    series = ns.Result.from_json(json.dumps(data), tmp_path)["media"].series
    assert series is not None
    assert (series.bin, series.bins, series.file) == (0.5, 3, tmp_path / "series-media.jsonl")
    first, silent, last = series.read()
    assert (first.index, first.start, silent.start) == (0, 0.0, 0.5)
    assert (first.by_send.sent, first.by_send.received, first.by_send.lost) == (100, 100, 0)
    assert first.by_send.delivered_ratio == 1.0
    assert first.by_send.delay is not None
    assert first.by_send.delay.p50 == pytest.approx(0.024)
    assert first.by_arrival.received == 98
    assert first.by_arrival.bits_per_second(series.bin) == pytest.approx(98 * 9600 / 0.5)
    # A bin of lost packets has no delay, and a bin in which nothing was sent has no ratio
    assert (silent.by_send.lost, silent.by_send.delay, silent.by_arrival.delay) == (100, None, None)
    assert silent.by_send.delivered_ratio == 0.0
    assert (last.by_send.sent, last.by_send.delivered_ratio) == (0, None)
    assert last.by_arrival.received == 2


def test_the_recoveries_are_read_in_seconds_and_a_missing_time_says_why() -> None:
    base = {
        "event": 1,
        "kind": "outage-end",
        "link": "access",
        "direction": "forward",
        "basis": "send",
        "from_ns": 8_000_000_000,
        "disturbance_ns": 5_000_000_000,
        "baseline": 1.0,
        "level": 0.9,
        "baseline_bins": 5,
        "window_bins": 3,
        "fraction": 0.9,
        "recovered_at_ns": 10_000_000_000,
        "time_ns": 2_000_000_000,
        "reason": None,
    }
    never = base | {"recovered_at_ns": None, "time_ns": None, "reason": "never reached"}
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["recoveries"] = [base, never]
    found, missing = ns.Result.from_json(json.dumps(data), Path("/out"))["media"].recoveries
    assert (found.kind, found.direction, found.basis) == ("outage-end", "forward", "send")
    assert (found.start, found.disturbance, found.recovered_at) == (8.0, 5.0, 10.0)
    assert found.time == pytest.approx(2.0)
    assert found.recovered
    assert (found.baseline, found.level, found.window_bins) == (1.0, 0.9, 3)
    assert (missing.time, missing.recovered_at, missing.recovered) == (None, None, False)
    assert missing.reason == "never reached"


def _program() -> dict[str, Any]:
    return {
        "id": "app",
        "node": "server",
        "run": ["./app.sh", "10.100.0.2"],
        "started_ns": -500_000_000,
        "ready_ns": -400_000_000,
        "ended_ns": 3_300_000_000,
        "ended": "stopped",
        "exit_code": None,
        "exit_signal": 15,
        "stdout_file": "program-app.stdout.log",
        "stderr_file": "program-app.stderr.log",
        "output": {
            "stdout_bytes": 343,
            "stdout_dropped_bytes": 0,
            "stderr_bytes": 5,
            "stderr_dropped_bytes": 0,
            "long_lines": 0,
        },
        "lines_dropped": False,
        "metrics": [
            {"name": "tick", "unit": None, "count": 5, "first": 1, "last": 5, "min": 1, "max": 5},
            {"name": "up", "unit": "bool", "count": 1, "first": 1, "last": 1, "min": 1, "max": 1},
        ],
        "metrics_file": "metrics-app.jsonl",
        "metric_readings": 6,
        "bad_metric_lines": 1,
        "bad_metric_examples": ["`value` must be a number: {}"],
    }


def test_a_program_is_read_in_seconds_with_its_metrics_and_files(tmp_path: Path) -> None:
    assert result().programs == {}
    data = json.loads(json.dumps(RESULT))
    data["programs"] = [_program()]
    (tmp_path / "metrics-app.jsonl").write_text(
        '{"t_ns":-500000000,"metric":"up","value":1.0,"unit":"bool"}\n'
        '{"t_ns":1500000,"metric":"tick","value":2.0}\n',
        encoding="utf-8",
    )
    read = ns.Result.from_json(json.dumps(data), tmp_path)
    program = read.programs["app"]
    assert (program.node, program.run) == ("server", ("./app.sh", "10.100.0.2"))
    assert (program.started, program.ready, program.ended) == (-0.5, -0.4, 3.3)
    assert (program.how_it_ended, program.exit_code, program.exit_signal) == ("stopped", None, 15)
    assert program.stdout_file == tmp_path / "program-app.stdout.log"
    assert (program.output.stdout_bytes, program.output.long_lines) == (343, 0)
    assert program.metrics["up"].unit == "bool"
    assert (program.metrics["tick"].count, program.metrics["tick"].last) == (5, 5.0)
    assert program.bad_metric_lines == 1
    assert "must be a number" in program.bad_metric_examples[0]
    assert program.readings() == ((-0.5, "up", 1.0), (0.0015, "tick", 2.0))
    # Stopped by the run: neither a success nor a crash
    assert (program.succeeded, program.crashed) == (False, False)


def test_a_program_that_ended_by_itself_with_an_error_is_a_crash() -> None:
    data = json.loads(json.dumps(RESULT))
    crashed = _program() | {"ended": "on-its-own", "exit_code": 3, "exit_signal": None}
    fine = _program() | {"id": "ok", "ended": "on-its-own", "exit_code": 0, "exit_signal": None}
    quiet = _program()
    quiet.update(id="quiet", metrics=[], metrics_file=None)
    data["programs"] = [crashed, fine, quiet]
    programs = ns.Result.from_json(json.dumps(data), Path("/out")).programs
    assert (programs["app"].crashed, programs["app"].succeeded) == (True, False)
    assert (programs["ok"].succeeded, programs["ok"].crashed) == (True, False)
    # No file of readings: no readings, and no error
    assert programs["quiet"].readings() == ()


def _capture_entry() -> dict[str, Any]:
    return {
        "link": "access",
        "node": "client",
        "interface": "laccess",
        "file": "capture-access-client.pcap",
        "packets": 4767,
        "bytes": 5_200_000,
        "first_ns": 1_500_000,
        "last_ns": 2_999_000_000,
        "realtime_minus_monotonic_ns": 1_762_000_000_000_000_000,
        "receive_buffer_bytes": 4_194_304,
        "interface_frames": 4767,
        "missing": 0,
    }


def test_a_capture_is_read_in_seconds_with_the_path_of_its_file(tmp_path: Path) -> None:
    assert result().captures == ()
    data = json.loads(json.dumps(RESULT))
    data["captures"] = [_capture_entry()]
    capture = ns.Result.from_json(json.dumps(data), tmp_path).captures[0]
    assert (capture.link, capture.node, capture.interface) == ("access", "client", "laccess")
    assert capture.file == tmp_path / "capture-access-client.pcap"
    assert (capture.packets, capture.bytes) == (4767, 5_200_000)
    assert (capture.first, capture.last) == (0.0015, 2.999)
    assert capture.clock_offset == 1_762_000_000.0
    assert (capture.receive_buffer, capture.interface_frames) == (4_194_304, 4767)
    assert capture.complete


def test_canary_a_capture_that_lost_frames_or_could_not_check_itself_is_not_complete() -> None:
    data = json.loads(json.dumps(RESULT))
    lost = _capture_entry() | {"missing": 12, "interface_frames": 4779}
    unchecked = _capture_entry() | {"missing": None, "interface_frames": None}
    empty = _capture_entry() | {"packets": 0, "bytes": 0, "first_ns": None, "last_ns": None}
    data["captures"] = [lost, unchecked, empty]
    lost_, unchecked_, empty_ = ns.Result.from_json(json.dumps(data), Path("/out")).captures
    assert (lost_.missing, lost_.complete) == (12, False)
    assert (unchecked_.missing, unchecked_.complete) == (None, False)
    assert (empty_.first, empty_.last, empty_.complete) == (None, None, True)


def test_the_trace_of_a_trace_replay_and_the_bytes_that_arrived() -> None:
    media = result()["media"]
    assert media.trace is None
    assert media.received_bytes == 0
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["kind"] = "trace-replay"
    data["workloads"][0]["received_bytes"] = 410_000
    data["workloads"][0]["trace"] = {
        "file": "call.csv",
        "sha256": "ab" * 32,
        "packets": 625,
        "padded": 12,
        "smallest_bytes": 16,
        "largest_bytes": 1472,
        "span_ns": 12_340_000_000,
    }
    replayed = ns.Result.from_json(json.dumps(data), Path("/out"))["media"]
    assert replayed.kind == "trace-replay"
    assert replayed.received_bytes == 410_000
    assert replayed.trace is not None
    assert (replayed.trace.file, replayed.trace.packets, replayed.trace.padded) == (
        "call.csv",
        625,
        12,
    )
    assert (replayed.trace.smallest, replayed.trace.largest) == (16, 1472)
    assert replayed.trace.span == pytest.approx(12.34)
    assert replayed.trace.sha256 == "ab" * 32


def _tcp_entry() -> dict[str, Any]:
    delay = {
        "min_ns": 40_000_000,
        "mean_ns": 50_000_000,
        "p50_ns": 45_000_000,
        "p90_ns": 60_000_000,
        "p95_ns": 70_000_000,
        "p99_ns": 80_000_000,
        "p999_ns": 90_000_000,
        "max_ns": 90_000_000,
    }
    return {
        "id": "down",
        "kind": "tcp-bulk",
        "from": "client",
        "to": "server",
        "from_address": "10.200.0.1",
        "to_address": "10.200.0.2:6000",
        "congestion": "cubic",
        "connections": 2,
        "failed_connections": 1,
        "start_ns": 0,
        "window_ns": 3_000_000_000,
        "bytes_sent": 2_000_000,
        "bytes_received": 1_900_000,
        "goodput": {
            "bin_ns": 1_000_000_000,
            "bits_per_second": [8_000_000, 6_000_000],
            "mean_bits_per_second": 7_500_000,
        },
        "transfers": [
            {
                "connection": 1,
                "started_ns": 2_000_000,
                "established_ns": 50_000_000,
                "done_ns": 5_000_000_000,
                "bytes_sent": 400_000,
                "bytes_received": 0,
                "ok": False,
            },
            {
                "connection": 0,
                "started_ns": 1_000_000,
                "established_ns": 45_000_000,
                "done_ns": 2_001_000_000,
                "bytes_sent": 1_600_000,
                "bytes_received": 0,
                "ok": True,
            },
        ],
        "retransmitted_segments": 7,
        "rtt": delay,
        "requests": {
            "sent": 10,
            "answered": 9,
            "failed": 1,
            "latency": delay,
            "round_trip": None,
            "max_wait_ns": 3_000_000,
        },
        "first_error": "the server closed the connection",
        "info_error": None,
        "stopped": False,
        "client_file": "tcp-0.client.streams",
        "server_file": "tcp-0.server.streams",
        "recoveries": [
            {
                "event": 0,
                "kind": "set",
                "link": "access",
                "direction": "forward",
                "basis": "goodput",
                "from_ns": 6_000_000_000,
                "disturbance_ns": 6_000_000_000,
                "baseline": 9_500_000.0,
                "level": 8_550_000.0,
                "baseline_bins": 5,
                "window_bins": 3,
                "fraction": 0.9,
                "recovered_at_ns": 9_000_000_000,
                "time_ns": 3_000_000_000,
                "reason": None,
            }
        ],
    }


def test_a_tcp_workload_is_read_in_seconds_and_a_result_without_one_has_none(
    tmp_path: Path,
) -> None:
    plain = result()
    assert plain.tcp == {}
    assert plain.reproducibility.exact
    assert plain.reproducibility.because == ()
    data = json.loads(json.dumps(RESULT))
    data["tcp"] = [_tcp_entry()]
    data["reproducibility"] = {"class": "statistical", "because": ["feedback"]}
    read = ns.Result.from_json(json.dumps(data), tmp_path)
    assert (read.reproducibility.kind, read.reproducibility.because) == (
        "statistical",
        ("feedback",),
    )
    assert not read.reproducibility.exact
    tcp = read.tcp["down"]
    assert (tcp.kind, tcp.congestion, tcp.connections, tcp.failed_connections) == (
        "tcp-bulk",
        "cubic",
        2,
        1,
    )
    assert (tcp.start, tcp.window) == (0.0, 3.0)
    assert tcp.client_file == tmp_path / "tcp-0.client.streams"
    assert tcp.goodput is not None
    assert (tcp.goodput.bin, tcp.goodput.bits_per_second, tcp.goodput.mean) == (
        1.0,
        (8e6, 6e6),
        7.5e6,
    )
    assert tcp.rtt is not None
    assert (tcp.rtt.min, tcp.rtt.median, tcp.rtt.p99) == (0.04, 0.045, 0.08)
    assert tcp.requests is not None
    assert (tcp.requests.answered, tcp.requests.failed, tcp.requests.max_wait) == (9, 1, 0.003)
    assert tcp.requests.latency is not None
    assert tcp.requests.round_trip is None
    assert tcp.retransmitted_segments == 7
    [recovery] = tcp.recoveries
    assert (recovery.basis, recovery.baseline, recovery.time, recovery.recovered) == (
        "goodput",
        9_500_000.0,
        3.0,
        True,
    )
    # A result of an older run has none
    older = json.loads(json.dumps(data))
    del older["tcp"][0]["recoveries"]
    assert ns.Result.from_json(json.dumps(older), tmp_path).tcp["down"].recoveries == ()


def test_canary_a_tcp_workload_with_a_failed_connection_is_not_complete() -> None:
    data = json.loads(json.dumps(RESULT))
    data["tcp"] = [_tcp_entry()]
    tcp = ns.Result.from_json(json.dumps(data), Path("/out")).tcp["down"]
    assert not tcp.complete
    # The completion counts the connection that did its work, from the first connect to its end
    assert tcp.completion == pytest.approx(2.0, abs=1e-6)
    assert tcp.transfers[1].duration == pytest.approx(2.0, abs=1e-6)
    entry = _tcp_entry()
    transfers: list[dict[str, Any]] = entry["transfers"]
    fine = entry | {"failed_connections": 0, "transfers": [{**t, "ok": True} for t in transfers]}
    data["tcp"] = [fine]
    assert ns.Result.from_json(json.dumps(data), Path("/out")).tcp["down"].complete
    # No connection that did its work: no completion
    broken = entry | {"transfers": [{**t, "ok": False} for t in transfers]}
    data["tcp"] = [broken]
    assert ns.Result.from_json(json.dumps(data), Path("/out")).tcp["down"].completion is None


def test_the_echo_of_a_workload_is_read_in_seconds_and_a_workload_without_one_has_none(
    tmp_path: Path,
) -> None:
    assert result()["media"].echo is None
    data = json.loads(json.dumps(RESULT))
    data["workloads"][0]["echo"] = {
        "echoed": 620,
        "echo_errors": 2,
        "foreign_datagrams": 1,
        "records_file": "flow-0.echo.records",
        "sent": 625,
        "received": 600,
        "lost": 25,
        "loss_ratio": 0.04,
        "duplicates": 1,
        "reordered": 2,
        "unexpected": 0,
        "delay": data["workloads"][0]["delay"],
        "throughput": {"bin_ns": 1_000_000_000, "bits_per_second": [1.0]},
    }
    echo = ns.Result.from_json(json.dumps(data), tmp_path)["media"].echo
    assert echo is not None
    assert (echo.sent, echo.received, echo.lost, echo.loss_ratio) == (625, 600, 25, 0.04)
    assert (echo.echoed, echo.echo_errors, echo.foreign_datagrams) == (620, 2, 1)
    assert (echo.duplicates, echo.reordered) == (1, 2)
    assert echo.round_trip is not None
    assert (echo.round_trip.min, echo.round_trip.p50) == (0.031168696, 0.04261547)
    assert echo.records_file == tmp_path / "flow-0.echo.records"
