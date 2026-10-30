"""Reading, building and checking scenarios."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

import netscenario as ns

EXAMPLES = ["minimal", "lte-like", "satellite", "lossy-both-ways"]

MINIMAL = """\
version: 0
name: t
duration: 1s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
"""


@pytest.mark.parametrize("name", EXAMPLES)
def test_the_examples_load_and_say_what_they_need(repository: Path, name: str) -> None:
    scenario = ns.Scenario.load(repository / "scenarios" / f"{name}.yaml")
    assert scenario.needs()[:2] == ("point-to-point", "ipv4")
    assert scenario.name


def test_a_scenario_built_in_code_is_the_scenario_of_the_file(repository: Path) -> None:
    loaded = ns.Scenario.load(repository / "scenarios" / "lte-like.yaml")
    built = (
        ns.Scenario.builder("lte-like", duration="30s", seed=42)
        .nodes("client", "server")
        .link(
            "access",
            "client",
            "server",
            forward=ns.Impairments(
                delay="40ms",
                jitter=ns.Jitter("5ms", "normal"),
                loss=ns.Loss("1%"),
                rate="10mbit",
            ),
            reverse=ns.Impairments(delay="40ms", rate="20mbit"),
        )
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .build()
    )
    # The same model whichever way it is made: the canonical forms are equal
    assert built.data == loaded.data
    assert built.to_json() == loaded.to_json()


def test_shorthands_for_the_jitter_and_the_loss_mean_a_uniform_jitter_and_a_random_loss() -> None:
    scenario = (
        ns.Scenario.builder("t", duration="2s")
        .nodes("a", "b")
        .link("l", "a", "b", both=ns.Impairments(delay="20ms", jitter="5ms", loss="1%"))
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
        .build()
    )
    link = scenario.data["topology"]["links"][0]["both"]
    assert link["jitter"] == {"amount": "5ms"}
    assert link["loss"] == {"model": "random", "p": "1%"}


def test_the_data_is_a_copy() -> None:
    scenario = ns.Scenario.from_yaml(MINIMAL)
    scenario.data["name"] = "changed"
    assert scenario.name == "t"
    assert json.loads(scenario.to_json())["name"] == "t"


def test_a_seed_can_be_replaced_and_removed(repository: Path) -> None:
    scenario = ns.Scenario.load(repository / "scenarios" / "lte-like.yaml")
    assert scenario.seed == 42
    assert scenario.with_seed(7).seed == 7
    assert scenario.with_seed(None).seed is None
    assert scenario.seed == 42, "the original is not changed"


def test_a_faulty_file_lists_its_problems_with_their_lines() -> None:
    text = MINIMAL.replace("nodes: [a, b]", "nodes: [a, server]").replace("[a, b]", "[a, sever]")
    text = text.replace("to: b", "to: server")
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.from_yaml(text, name="bad.yaml")
    problem = raised.value.problems[0]
    assert (problem.line, problem.path) == (8, "topology.links[0].between[1]")
    assert problem.help is not None
    assert "did you mean 'server'" in problem.help
    assert "bad.yaml:8:" in str(raised.value)


def test_a_bare_number_is_refused_because_it_has_no_unit() -> None:
    builder = (
        ns.Scenario.builder("t", duration="2s")
        .nodes("a", "b")
        .link("l", "a", "b", both=ns.Impairments(delay="20"))
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
    )
    with pytest.raises(ns.ScenarioError) as raised:
        builder.build()
    assert "unit" in str(raised.value)


def test_a_scenario_built_in_code_has_problems_with_paths_and_without_lines() -> None:
    builder = (
        ns.Scenario.builder("t", duration="2s")
        .nodes("a", "b")
        .link("l", "a", "c")
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
    )
    with pytest.raises(ns.ScenarioError) as raised:
        builder.build()
    problem = raised.value.problems[0]
    assert problem.path == "topology.links[0].between[1]"
    assert (problem.line, problem.column) == (None, None)
    assert "-->" not in str(raised.value), "no lines of a generated text"
    assert "path: topology.links[0].between[1]" in str(raised.value)


def test_every_faulty_file_of_the_corpus_is_refused_with_problems(repository: Path) -> None:
    files = sorted((repository / "scenarios" / "invalid").glob("*.yaml"))
    assert len(files) >= 40
    for file in files:
        with pytest.raises(ns.ScenarioError) as raised:
            ns.Scenario.load(file)
        assert raised.value.problems, file
        assert str(raised.value).startswith("error: "), file


def test_the_missing_file_is_an_oserror(tmp_path: Path) -> None:
    with pytest.raises(OSError, match=r"nothing\.yaml"):
        ns.Scenario.load(tmp_path / "nothing.yaml")
