"""The ``expect`` section of a scenario file (ASR-07, D-094) through the Python package.

The verdict is judged by the core and written into ``result.json`` and ``runset.json``; these
tests read it from JSON (no namespaces needed) and then run a file in real namespaces.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from test_results import RESULT
from test_runset import load as load_set

import netscenario as ns

VERDICT: dict[str, Any] = {
    "passed": False,
    "runs": 1,
    "checks": [
        {
            "name": "loss stays small",
            "metric": "w.loss_ratio",
            "statistic": "value",
            "unit": "ratio",
            "bound": "< 5%",
            "threshold": 0.05,
            "actual": 0.012,
            "margin": 0.038,
            "passed": True,
        },
        {
            "name": "the delay",
            "metric": "w.delay.p99",
            "statistic": "value",
            "unit": "seconds",
            "bound": "<= 150ms",
            "threshold": 0.15,
            "actual": 0.2,
            "margin": -0.05,
            "passed": False,
        },
        {
            "name": "a metric that is not there",
            "metric": "w.jitter.max",
            "statistic": "value",
            "unit": "seconds",
            "bound": "< 1s",
            "threshold": 1.0,
            "passed": False,
            "note": "the run has no 'w.jitter.max'",
        },
    ],
}


def result_with(verdict: dict[str, Any] | None) -> ns.Result:
    raw = json.loads(json.dumps(RESULT))
    if verdict is not None:
        raw["expect"] = verdict
    return ns.Result.from_json(json.dumps(raw), Path("out"))


def test_a_result_without_checks_has_no_verdict() -> None:
    assert result_with(None).expect is None


def test_the_verdict_of_a_run_is_read_from_its_json() -> None:
    expect = result_with(VERDICT).expect
    assert expect is not None
    assert (expect.passed, expect.runs, len(expect.checks)) == (False, 1, 3)
    held, missed, missing = expect.checks
    assert held.passed
    assert held.margin == pytest.approx(0.038)
    assert (missed.statistic, missed.unit, missed.bound) == ("value", "seconds", "<= 150ms")
    assert missed.actual == pytest.approx(0.2)
    assert missed.margin == pytest.approx(-0.05)
    assert missing.actual is None
    assert missing.margin is None
    assert missing.note == "the run has no 'w.jitter.max'"
    assert expect.failures == (missed, missing)


def test_a_check_says_what_was_found_and_by_how_much() -> None:
    held, missed, missing = result_with(VERDICT).expect.checks  # type: ignore[union-attr]
    assert str(held) == "loss stays small: 0.012 (< 5%, 0.038 to spare)"
    assert str(missed) == "the delay: 0.2 s (<= 150ms, misses by 0.05 s)"
    assert str(missing) == "a metric that is not there: < 1s (the run has no 'w.jitter.max')"


def test_a_verdict_that_does_not_hold_raises_with_the_checks_that_do_not() -> None:
    expect = result_with(VERDICT).expect
    assert expect is not None
    with pytest.raises(AssertionError) as raised:
        expect.raise_for_failures()
    message = str(raised.value)
    assert message.startswith("1 of 3 checks of the scenario file hold:")
    assert "the delay: 0.2 s" in message
    assert "a metric that is not there" in message
    assert "loss stays small" not in message
    # Canary: a verdict that holds does not raise
    holding = {**VERDICT, "passed": True, "checks": VERDICT["checks"][:1]}
    ok = result_with(holding).expect
    assert ok is not None
    assert ok.failures == ()
    ok.raise_for_failures()


def test_the_verdict_of_a_set_is_read_from_its_json() -> None:
    runs = load_set(expect={**VERDICT, "runs": 6})
    assert runs.expect is not None
    assert runs.expect.runs == 6
    assert load_set().expect is None


def test_the_repetitions_the_checks_need_are_known_before_the_run() -> None:
    text = (
        "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n"
        "    - { id: l, between: [a, b] }\nworkloads:\n"
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n"
    )
    assert ns.Scenario.from_yaml(text).expect_repetitions is None
    one = text + 'expect:\n  checks:\n    - { metric: w.loss_ratio, value: "< 1%" }\n'
    assert ns.Scenario.from_yaml(one).expect_repetitions is None
    many = (
        text
        + "expect:\n  repetitions: 7\n  checks:\n"
        + '    - { metric: w.loss_ratio, ci95-high: "< 1%" }\n'
    )
    assert ns.Scenario.from_yaml(many).expect_repetitions == 7


def test_a_check_that_cannot_be_judged_is_refused_when_the_file_is_read() -> None:
    text = (
        "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n"
        "    - { id: l, between: [a, b] }\nworkloads:\n"
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n"
        'expect:\n  checks:\n    - { metric: w.delay.p98x, value: "< 1s" }\n'
    )
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.from_yaml(text)
    assert "w.delay.p98x" in str(raised.value) or "delay.p98x" in str(raised.value)


# ---- runs in real namespaces ----

PLAIN = """\
version: 0
name: judged
duration: 2s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }
expect:
  checks:
    - { name: nothing is lost, metric: w.loss_ratio, value: "<= 0%" }
    - { name: a false claim, metric: w.delay.p99, value: "< 1ns" }
