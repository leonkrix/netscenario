"""The plugin, tested by running pytest inside pytest (``pytester``)."""

from __future__ import annotations

import importlib.metadata
import xml.etree.ElementTree as ET
from pathlib import Path

import pytest

import netscenario as ns
from pytest_netscenario import runner

SCENARIO = """\
version: 0
name: plug
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


def failing_doctor(calls: list[int] | None = None) -> ns.DoctorReport:
    """A report of a machine that cannot make network namespaces."""
    if calls is not None:
        calls.append(1)
    return ns.DoctorReport(
        ok=False,
        checks=(
            ns.Check(
                section="rights",
                id="can-create-namespaces",
                status="fail",
                title="network namespaces",
                detail="no way works on this machine",
                fix="run netscenario as root",
            ),
        ),
    )


# ---- the fixture and the entry point ----


def test_the_fixture_is_found_through_the_entry_point(pytester: pytest.Pytester) -> None:
    pytester.makepyfile(
        """
        def test_it(netscenario):
            assert callable(netscenario.run) and callable(netscenario.load)
            assert netscenario.results == ()
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


def test_canary_a_failing_test_is_reported_as_failed(pytester: pytest.Pytester) -> None:
    # The check above must be able to fail
    pytester.makepyfile(
        """
        def test_it(netscenario):
            assert netscenario.results == ("something",)
        """
    )
    pytester.runpytest().assert_outcomes(failed=1)


def test_the_header_names_the_version(pytester: pytest.Pytester) -> None:
    result = pytester.runpytest()
    result.stdout.fnmatch_lines([f"netscenario: {ns.__version__}"])


def test_plugin_and_bindings_have_the_same_version() -> None:
    # They are released together (D-004)
    assert importlib.metadata.version("pytest-netscenario") == importlib.metadata.version(
        "netscenario"
    )


# ---- finding scenario files ----


