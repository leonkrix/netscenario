"""``report`` and ``junit_properties``: the reports of a directory, from Python (CLI-02, RPT-02).

The formats themselves are tested with snapshots in the Rust core. These tests check what the
Python layer adds: that the call reaches the native library with the format, that a wrong input is
a ``ValueError``, that a result and a set know their directory, and that the properties of the
pytest plugin are those of the report.
"""

from __future__ import annotations

import json
import xml.etree.ElementTree as ET
from pathlib import Path

import pytest
from test_results import RESULT

import netscenario as ns
from netscenario import RunSet
from pytest_netscenario import runner

SET_FIXTURE = Path(__file__).parent / "fixtures" / "runset.json"


def a_result(directory: Path) -> ns.Result:
    """The result of the tests of the typed result, written to a directory as `result.json`."""
    directory.mkdir(parents=True, exist_ok=True)
    text = json.dumps(RESULT)
    (directory / "result.json").write_text(text, encoding="utf-8")
    return ns.Result.from_json(text, directory)


def a_set(directory: Path) -> RunSet:
    directory.mkdir(parents=True, exist_ok=True)
    text = SET_FIXTURE.read_text(encoding="utf-8")
    (directory / "runset.json").write_text(text, encoding="utf-8")
    return RunSet.from_json(text, directory)


def properties_of(xml: str) -> dict[str, str]:
    tree = ET.fromstring(xml)  # noqa: S314 - the text was just made by this package
    found: dict[str, str] = {}
    for element in tree.iter("property"):
        name, value = element.get("name"), element.get("value")
        if name is not None and value is not None:
            found[name] = value
    return found


