"""The profile library (SCN-06, D-095) through the Python package."""

from __future__ import annotations

from typing import Any

import pytest

import netscenario as ns

LINK = """\
version: 0
name: t
duration: 3s
topology:
  nodes: [terminal, network]
  links:
    - id: access
      between: [terminal, network]
{link}
workloads:
  - {{ id: up, kind: udp-cbr, from: terminal, to: network, rate: 160kbit, packet-size: 1000b }}
"""


def read(link: str) -> ns.Scenario:
    return ns.Scenario.from_yaml(LINK.format(link=link))


def access(scenario: ns.Scenario) -> dict[str, Any]:
    links: list[dict[str, Any]] = scenario.data["topology"]["links"]
    return links[0]


def test_a_profile_is_resolved_and_recorded_in_the_scenario() -> None:
    scenario = read("      profile: lte-good")
    link = access(scenario)
    # The first node of the link is the terminal: forward is up, reverse is down
    assert link["forward"] == {"delay": "24ms"}
    assert link["reverse"] == {"delay": "8ms"}
    assert link["profile"] == "lte-good"
    assert scenario.data["profile-library"] == 1


def test_the_keys_of_the_file_win_over_the_profile() -> None:
    link = access(
        read(
            "      profile: lte-good\n"
            "      forward: { delay: 60ms, loss: { model: random, p: 1% } }"
        )
    )
    assert link["forward"]["delay"] == "60ms"
    assert link["forward"]["loss"] == {"model": "random", "p": "1%"}
    assert link["reverse"] == {"delay": "8ms"}


def test_a_profile_that_does_not_exist_names_the_closest_one() -> None:
    with pytest.raises(ns.ScenarioError, match="no profile called 'lte-typcal'") as raised:
        read("      profile: lte-typcal")
    assert "did you mean 'lte-typical'" in str(raised.value)


def test_the_builder_takes_a_profile_and_the_scenario_reads_back_to_itself() -> None:
    built = (
        ns.Scenario.builder("t", duration="3s")
        .nodes("terminal", "network")
        .link("access", "terminal", "network", profile="geo-good")
        .udp_cbr("up", "terminal", "network", rate="160kbit", packet_size="1000b")
        .build()
    )
    link = access(built)
    assert link["profile"] == "geo-good"
    assert link["reverse"]["rate"] == "144mbit"
    assert "rate" not in link["forward"]
    again = ns.Scenario.from_yaml(built.to_json())
    assert again.to_json() == built.to_json()


def test_a_recorded_library_is_not_resolved_again() -> None:
    """Canary: a recorded library keeps the values of its run, not those of this release."""
    recorded = ns.Scenario.from_yaml(
        LINK.format(link="      profile: lte-good\n      forward: { delay: 99ms }").replace(
            "version: 0\n", "version: 0\nprofile-library: 1\n"
        )
    )
    link = access(recorded)
    assert link["forward"] == {"delay": "99ms"}
    assert "reverse" not in link


def test_a_run_records_the_profile(runnable: None) -> None:
    del runnable
    scenario = read("      profile: dsl-good")
    result = ns.run(scenario)
    assert dict(result.scenario_profiles) == {"access": "dsl-good"}
    assert result.profile_library == 1
