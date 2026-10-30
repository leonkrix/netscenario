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
    assert properties["netscenario.plug.media.loss_ratio"] == "0.0"
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
