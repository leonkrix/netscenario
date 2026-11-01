"""The typed result, from the text of a result.json."""

from __future__ import annotations

import json
from pathlib import Path

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
