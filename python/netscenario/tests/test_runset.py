"""A set of runs: the typed object from a ``runset.json`` and the checks of ``repetitions``.

The file ``fixtures/runset.json`` is written by a test of the Rust core (``UPDATE_FIXTURE=1``),
which fails if the format changes; these tests read it, so the two sides agree on the format.
The runs in real namespaces are in ``test_run.py``.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, assert_type

import pytest
from test_results import RESULT

import netscenario as ns
from netscenario import RunSet, events

FIXTURE = Path(__file__).parent / "fixtures" / "runset.json"
"""Six runs of a scenario with the seed 7; the delay is missing in the third run."""


def load(directory: Path | None = None, **changes: Any) -> RunSet:
    raw: dict[str, Any] = json.loads(FIXTURE.read_text(encoding="utf-8"))
    raw.update(changes)
    return RunSet.from_json(json.dumps(raw), directory or Path("set"))


def test_a_set_is_read_from_its_json() -> None:
    runs = load()
    assert (runs.scenario_name, runs.repetitions, len(runs.runs)) == ("t", 6, 6)
    assert runs.seed == ns.SetSeed(value=7, drawn=False)
    assert runs.confidence.statistic == "mean"
    assert runs.confidence.level == pytest.approx(0.95)
    assert runs.confidence.method == "expanded-percentile"
    assert runs.scenario_hash.startswith("sha256:")
    assert [run.index for run in runs.runs] == [0, 1, 2, 3, 4, 5]
    assert runs.runs[0].directory == Path("set") / "run-001"
    assert all(run.loss_reproducible for run in runs.runs)
    assert len({run.seed for run in runs.runs}) == 6, "every run has a seed of its own"


def test_a_metric_has_the_values_of_the_runs_and_an_interval_around_their_mean() -> None:
    loss = load().metric("w.loss_ratio")
    # The runs lost 2, 4, ... 12 %: mean 7 %, median 7 %; the squares of the deviations add up to
    # 0.007, so the sample variance is 0.007 / 5 = 0.0014 and the deviation 0.0374165739
    assert loss.values == pytest.approx((0.02, 0.04, 0.06, 0.08, 0.10, 0.12))
    assert loss.mean == pytest.approx(0.07)
    assert loss.median == pytest.approx(0.07)
    assert (loss.min, loss.max) == (pytest.approx(0.02), pytest.approx(0.12))
    assert loss.stddev == pytest.approx(0.0374165739, rel=1e-6)
    interval = loss.interval
    assert interval.statistic == "mean"
    assert interval.estimate == pytest.approx(loss.mean)
    assert interval.low < loss.mean < interval.high
    assert loss.min <= interval.low
    assert interval.high <= loss.max
    assert (loss.low, loss.high) == (interval.low, interval.high)
    assert interval.width == pytest.approx(interval.high - interval.low)
    assert interval.runs == 6
    assert 0 < interval.tail < 0.025, "the expanded interval leaves less out than the plain one"


def test_the_short_names_of_the_95_percent_interval_are_the_ends() -> None:
    loss = load().metric("w.loss_ratio")
    assert (loss.ci95_low, loss.ci95_high) == (loss.interval.low, loss.interval.high)


def test_ci95_refuses_a_set_that_was_made_with_another_level() -> None:
    raw: dict[str, Any] = json.loads(FIXTURE.read_text(encoding="utf-8"))
    for metric in raw["metrics"]:
        metric["interval"]["level"] = 0.9
    loss = RunSet.from_json(json.dumps(raw), Path("set")).metric("w.loss_ratio")
    assert loss.low == loss.interval.low, "low and high are for the level of the set"
    with pytest.raises(ValueError, match=r"level 0\.9, not 0\.95"):
        _ = loss.ci95_low
    with pytest.raises(ValueError, match=r"use \.low and \.high"):
        _ = loss.ci95_high


def test_a_metric_that_some_runs_lack_has_no_interval_and_says_why() -> None:
    runs = load()
    assert "w.delay.p99" not in runs.metrics
    assert {p.name for p in runs.partial} >= {"w.delay.p99", "w.delay.min"}
    p99 = next(p for p in runs.partial if p.name == "w.delay.p99")
    assert (p99.present_in, p99.of) == (5, 6)
    with pytest.raises(KeyError, match=r"was there in 5 of 6 runs"):
        runs.metric("w.delay.p99")


def test_an_unknown_metric_names_the_ones_that_look_like_it() -> None:
    with pytest.raises(KeyError, match=r"has no metric 'w.loss_ratoi'.*did you mean w.loss_ratio"):
        load().metric("w.loss_ratoi")
    with pytest.raises(KeyError, match="has no metric 'nothing'"):
        load().metric("nothing")


def test_the_metrics_are_sorted_and_iterable() -> None:
    runs = load()
    names = [metric.name for metric in runs]
    assert names == sorted(names)
    assert len(runs) == len(names) == len(runs.metrics)
    assert {"w.sent", "w.loss_ratio", "w.throughput.mean"} <= set(names)


def test_a_metric_that_is_the_same_in_every_run_has_a_point_as_its_interval() -> None:
    sent = load().metric("w.sent")
    assert sent.stddev == 0
    assert (sent.interval.low, sent.interval.high) == (sent.mean, sent.mean)


def test_the_results_of_the_runs_are_read_from_their_directories(tmp_path: Path) -> None:
    result_text = json.dumps(RESULT)
    for index in range(1, 7):
        directory = tmp_path / f"run-{index:03}"
        directory.mkdir()
        (directory / "result.json").write_text(result_text, encoding="utf-8")
    results = load(tmp_path).results
    assert len(results) == 6
    assert all(isinstance(result, ns.Result) for result in results)
    assert results[2].out_dir == tmp_path / "run-003"


def test_what_is_not_a_set_of_this_version_is_refused() -> None:
    raw: dict[str, Any] = json.loads(FIXTURE.read_text(encoding="utf-8"))
    with pytest.raises(ValueError, match="not a netscenario set of runs"):
        RunSet.from_json(json.dumps({**raw, "format": "netscenario-result"}), "set")
    with pytest.raises(ValueError, match="version 7; this package reads version 0"):
        RunSet.from_json(json.dumps({**raw, "version": 7}), "set")


def test_the_event_that_starts_a_repetition_is_made() -> None:
    event = events.make_event("repetition_started", {"index": 2.0, "total": 10.0})
    assert event == events.RepetitionStarted(index=2, total=10)


def test_a_run_with_repetitions_is_typed_as_a_set_and_one_without_as_a_result() -> None:
    def typed(scenario: ns.Scenario) -> None:
        assert_type(ns.run(scenario), ns.Result)
        assert_type(ns.run(scenario, repetitions=5), RunSet)
        assert_type(ns.run(scenario, repetitions=5, statistic="median", confidence=0.9), RunSet)

    assert callable(typed), "pyright checks the types of the function above"


@pytest.mark.parametrize(
    ("options", "message"),
    [
        ({"repetitions": 4}, "repetitions is 4; it is from 5"),
        ({"repetitions": 1001}, "repetitions is 1001"),
        ({"repetitions": 5, "confidence": 1.0}, "level of an interval"),
        ({"repetitions": 5, "confidence": 0.0}, "level of an interval"),
        ({"repetitions": 5, "statistic": "mode"}, "statistic: the statistic is mean or median"),
        ({"statistic": "mean"}, "statistic and confidence need repetitions"),
        ({"confidence": 0.9}, "statistic and confidence need repetitions"),
    ],
)
def test_wrong_options_of_repetitions_are_refused_before_anything_runs(
    options: dict[str, Any], message: str, repository: Path
) -> None:
    scenario = ns.Scenario.load(repository / "scenarios" / "lte-like.yaml")
    with pytest.raises(ValueError, match=message):
        ns.run(scenario, **options)