def test_a_scenario_file_next_to_the_test_is_read_without_a_machine_that_can_run_it(
    pytester: pytest.Pytester, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(runner, "doctor", failing_doctor)
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            scenario = netscenario.load("plug.yaml")
            assert scenario.name == "plug"
            assert "fixed-delay" in scenario.needs()
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


def test_the_ini_option_names_more_directories(pytester: pytest.Pytester) -> None:
    pytester.mkdir("scenarios")
    (pytester.path / "scenarios" / "plug.yaml").write_text(SCENARIO, encoding="utf-8")
    pytester.makeini("[pytest]\nnetscenario_scenarios = scenarios\n")
    pytester.makepyfile(
        """
        def test_it(netscenario):
            assert netscenario.load("plug.yaml").name == "plug"
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


def test_a_missing_file_lists_the_places_that_were_looked_in(pytester: pytest.Pytester) -> None:
    pytester.makepyfile(
        """
        import pytest

        def test_it(netscenario):
            with pytest.raises(FileNotFoundError) as raised:
                netscenario.load("nothing.yaml")
            assert "'nothing.yaml' was not found in:" in str(raised.value)
            assert str(__import__("pathlib").Path(__file__).parent) in str(raised.value)
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


def test_a_faulty_scenario_file_is_a_scenario_error_with_its_lines(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", bad="version: 0\nname: bad\n")
    pytester.makepyfile(
        """
        import netscenario as ns
        import pytest

        def test_it(netscenario):
            with pytest.raises(ns.ScenarioError):
                netscenario.load("bad.yaml")
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


# ---- the guard ----


GUARD_TESTS = """
def test_a(netscenario):
    netscenario.run("plug.yaml")

def test_b(netscenario):
    netscenario.run("plug.yaml")

def test_c(netscenario):
    netscenario.load("plug.yaml")
"""


@pytest.mark.parametrize(
    ("ci", "option", "outcome"),
    [
        (False, None, "skipped"),
        (True, None, "failed"),
        (False, "fail", "failed"),
        (True, "skip", "skipped"),
    ],
)
def test_a_machine_that_cannot_run_scenarios_skips_locally_and_fails_on_the_ci(
    pytester: pytest.Pytester,
    monkeypatch: pytest.MonkeyPatch,
    ci: bool,
    option: str | None,
    outcome: str,
) -> None:
    calls: list[int] = []
    monkeypatch.setattr(runner, "doctor", lambda: failing_doctor(calls))
    if ci:
        monkeypatch.setenv("CI", "true")
    else:
        monkeypatch.delenv("CI", raising=False)
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(GUARD_TESTS)
    arguments = [f"--netscenario-environment={option}"] if option else []
    result = pytester.runpytest("-rs", *arguments)
    # Two tests run a scenario, and the one that only reads it is not affected
    result.assert_outcomes(passed=1, **{outcome: 2})
    # The machine was asked once for the whole session, and the reason says how to fix it
    assert len(calls) == 1
    result.stdout.fnmatch_lines(
        ["*this machine cannot run scenarios*", "*fix: run netscenario as root*"]
    )


# ---- real runs ----


@pytest.mark.usefixtures("runnable")
def test_a_scenario_file_is_run_in_the_temporary_directory_of_the_test(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario, tmp_path):
            result = netscenario.run("plug.yaml", settle=0.2)
            media = result["media"]
            assert (media.sent, media.lost) == (209, 0)
            assert media.delay is not None and 0.020 <= media.delay.p50 < 0.022
            assert tmp_path in result.out_dir.parents
            assert netscenario.results == (result,)
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


@pytest.mark.usefixtures("runnable")
def test_the_options_of_the_series_reach_the_run(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            result = netscenario.run("plug.yaml", settle=0.2, bin=0.5)
            series = result["media"].series
            assert series is not None and series.bin == 0.5
            assert sum(b.by_send.sent for b in series.read()) == result["media"].sent
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


@pytest.mark.usefixtures("runnable")
def test_the_numbers_of_a_run_are_in_the_report_of_a_failing_test(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            result = netscenario.run("plug.yaml", settle=0.2)
            assert result["media"].lost == 999
        """
    )
    result = pytester.runpytest()
    result.assert_outcomes(failed=1)
    result.stdout.fnmatch_lines(
        [
            "*- netscenario -*",
            "plug: kernel (*), seed * (drawn)",
            "  media client -> server: sent 209, received 209, lost 0.00%, reordered 0, "
            "delay p50 20.*",
            "  result: *result.json",
        ]
    )


@pytest.mark.usefixtures("runnable")
def test_the_numbers_of_a_run_are_properties_of_the_test_in_a_junit_file(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            netscenario.run("plug.yaml", settle=0.2)
        """
    )
    pytester.runpytest("--junitxml=junit.xml").assert_outcomes(passed=1)
    tree = ET.parse(pytester.path / "junit.xml")  # noqa: S314 - the file was just written by pytest
    properties: dict[str, str] = {}
    for element in tree.iter("property"):
        name, value = element.get("name"), element.get("value")
        if name is not None and value is not None:
            properties[name] = value
    assert properties["netscenario.plug.media.sent"] == "209"
    assert properties["netscenario.plug.media.lost"] == "0"
    assert float(properties["netscenario.plug.media.loss_ratio"]) == 0.0
    assert 0.020 <= float(properties["netscenario.plug.media.delay_p50_s"]) < 0.022


@pytest.mark.usefixtures("runnable")
def test_the_marker_runs_the_scenario_and_gives_the_result(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("plug.yaml", seed=7, settle=0.2)
        def test_it(netscenario_result):
            assert netscenario_result.seed.value == 7
            assert not netscenario_result.seed.drawn
            assert netscenario_result["media"].sent == 209
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


@pytest.mark.usefixtures("runnable")
def test_the_capture_of_a_link_is_asked_for_by_the_marker_and_by_the_call(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("plug.yaml", settle=0.2, capture=["access"])
        def test_marker(netscenario_result):
            files = sorted(c.file.name for c in netscenario_result.captures)
            assert files == ["capture-access-client.pcap", "capture-access-server.pcap"]
            assert all(c.file.is_file() and c.complete for c in netscenario_result.captures)

        def test_call(netscenario):
            result = netscenario.run("plug.yaml", settle=0.2, capture=["access"])
            assert len(result.captures) == 2
        """
    )
    pytester.runpytest().assert_outcomes(passed=2)


@pytest.mark.usefixtures("runnable")
def test_the_marker_takes_the_options_of_the_series_and_refuses_the_others(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("plug.yaml", settle=0.2, bin=0.5, recovery_basis="arrival")
        def test_it(netscenario_result):
            assert netscenario_result["media"].series.bin == 0.5

        @pytest.mark.netscenario("plug.yaml", out="x")
        def test_refused(netscenario_result):
            pass
        """
    )
    outcome = pytester.runpytest()
    outcome.assert_outcomes(passed=1, errors=1)
    outcome.stdout.fnmatch_lines(["*does not know out*"])


@pytest.mark.usefixtures("runnable")
def test_two_runs_of_a_test_go_into_two_directories(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            first = netscenario.run("plug.yaml", settle=0.2)
            second = netscenario.run("plug.yaml", settle=0.2, seed=first.seed.value)
            assert first.out_dir != second.out_dir
            assert len(netscenario.results) == 2
        """
    )
    result = pytester.runpytest("-rA")
    result.assert_outcomes(passed=1)
    # Both runs are in the report
    assert result.stdout.str().count("plug: kernel") == 2


# ---- the marker is checked ----


@pytest.mark.parametrize(
    ("marker", "message"),
    [
        ("", "needs the marker"),
        ("@pytest.mark.netscenario", "one argument"),
        ('@pytest.mark.netscenario("a.yaml", "b.yaml")', "one argument"),
        ('@pytest.mark.netscenario("plug.yaml", sead=1)', "does not know sead"),
        (
            '@pytest.mark.netscenario("plug.yaml", repetitions=5)',
            "use the fixture netscenario_runs",
        ),
    ],
)
def test_a_wrong_marker_says_what_is_wrong(
    pytester: pytest.Pytester, marker: str, message: str
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        f"""
        import pytest

        {marker}
        def test_it(netscenario_result):
            pass
        """
    )
    result = pytester.runpytest()
    # A fixture that cannot be made is an error of the test, not a failure of it
    result.assert_outcomes(errors=1)
    result.stdout.fnmatch_lines([f"*{message}*"])


def test_the_fixture_of_a_set_needs_repetitions_in_the_marker(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("plug.yaml", seed=1)
        def test_it(netscenario_runs):
            pass
        """
    )
    result = pytester.runpytest()
    result.assert_outcomes(errors=1)
    result.stdout.fnmatch_lines(["*needs repetitions in the marker*"])


def test_the_marker_takes_repetitions_and_the_options_of_the_interval(
    pytester: pytest.Pytester,
) -> None:
    result = pytester.runpytest("--markers")
    result.stdout.fnmatch_lines(["*repetitions=None, statistic=None, confidence=None*"])


@pytest.mark.usefixtures("runnable")
def test_a_run_with_too_few_repetitions_is_a_value_error(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        def test_it(netscenario):
            with pytest.raises(ValueError, match="from 5"):
                netscenario.run("plug.yaml", repetitions=4)
            with pytest.raises(ValueError, match="need repetitions"):
                netscenario.run("plug.yaml", statistic="median")
        """
    )
    # The checks are made before a network is built; the guard of the environment comes first
    pytester.runpytest().assert_outcomes(passed=1)


def test_the_marker_is_registered(pytester: pytest.Pytester) -> None:
    result = pytester.runpytest("--markers")
    result.stdout.fnmatch_lines(["@pytest.mark.netscenario(scenario, *"])


def test_find_scenario_file_takes_the_first_directory_that_has_it(tmp_path: Path) -> None:
    first, second = tmp_path / "a", tmp_path / "b"
    first.mkdir()
    second.mkdir()
    (second / "s.yaml").write_text("x", encoding="utf-8")
    (first / "s.yaml").write_text("y", encoding="utf-8")
    assert runner.find_scenario_file("s.yaml", [first, second]) == first / "s.yaml"
    assert runner.find_scenario_file("s.yaml", [tmp_path, second]) == second / "s.yaml"
    absolute = second / "s.yaml"
    assert runner.find_scenario_file(absolute, [first]) == absolute
    with pytest.raises(FileNotFoundError, match="does not exist"):
        runner.find_scenario_file(tmp_path / "nothing.yaml", [first])


# ---- sets of runs (ASR-02, ASR-03) ----

SET_FIXTURE = (
    Path(__file__).resolve().parents[2] / "netscenario" / "tests" / "fixtures" / "runset.json"
)


def a_set() -> ns.RunSet:
    return ns.RunSet.from_json(SET_FIXTURE.read_text(encoding="utf-8"), Path("out"))


def test_the_report_of_a_set_shows_the_headline_metrics_with_their_intervals() -> None:
    text = runner.describe_set(a_set())
    lines = text.splitlines()
    assert lines[0] == ("t: 6 repetitions, seed 7; interval of the mean, 95%, expanded-percentile")
    loss = next(line for line in lines if line.startswith("  w.loss_ratio:"))
    assert loss.startswith("  w.loss_ratio: 7.00% [")
    assert " to " in loss
    assert "spread 3.74%" in loss
    # The mean of the headline metrics only: not the counts
    assert not any(line.startswith("  w.sent") for line in lines)
    assert any(
        "metrics are missing in some runs, no interval: w.delay.max" in line for line in lines
    )
    assert lines[-1] == f"  set: {Path('out') / 'runset.json'}"


def test_the_properties_of_a_set_are_the_mean_and_the_ends_of_the_interval() -> None:
    found = dict(runner.set_properties(a_set()))
    # The values come as text, as the JUnit file has them
    assert found["netscenario.t.repetitions"] == "6"
    assert float(str(found["netscenario.t.confidence"])) == pytest.approx(0.95)
    assert float(str(found["netscenario.t.w.loss_ratio.mean"])) == pytest.approx(0.07)
    low = float(str(found["netscenario.t.w.loss_ratio.low"]))
    high = float(str(found["netscenario.t.w.loss_ratio.high"]))
    assert low < 0.07 < high
    # What is not a headline metric is not a property
    assert not any(".sent." in name for name in found)


def test_canary_a_wrong_expectation_about_the_report_fails() -> None:
    # The checks above compare with exact text; this shows that they could fail
    assert runner.describe_set(a_set()).splitlines()[0] != "t: 5 repetitions"


@pytest.mark.usefixtures("runnable")
def test_the_marker_runs_a_set_and_gives_it(pytester: pytest.Pytester) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("plug.yaml", repetitions=5, seed=7, settle=0.2)
        def test_it(netscenario_runs):
            assert netscenario_runs.repetitions == 5
            assert netscenario_runs.seed.value == 7
            assert netscenario_runs.metric("media.sent").mean == 209
            assert netscenario_runs.metric("media.loss_ratio").ci95_high < 0.01
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


@pytest.mark.usefixtures("runnable")
def test_the_numbers_of_a_set_are_in_the_report_and_the_junit_file(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(".yaml", plug=SCENARIO)
    pytester.makepyfile(
        """
        def test_it(netscenario):
            runs = netscenario.run("plug.yaml", repetitions=5, settle=0.2)
            assert runs.metric("media.lost").mean == 999
        """
    )
    result = pytester.runpytest("--junitxml=junit.xml")
    result.assert_outcomes(failed=1)
    result.stdout.fnmatch_lines(
        ["*- netscenario -*", "plug: 5 repetitions, seed * (drawn); interval of the mean*"]
    )
    tree = ET.parse(pytester.path / "junit.xml")  # noqa: S314 - the file was just written by pytest
    names = {e.get("name") for e in tree.iter("property")}
    assert {"netscenario.plug.repetitions", "netscenario.plug.media.loss_ratio.high"} <= names


# ---- the checks of a scenario file (ASR-07, D-094) ----

CHECKED = """\
version: 0
name: checked
duration: 1s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
expect:
  checks:
    - { name: the stream arrives, metric: media.loss_ratio, value: "<= 0%" }
    - { name: CHECK_NAME, metric: media.delay.p99, value: "< BOUND" }
"""


@pytest.mark.usefixtures("runnable")
def test_a_check_of_the_file_that_holds_leaves_the_test_passed(pytester: pytest.Pytester) -> None:
    pytester.makefile(
        ".yaml", checked=CHECKED.replace("CHECK_NAME", "small").replace("BOUND", "1s")
    )
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("checked.yaml")
        def test_it(netscenario_result):
            assert netscenario_result.expect.passed
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)


@pytest.mark.usefixtures("runnable")
def test_a_check_of_the_file_that_does_not_hold_fails_the_test_that_ran_it(
    pytester: pytest.Pytester,
) -> None:
    pytester.makefile(
        ".yaml", checked=CHECKED.replace("CHECK_NAME", "impossibly fast").replace("BOUND", "1ns")
    )
    # The body of the test asserts nothing: the claim is in the file
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("checked.yaml")
        def test_it(netscenario_result):
            assert netscenario_result["media"].sent > 0
        """
    )
    result = pytester.runpytest()
    result.assert_outcomes(failed=1)
    result.stdout.fnmatch_lines(["*1 of 2 checks of the scenario file hold*", "*impossibly fast*"])
    assert "the stream arrives" not in result.stdout.str().split("checks of the scenario file")[1]
    # The same through the fixture that runs a file by name
    pytester.makepyfile(
        test_by_name="""
        def test_it(netscenario):
            netscenario.run("checked.yaml")
        """
    )
    pytester.runpytest("test_by_name.py").assert_outcomes(failed=1)


@pytest.mark.usefixtures("runnable")
def test_a_check_of_the_file_is_a_failure_in_the_junit_file(pytester: pytest.Pytester) -> None:
    pytester.makefile(
        ".yaml", checked=CHECKED.replace("CHECK_NAME", "impossibly fast").replace("BOUND", "1ns")
    )
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("checked.yaml")
        def test_it(netscenario_result):
            pass
        """
    )
    xml = pytester.path / "junit.xml"
    pytester.runpytest(f"--junitxml={xml}").assert_outcomes(failed=1)
    suite = ET.parse(xml).getroot()  # noqa: S314 - the file was just written by pytest
    case = next(suite.iter("testcase"))
    failure = case.find("failure")
    assert failure is not None
    assert "impossibly fast" in (failure.get("message") or failure.text or "")


def test_the_fixtures_know_that_the_checks_need_a_set(pytester: pytest.Pytester) -> None:
    needs_set = (
        CHECKED.replace("CHECK_NAME", "p99")
        .replace("BOUND", "1s")
        .replace("expect:\n  checks:", "expect:\n  repetitions: 5\n  checks:")
        .replace("value:", "max:")
    )
    pytester.makefile(".yaml", checked=needs_set)
    # A single result cannot judge them
    pytester.makepyfile(
        test_single="""
        import pytest

        @pytest.mark.netscenario("checked.yaml")
        def test_it(netscenario_result):
            pass
        """
    )
    result = pytester.runpytest("test_single.py")
    result.assert_outcomes(errors=1)
    result.stdout.fnmatch_lines(["*need a set of 5 runs*netscenario_runs*"])


@pytest.mark.usefixtures("runnable")
def test_the_runs_of_a_file_with_checks_on_a_set_need_no_repetitions_in_the_marker(
    pytester: pytest.Pytester,
) -> None:
    needs_set = (
        CHECKED.replace("CHECK_NAME", "p99")
        .replace("BOUND", "1s")
        .replace("expect:\n  checks:", "expect:\n  repetitions: 5\n  checks:")
        .replace("value:", "max:")
    )
    pytester.makefile(".yaml", checked=needs_set)
    pytester.makepyfile(
        """
        import pytest

        @pytest.mark.netscenario("checked.yaml")
        def test_it(netscenario_runs):
            assert netscenario_runs.repetitions == 5
            assert netscenario_runs.expect.passed
        """
    )
    pytester.runpytest().assert_outcomes(passed=1)