"""


@pytest.mark.usefixtures("runnable")
def test_a_run_is_judged_and_the_verdict_is_in_the_result(tmp_path: Path) -> None:
    result = ns.run(ns.Scenario.from_yaml(PLAIN), out=tmp_path / "out")
    assert isinstance(result, ns.Result)
    assert result.expect is not None
    by_name = {check.name: check for check in result.expect.checks}
    assert by_name["nothing is lost"].passed
    assert not by_name["a false claim"].passed, "a claim that is false fails"
    assert by_name["a false claim"].margin is not None
    assert by_name["a false claim"].margin < 0
    assert not result.expect.passed
    # The same verdict is in the file, which is what the reports read
    written = json.loads((tmp_path / "out" / "result.json").read_text(encoding="utf-8"))
    assert written["expect"]["passed"] is False
    assert [c["name"] for c in written["expect"]["checks"]] == list(by_name)
    # The call does not raise; the verdict does
    with pytest.raises(AssertionError, match="a false claim"):
        result.expect.raise_for_failures()
    # A claim that is true passes, for the same run (the canary of the false one)
    fixed = PLAIN.replace('value: "< 1ns"', 'value: "< 1s"')
    again = ns.run(ns.Scenario.from_yaml(fixed), out=tmp_path / "again")
    assert isinstance(again, ns.Result)
    assert again.expect is not None
    assert again.expect.passed
    again.expect.raise_for_failures()
    report = ns.report(tmp_path / "out", "md")
    assert "Checks of the file: 1 of 2 hold" in report
    assert "**fails**" in report


SET = """\
version: 0
name: judged-set
duration: 1s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }
expect:
  repetitions: 5
  checks:
    - { metric: w.loss_ratio, ci95-high: "< 1%" }
"""


@pytest.mark.usefixtures("runnable")
def test_a_file_whose_checks_need_a_set_is_run_as_a_set(tmp_path: Path) -> None:
    runs = ns.run(ns.Scenario.from_yaml(SET), out=tmp_path / "set")
    assert isinstance(runs, ns.RunSet)
    assert runs.repetitions == 5
    assert runs.expect is not None
    assert runs.expect.passed
    assert runs.expect.runs == 5
    assert runs.expect.checks[0].statistic == "ci95-high"
    # More runs are allowed, fewer are not
    more = ns.run(ns.Scenario.from_yaml(SET), repetitions=6, out=tmp_path / "more")
    assert isinstance(more, ns.RunSet)
    assert more.repetitions == 6
    needs_six = SET.replace("repetitions: 5", "repetitions: 6")
    with pytest.raises(ns.RunError, match="at least 6 runs"):
        ns.run(ns.Scenario.from_yaml(needs_six), repetitions=5, out=tmp_path / "fewer")
