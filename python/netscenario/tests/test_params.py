"""Parameters, templates and includes (SCN-05, D-093) through the Python package."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

import netscenario as ns

WITH_PARAMS = """\
version: 0
name: t
duration: 5s
params:
  rtt: 40ms
  rate: 10mbit
  flows:
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
      both: { delay: "{rtt / 2}", rate: "{rate}" }
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: "{rate / flows}", packet-size: 1000b }
"""


def link(scenario: ns.Scenario) -> dict[str, Any]:
    links: list[dict[str, Any]] = scenario.data["topology"]["links"]
    return links[0]


def test_the_values_given_are_used_and_recorded() -> None:
    scenario = ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 4})
    assert link(scenario)["both"] == {"delay": "20ms", "rate": "10mbit"}
    assert scenario.data["workloads"][0]["rate"] == "2500kbit"
    assert scenario.params == {"flows": "4", "rate": "10mbit", "rtt": "40ms"}


def test_values_of_python_types_are_written_as_the_file_would() -> None:
    scenario = ns.Scenario.from_yaml(
        WITH_PARAMS, params={"flows": 2, "rtt": "100ms", "rate": "1mbit"}
    )
    assert link(scenario)["both"] == {"delay": "50ms", "rate": "1mbit"}
    half = ns.Scenario.from_yaml(WITH_PARAMS.replace("flows:", "flows: 1"), params={"flows": 0.5})
    assert half.data["workloads"][0]["rate"] == "20mbit"
    assert half.params["flows"] == "0.5"
    # A very small float is not written with an exponent, which no scenario reads
    text = WITH_PARAMS.replace("flows:", "tiny: 1").replace("{rate / flows}", "{rate}")
    assert ns.Scenario.from_yaml(text, params={"tiny": 0.00001}).params["tiny"] == "0.00001"
    assert ns.Scenario.from_yaml(text, params={"tiny": 1e-07}).params["tiny"] == "0.0000001"
    assert ns.Scenario.from_yaml(text, params={"tiny": 1.5e-07}).params["tiny"] == "0.00000015"
    assert (
        ns.Scenario.from_yaml(text, params={"tiny": 2.5e16}).params["tiny"] == "25000000000000000"
    )


def test_a_parameter_without_a_default_must_be_given() -> None:
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.from_yaml(WITH_PARAMS)
    message = str(raised.value)
    assert "needs a value for `flows`" in message
    assert "--param flows=<value>" in message
    (problem,) = raised.value.problems
    assert problem.line == 7
    assert problem.file is None
    assert problem.chain == ()


def test_a_wrong_value_and_an_unknown_name_are_refused() -> None:
    with pytest.raises(ns.ScenarioError, match="is a rate, and a duration was given"):
        ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 2, "rate": "50ms"})
    with pytest.raises(ns.ScenarioError, match="did you mean `rtt`"):
        ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 2, "rt": "50ms"})
    with pytest.raises(ValueError, match="no value of a scenario"):
        ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": float("nan")})


def test_a_scenario_is_read_again_with_other_values() -> None:
    first = ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 2})
    second = first.with_params(rtt="80ms")
    assert link(second)["both"] == {"delay": "40ms", "rate": "10mbit"}
    # What was given before stays
    assert second.params["flows"] == "2"
    assert first.params["rtt"] == "40ms"
    # Another value is another scenario, and so another hash
    assert second.to_json() != first.to_json()


def test_a_scenario_that_was_changed_or_built_cannot_be_read_again() -> None:
    seeded = ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 2}).with_seed(3)
    with pytest.raises(ValueError, match="was not read from a file or text"):
        seeded.with_params(rtt="1ms")


def test_the_json_of_a_scenario_with_parameters_reads_back_to_itself() -> None:
    scenario = ns.Scenario.from_yaml(WITH_PARAMS, params={"flows": 4})
    again = ns.Scenario.from_yaml(scenario.to_json())
    assert again.to_json() == scenario.to_json()


def test_templates_are_expanded_in_a_file_without_a_directory() -> None:
    text = """\
version: 0
name: t
duration: 5s
templates:
  hop:
    params: { id: , delay: 5ms }
    content: { id: "{id}", between: [a, b], both: { delay: "{delay}" } }
topology:
  nodes: [a, b]
  links:
    - use: hop
      with: { id: first }
    - use: hop
      with: { id: second, delay: 7ms }
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
"""
    scenario = ns.Scenario.from_yaml(text)
    assert [entry["id"] for entry in scenario.data["topology"]["links"]] == ["first", "second"]
    assert scenario.data["topology"]["links"][1]["both"] == {"delay": "7ms"}


def write_scenario(directory: Path, rate: str = "{rate}") -> Path:
    (directory / "parts").mkdir()
    (directory / "parts" / "access.yaml").write_text(
        'params:\n  id:\n  rate: 10mbit\ncontent:\n  id: "{id}"\n  between: [a, b]\n'
        f'  both: {{ rate: "{rate}" }}\n',
        encoding="utf-8",
    )
    path = directory / "scenario.yaml"
    path.write_text(
        "version: 0\nname: t\nduration: 5s\ntopology:\n  nodes: [a, b]\n  links:\n"
        "    - include: parts/access.yaml\n      with: { id: l, rate: 2mbit }\n"
        "workloads:\n"
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n",
        encoding="utf-8",
    )
    return path


def test_an_included_file_is_read_from_the_directory_of_the_scenario(tmp_path: Path) -> None:
    scenario = ns.Scenario.load(write_scenario(tmp_path))
    assert link(scenario)["both"] == {"rate": "2mbit"}
    # The text alone has no directory to look in
    with pytest.raises(ns.ScenarioError, match="there is no directory to look for it in"):
        ns.Scenario.from_yaml((tmp_path / "scenario.yaml").read_text(encoding="utf-8"))


def test_a_fault_in_an_included_file_is_shown_there_with_the_place_of_the_include(
    tmp_path: Path,
) -> None:
    path = write_scenario(tmp_path, rate="{rate / 0}")
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.load(path)
    (problem,) = raised.value.problems
    assert problem.message == "division by zero"
    assert problem.file == "parts/access.yaml"
    assert problem.line == 7
    assert len(problem.chain) == 1
    assert problem.chain[0].startswith(f"used from {path}:7:")
    assert "--> parts/access.yaml:7:" in str(raised.value)


def test_a_file_outside_the_directory_is_refused_unless_it_is_allowed(tmp_path: Path) -> None:
    inner = tmp_path / "inner"
    inner.mkdir()
    (tmp_path / "shared.yaml").write_text("content: { id: x, between: [a, b] }\n", encoding="utf-8")
    path = inner / "scenario.yaml"
    path.write_text(
        "version: 0\nname: t\nduration: 5s\ntopology:\n  nodes: [a, b]\n  links:\n"
        "    - include: ../shared.yaml\n"
        "workloads:\n"
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n",
        encoding="utf-8",
    )
    with pytest.raises(ns.ScenarioError, match="outside the directory of the scenario file"):
        ns.Scenario.load(path)
    assert ns.Scenario.load(path, allow_outside_files=True).name == "t"


def test_the_examples_with_parameters_and_includes_load(repository: Path) -> None:
    for name in ["parameters", "templates", "include"]:
        scenario = ns.Scenario.load(repository / "scenarios" / f"{name}.yaml")
        assert scenario.name == name
    bigger = ns.Scenario.load(
        repository / "scenarios" / "parameters.yaml", params={"rate": "20mbit"}
    )
    assert bigger.data["workloads"][0]["rate"] == "16mbit"
    assert json.loads(bigger.to_json())["params"]["rate"] == "20mbit"