def test_the_markdown_of_a_run_is_the_summary_for_a_ci_job(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    text = ns.report(result.out_dir)
    assert text.startswith("## netscenario: lte-like-link\n")
    assert "| Sent | 625 |" in text
    assert text == ns.report(result.out_dir, "md"), "md is the default"
    assert text == result.report("md"), "a result knows its directory"


def test_the_junit_of_a_run_has_a_case_for_each_workload(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    xml = ns.report(result.out_dir, "junit")
    case = ET.fromstring(xml).find(".//testcase")  # noqa: S314 - the text was just made by this package
    assert case is not None
    assert (case.get("classname"), case.get("name")) == ("lte-like-link", "media")
    assert properties_of(xml)["netscenario.lte-like-link.media.sent"] == "625"
    assert not xml.count("<failure"), "a result that went through has no failed case"


def test_the_report_of_a_set_has_the_intervals(tmp_path: Path) -> None:
    runs = a_set(tmp_path / "set")
    text = runs.report()
    assert text.startswith("## netscenario: t, 6 repetitions\n")
    assert "### Headline metrics" in text
    assert ns.report(runs.out_dir, "junit") == runs.report("junit")


def test_html_needs_the_files_of_the_run_and_names_the_missing_one(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    with pytest.raises(ValueError, match=r"flow-0\.records"):
        ns.report(result.out_dir, "html")


def test_a_wrong_format_or_directory_is_a_value_error(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    with pytest.raises(ValueError, match="the format is md, html or junit, not 'pdf'"):
        ns.report(result.out_dir, "pdf")  # type: ignore[arg-type]  # the point of the test
    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(ValueError, match=r"has neither result\.json nor runset\.json"):
        ns.report(empty)
    with pytest.raises(ValueError, match=r"has neither result\.json nor runset\.json"):
        ns.report(tmp_path / "nothing-here")


def test_the_properties_of_the_plugin_are_those_of_the_junit_report(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    from_report = properties_of(ns.report(result.out_dir, "junit"))
    from_plugin = {name: str(value) for name, value in runner.properties(result)}
    # Everything the plugin writes is in the report, with the same value; the report adds only the
    # properties of the suite (version, backend, seed)
    assert from_plugin.items() <= from_report.items()
    assert {n for n in from_report if n not in from_plugin} == {
        "netscenario.version",
        "netscenario.backend",
        "netscenario.scenario_hash",
        "netscenario.loss_reproducible",
        "netscenario.seed",
    }
    # And the names that the plugin wrote before this list came from the library are all still there
    for name in (
        "sent",
        "received",
        "lost",
        "loss_ratio",
        "reordered",
        "delay_p50_s",
        "delay_p99_s",
    ):
        assert f"netscenario.lte-like-link.media.{name}" in from_plugin


def test_the_properties_of_a_set_are_those_of_its_report(tmp_path: Path) -> None:
    runs = a_set(tmp_path / "set")
    from_report = properties_of(runs.report("junit"))
    from_function = dict(ns.junit_properties(runs))
    assert from_function.items() <= from_report.items()
    assert from_function["netscenario.t.repetitions"] == "6"
    assert "netscenario.t.w.loss_ratio.high" in from_function


def test_canary_a_property_that_the_run_does_not_have_is_not_made_up(tmp_path: Path) -> None:
    result = a_result(tmp_path / "run")
    names = {name for name, _ in ns.junit_properties(result)}
    assert not any(".delay_p999_s" in name or ".jitter_p99_s" in name for name in names)
    assert "netscenario.lte-like-link.media.delay_p50_s" in names


# ---- the report section of the plugin for a run with a timeline (needs no machine) ----


def a_result_with_a_timeline() -> ns.Result:
    """The result of the typed result with two events and two times to recover."""

    from test_results import RESULT  # the sibling test module of the package

    data = json.loads(json.dumps(RESULT))
    data["events"] = [
        {
            "event": 0,
            "kind": "set",
            "link": "access",
            "direction": "forward",
            "scheduled_ns": 4_000_000_000,
            "started_ns": 4_000_000_000,
            "applied_ns": 4_001_800_000,
        },
        {
            "event": 1,
            "kind": "down",
            "link": "access",
            "direction": None,
            "scheduled_ns": 8_000_000_000,
            "started_ns": 7_990_000_000,
            "applied_ns": 7_991_000_000,
            "packet": 1664,
            "held_ns": 0,
        },
    ]
    data["workloads"][0]["recoveries"] = [
        {
            "event": 0,
            "kind": "set",
            "link": "access",
            "direction": "forward",
            "basis": "send",
            "from_ns": 4_000_000_000,
            "disturbance_ns": 4_000_000_000,
            "baseline": 1.0,
            "level": 0.9,
            "baseline_bins": 5,
            "window_bins": 3,
            "fraction": 0.9,
            "recovered_at_ns": 6_000_000_000,
            "time_ns": 2_000_000_000,
            "reason": None,
        },
        {
            "event": 1,
            "kind": "down",
            "link": "access",
            "direction": None,
            "basis": "send",
            "from_ns": 8_000_000_000,
            "disturbance_ns": 8_000_000_000,
            "baseline": 1.0,
            "level": 0.9,
            "baseline_bins": 5,
            "window_bins": 3,
            "fraction": 0.9,
            "recovered_at_ns": None,
            "time_ns": None,
            "reason": "the level was not reached before the run ended",
        },
    ]
    return ns.Result.from_json(json.dumps(data), Path("out"))


def test_the_report_of_a_run_names_the_events_and_the_recoveries() -> None:
    lines = runner.describe(a_result_with_a_timeline()).splitlines()
    assert "  timeline:" in lines
    assert "    event 0 at 4 s: link access forward set (1.8 ms late)" in lines
    assert "    event 1 at 8 s: link access both ways down (before packet 1664)" in lines
    assert "    media after event 0 (set): recovered in 2 s" in lines
    assert (
        "    media after event 1 (down): not recovered (the level was not reached before the "
        "run ended)" in lines
    )
    # The timeline comes after the workloads and before the path of the result
    assert lines.index("  timeline:") > next(i for i, line in enumerate(lines) if "media" in line)
    assert lines[-1].startswith("  result: ")


def test_a_run_without_events_has_no_timeline_section() -> None:
    text = runner.describe(ns.Result.from_json(json.dumps(RESULT), Path("out")))
    assert "timeline" not in text
