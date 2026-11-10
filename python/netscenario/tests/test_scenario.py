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


def test_a_stream_without_the_checksum_is_written_as_a_boolean_and_checked_by_the_core() -> None:
    builder = ns.Scenario.builder("t", duration="2s").nodes("a", "b").link("l", "a", "b")
    scenario = builder.udp_cbr(
        "w", "a", "b", rate="1mbit", packet_size="1000b", udp_checksum=False
    ).build()
    assert scenario.data["workloads"][0]["udp-checksum"] is False
    # The default writes nothing, so a scenario that does not ask for it is unchanged
    plain = (
        ns.Scenario.builder("t", duration="2s")
        .nodes("a", "b")
        .link("l", "a", "b")
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
        .build()
    )
    assert "udp-checksum" not in plain.data["workloads"][0]
    # Canary: the core refuses a payload that cannot carry the CRC-32, and says why
    with pytest.raises(ns.ScenarioError, match="too small for `udp-checksum: false`"):
        (
            ns.Scenario.builder("t", duration="2s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .udp_cbr("w", "a", "b", rate="1mbit", packet_size="18b", udp_checksum=False)
            .build()
        )


def test_an_echo_is_written_as_a_boolean_and_the_core_refuses_it_without_the_checksum() -> None:
    builder = ns.Scenario.builder("t", duration="2s").nodes("a", "b").link("l", "a", "b")
    scenario = builder.udp_poisson(
        "w", "a", "b", rate="1mbit", packet_size="1000b", echo=True
    ).build()
    assert scenario.data["workloads"][0]["echo"] is True
    plain = (
        ns.Scenario.builder("t", duration="2s")
        .nodes("a", "b")
        .link("l", "a", "b")
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
        .build()
    )
    assert "echo" not in plain.data["workloads"][0]
    # Canary: a stream that is echoed has the checksum, and the core says so
    with pytest.raises(ns.ScenarioError, match="cannot be echoed"):
        (
            ns.Scenario.builder("t", duration="2s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .udp_cbr(
                "w", "a", "b", rate="1mbit", packet_size="1000b", udp_checksum=False, echo=True
            )
            .build()
        )


def test_the_traffic_patterns_built_in_code_are_the_scenario_of_the_file(repository: Path) -> None:
    loaded = ns.Scenario.load(repository / "scenarios" / "traffic-patterns.yaml")
    built = (
        ns.Scenario.builder("traffic-patterns", duration="20s", seed=7)
        .nodes("client", "server")
        .link("access", "client", "server", both=ns.Impairments(delay="20ms"))
        .udp_poisson("requests", "client", "server", rate="800kbit", packet_size="400b")
        .udp_onoff(
            "sensor",
            "client",
            "server",
            rate="400kbit",
            packet_size="200b",
            on="500ms",
            off="1500ms",
            distribution="exponential",
        )
        .udp_bursty(
            "video",
            "server",
            "client",
            packet_size="1200b",
            burst_packets=10,
            gap="200us",
            pause="100ms",
            distribution="exponential",
        )
        .build()
    )
    assert built.data == loaded.data
    assert "traffic-patterns" in built.needs()
    # Canary: a constant bit rate of the same scenario does not need the patterns
    plain = ns.Scenario.from_yaml(MINIMAL)
    assert "traffic-patterns" not in plain.needs()


def test_a_burst_that_is_too_fast_is_refused_by_the_core_with_a_path() -> None:
    builder = (
        ns.Scenario.builder("t", duration="1s")
        .nodes("a", "b")
        .link("l", "a", "b")
        .udp_bursty("w", "a", "b", packet_size="1000b", burst_packets=4, gap="1us", pause="10ms")
    )
    with pytest.raises(ns.ScenarioError) as raised:
        builder.build()
    assert [problem.path for problem in raised.value.problems] == ["workloads[0].gap"]


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


def test_the_measure_section_is_built_from_its_arguments_and_left_out_when_empty() -> None:
    def build(**measure: object) -> ns.Scenario:
        return (
            ns.Scenario.builder("m", duration="2s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
            .measure(**measure)  # type: ignore[arg-type]
            .build()
        )

    assert "measure" not in build().data
    full = build(
        bin="500ms",
        recovery_basis="arrival",
        recovery_baseline_bins=4,
        recovery_window_bins=2,
        recovery_fraction="80%",
        capture=["l"],
        packets=True,
    )
    assert full.data["measure"] == {
        "series": {"bin": "500ms"},
        "recovery": {
            "basis": "arrival",
            "baseline-bins": 4,
            "window-bins": 2,
            "fraction": "80%",
        },
        "capture": ["l"],
        "packets": True,
    }


def test_canary_a_capture_of_a_link_that_does_not_exist_is_refused_with_the_path() -> None:
    with pytest.raises(ns.ScenarioError, match=r"measure\.capture"):
        (
            ns.Scenario.builder("m", duration="2s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
            .measure(capture=["nope"])
            .build()
        )


TABLE = """\
version: 0
name: t
duration: 1s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
      forward:
        delay: 100ms
        jitter: { amount: 10ms, distribution: table, table: { file: FILE } }
workloads:
  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
"""


def _samples(count: int = 400) -> str:
    return "".join(f"{20 + i % 37}\n" for i in range(count))


def _jitter_table(scenario: ns.Scenario) -> dict[str, object]:
    topology = scenario.data["topology"]
    return topology["links"][0]["forward"]["jitter"]["table"]  # type: ignore[no-any-return]


def test_a_file_of_samples_next_to_the_scenario_becomes_the_table_in_it(tmp_path: Path) -> None:
    (tmp_path / "delays.txt").write_text(_samples(), encoding="utf-8")
    file = tmp_path / "near.yaml"
    file.write_text(TABLE.replace("FILE", "delays.txt"), encoding="utf-8")
    scenario = ns.Scenario.load(file)
    table = _jitter_table(scenario)
    assert table["file"] == "delays.txt"
    assert table["samples"] == 400
    assert len(str(table["sha256"])) == 64
    assert len(str(table["values"])) == 4096 * 4
    # What was made of the file is in the scenario, so a copy needs no file and keeps the table
    again = ns.Scenario.from_yaml(scenario.to_json())
    assert _jitter_table(again) == table
    assert _jitter_table(scenario.with_seed(5)) == table


def test_a_scenario_that_refers_to_a_file_needs_a_directory_and_stays_in_it(
    tmp_path: Path,
) -> None:
    (tmp_path / "delays.txt").write_text(_samples(), encoding="utf-8")
    text = TABLE.replace("FILE", "delays.txt")
    with pytest.raises(ns.ScenarioError) as nowhere:
        ns.Scenario.from_yaml(text)
    assert "there is no directory to look for it in" in str(nowhere.value)
    ns.Scenario.from_yaml(text, base_dir=tmp_path)
    # A path that leaves the directory is refused, and allowed when asked for (a canary)
    inside = tmp_path / "inside"
    inside.mkdir()
    outside = TABLE.replace("FILE", "../delays.txt")
    with pytest.raises(ns.ScenarioError) as refused:
        ns.Scenario.from_yaml(outside, base_dir=inside)
    assert "is outside the directory of the scenario file" in str(refused.value)
    ns.Scenario.from_yaml(outside, base_dir=inside, allow_outside_files=True)


def test_samples_that_make_no_table_are_a_problem_at_the_file(tmp_path: Path) -> None:
    (tmp_path / "few.txt").write_text("1\n2\n3\n", encoding="utf-8")
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.from_yaml(TABLE.replace("FILE", "few.txt"), base_dir=tmp_path)
    assert "3 samples are too few" in str(raised.value)
    assert raised.value.problems[0].path == "topology.links[0].forward.jitter.table.file"


TRACE = """\
version: 0
name: t
duration: 5s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
workloads:
  - { id: call, kind: trace-replay, from: a, to: b, trace: { file: call.csv } }
"""


def test_a_trace_next_to_the_scenario_becomes_its_packets(tmp_path: Path) -> None:
    rows = "".join(f"{i * 20}ms,{60 + (i % 5) * 100}b\n" for i in range(50))
    (tmp_path / "call.csv").write_text("time,size\n" + rows, encoding="utf-8")
    file = tmp_path / "call.yaml"
    file.write_text(TRACE, encoding="utf-8")
    scenario = ns.Scenario.load(file)
    trace = scenario.data["workloads"][0]["trace"]
    assert trace["file"] == "call.csv"
    assert len(trace["sha256"]) == 64
    assert trace["packets"]
    assert "trace-replay" in scenario.needs()
    # The scenario holds the packets, so a copy needs no file
    again = ns.Scenario.from_yaml(scenario.to_json())
    assert again.data["workloads"][0]["trace"]["packets"] == trace["packets"]
    # A faulty file is a problem at the file, with the line
    (tmp_path / "call.csv").write_text("0ms,100b\n5,100b\n", encoding="utf-8")
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.load(file)
    assert "line 2: the time" in str(raised.value)
    assert raised.value.problems[0].path == "workloads[0].trace.file"


def test_the_tcp_workloads_are_built_like_the_file_writes_them() -> None:
    built = (
        ns.Scenario.builder("tcp", duration="20s")
        .nodes("a", "b")
        .link("l", "a", "b")
        .tcp_bulk("down", "b", "a", bytes="5mb", connections=2, cc="reno")
        .request_response(
            "api", "a", "b", request="200b", response="4kb", interval="20ms", timeout="5s"
        )
        .build()
    )
    assert built.data["workloads"] == [
        {
            "id": "down",
            "kind": "tcp-bulk",
            "from": "b",
            "to": "a",
            "bytes": "5mb",
            "connections": 2,
            "cc": "reno",
        },
        {
            "id": "api",
            "kind": "request-response",
            "from": "a",
            "to": "b",
            "request": "200b",
            "response": "4kb",
            "interval": "20ms",
            "timeout": "5s",
        },
    ]
    assert "tcp-workloads" in built.needs()


def test_canary_a_tcp_workload_with_no_connections_is_refused_with_the_path() -> None:
    with pytest.raises(ns.ScenarioError, match=r"workloads\[0\]\.connections"):
        (
            ns.Scenario.builder("tcp", duration="20s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .tcp_bulk("down", "b", "a", connections=0)
            .build()
        )


TRACED = """\
version: 0
name: traced
duration: 4s
allow-limited: [fast-rate-steps]
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
      both: { delay: 10ms }
      model:
        kind: trace
        reverse: { file: down.up, format: mahimahi, window: 50ms }
workloads:
  - { id: w, kind: udp-cbr, from: b, to: a, rate: 8mbit, packet-size: 1200b }
"""


def test_a_trace_of_a_link_becomes_its_points_and_asks_for_what_it_needs(tmp_path: Path) -> None:
    (tmp_path / "down.up").write_text("".join(f"{ms}\n" for ms in range(1, 401)), encoding="utf-8")
    file = tmp_path / "traced.yaml"
    file.write_text(TRACED, encoding="utf-8")
    scenario = ns.Scenario.load(file)
    trace = scenario.data["topology"]["links"][0]["model"]["reverse"]
    assert trace["file"] == "down.up"
    assert len(trace["sha256"]) == 64
    assert trace["points"]
    needs = scenario.needs()
    assert "link-traces" in needs
    assert "rate-steps" in needs
    assert scenario.data["allow-limited"] == ["fast-rate-steps"]
    # The scenario holds the points, so a copy needs no file
    again = ns.Scenario.from_yaml(scenario.to_json())
    assert again.data["topology"]["links"][0]["model"]["reverse"]["points"] == trace["points"]
    # A file that is no trace is a problem at the file, with the line
    (tmp_path / "down.up").write_text("5\n3\n", encoding="utf-8")
    with pytest.raises(ns.ScenarioError) as raised:
        ns.Scenario.load(file)
    assert "line 2: 3 comes after 5" in str(raised.value)
    assert raised.value.problems[0].path == "topology.links[0].model.reverse.file"


def test_the_restrictions_a_scenario_accepts_are_built_from_the_argument() -> None:
    built = (
        ns.Scenario.builder("limited", duration="1s", allow_limited=["fast-rate-steps"])
        .nodes("a", "b")
        .link("l", "a", "b")
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
        .build()
    )
    assert built.data["allow-limited"] == ["fast-rate-steps"]
    plain = (
        ns.Scenario.builder("plain", duration="1s")
        .nodes("a", "b")
        .link("l", "a", "b")
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1000b")
        .build()
    )
    assert "allow-limited" not in plain.data


def test_the_builder_makes_routers_routes_a_timeline_and_checks() -> None:
    built = (
        ns.Scenario.builder("typed", duration="20s", seed=1)
        .node("client")
        .node("edge", role="router")
        .node("server")
        .link(
            "access",
            "client",
            "edge",
            cost=2,
            mtu=1400,
            both=ns.Impairments(
                delay="20ms",
                loss={
                    "model": "gilbert-elliott",
                    "p": "2%",
                    "r": "30%",
                    "loss-bad": "50%",
                    "loss-good": "0.1%",
                },
                reorder=ns.Reorder("5%", gap=3),
                duplicate="0.5%",
                corrupt="0.2%",
            ),
        )
        .link("core", "edge", "server", both=ns.Impairments(rate="10mbit", queue={"limit": "50p"}))
        .routing("auto", ecmp=False)
        .udp_cbr("media", "client", "server", rate="1mbit", packet_size="1200b")
        .event("5s", "core", forward=ns.Impairments(rate="5mbit"))
        .event("8s", "core", set={"forward": {"rate": "20mbit"}})
        .event("10s", "access", outage="1s", every="3s", count=2)
        .event("14s", "access", down=True)
        .event("15s", "access", up=True)
        .expect("media.loss_ratio", value="< 30%")
        .build()
    )
    data = built.data
    assert data["topology"]["nodes"][1] == {"id": "edge", "role": "router"}
    access = data["topology"]["links"][0]
    assert (access["cost"], access["mtu"]) == (2, 1400)
    assert access["both"]["reorder"] == {"share": "5%", "gap": 3}
    assert access["both"]["loss"]["model"] == "gilbert-elliott"
    assert [event["link"] for event in data["timeline"]] == ["core", "core", "access"] + [
        "access"
    ] * 2
    assert data["expect"]["checks"][0]["metric"] == "media.loss_ratio"


def test_the_builder_refuses_what_the_core_refuses() -> None:
    builder = (
        ns.Scenario.builder("typed", duration="5s")
        .nodes("a", "b")
        .link("l", "a", "b", both=ns.Impairments(reorder="5%"))
        .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1200b")
    )
    # A reordering needs a delay
    with pytest.raises(ns.ScenarioError):
        builder.build()
    with pytest.raises(ns.ScenarioError):
        (
            ns.Scenario.builder("typed", duration="5s")
            .nodes("a", "b")
            .link("l", "a", "b")
            .udp_cbr("w", "a", "b", rate="1mbit", packet_size="1200b")
            .event("9s", "l", down=True)
            .build()
        )
