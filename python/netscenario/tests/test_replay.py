"""``replay`` from Python: the typed comparison and the refusals (ENV-03, D-075).

The comparison itself is tested in the Rust core and the real replays in ``test_run.py``. These
tests need no machine: they read the JSON that the native library makes, and refuse directories
that cannot be replayed.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, assert_type

import pytest
from test_results import RESULT

import netscenario as ns

COMPARISON: dict[str, Any] = {
    "format": "netscenario-replay",
    "version": 0,
    "original": "first",
    "replay": "again",
    "seed": 7,
    "listed_sequences": 20,
    "comparison": {
        "verdict": "differs",
        "workloads": [
            {
                "id": "media",
                "lost_original": 3,
                "lost_replay": 3,
                "only_original": [4, 8],
                "only_replay": [5, 9],
                "count_only_original": 2,
                "count_only_replay": 2,
                "differing": 4,
            }
        ],
        "provenance": [{"path": "kernel", "recorded": "6.12.1", "other": "6.8.0"}],
        "other_machine": True,
        "reasons": [],
    },
}


def parse(**changes: Any) -> ns.Replay:
    data = json.loads(json.dumps(COMPARISON))
    data["comparison"].update(changes)
    return ns.Replay.from_json(json.dumps(data))


def test_a_replay_is_read_from_the_json_of_the_native_library() -> None:
    again = parse()
    assert again.verdict == "differs"
    assert not again.reproduced
    assert (again.original_dir, again.replay_dir, again.seed) == (Path("first"), Path("again"), 7)
    media = again.workloads[0]
    assert (media.id, media.lost_original, media.lost_replay) == ("media", 3, 3)
    assert (media.only_original, media.only_replay) == ((4, 8), (5, 9))
    assert (media.differing, media.identical) == (4, False)
    assert again.provenance == (ns.Difference("kernel", "6.12.1", "6.8.0"),)
    assert again.other_machine
    assert again.reasons == ()
    assert dict(again.raw)["seed"] == 7


def test_only_the_verdict_reproduced_is_a_reproduced_replay() -> None:
    for verdict in ("differs", "not-promised", "not-comparable"):
        assert not parse(verdict=verdict).reproduced, verdict
    same = parse(
        verdict="reproduced",
        workloads=[
            {
                "id": "media",
                "lost_original": 3,
                "lost_replay": 3,
                "only_original": [],
                "only_replay": [],
                "count_only_original": 0,
                "count_only_replay": 0,
                "differing": 0,
            }
        ],
        other_machine=False,
        provenance=[],
    )
    assert same.reproduced
    assert same.workloads[0].identical
    assert not same.other_machine


def test_what_is_not_a_replay_is_refused() -> None:
    with pytest.raises(ValueError, match="not a netscenario replay"):
        ns.Replay.from_json(json.dumps({**COMPARISON, "format": "netscenario-result"}))
    with pytest.raises(ValueError, match="not a netscenario replay"):
        ns.Replay.from_json(json.dumps({**COMPARISON, "version": 3}))


def test_the_result_of_the_replay_is_read_from_its_directory(tmp_path: Path) -> None:
    directory = tmp_path / "again"
    directory.mkdir()
    (directory / "result.json").write_text(json.dumps(RESULT), encoding="utf-8")
    again = ns.Replay.from_json(
        json.dumps({**COMPARISON, "replay": str(directory)}),
    )
    assert again.result().scenario_name == "lte-like-link"


def test_a_replay_is_typed_as_a_replay_with_a_verdict() -> None:
    def typed(directory: Path) -> None:
        again = ns.replay(directory)
        assert_type(again, ns.Replay)
        assert_type(again.verdict, ns.Verdict)
        assert_type(again.reproduced, bool)

    assert callable(typed), "pyright checks the types of the function above"


def test_a_directory_that_is_not_a_run_is_a_value_error(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match=r"has no result\.json"):
        ns.replay(tmp_path)
    (tmp_path / "runset.json").write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError, match=r"directory of a set of runs.*run-001"):
        ns.replay(tmp_path)


def test_a_run_without_a_recorded_scenario_is_refused_and_says_why(tmp_path: Path) -> None:
    (tmp_path / "result.json").write_text(json.dumps(RESULT), encoding="utf-8")
    with pytest.raises(ValueError, match="did not record the scenario"):
        ns.replay(tmp_path)


def test_a_scenario_that_is_not_the_one_of_the_result_is_refused_with_both_fingerprints(
    tmp_path: Path, repository: Path
) -> None:
    (tmp_path / "result.json").write_text(json.dumps(RESULT), encoding="utf-8")
    scenario = ns.Scenario.load(repository / "scenarios" / "lte-like.yaml")
    (tmp_path / "scenario.json").write_text(json.dumps(json.loads(scenario.to_json())), "utf-8")
    with pytest.raises(ValueError, match=r"sha256:00.*nothing is replayed") as error:
        ns.replay(tmp_path)
    assert "its fingerprint is sha256:" in str(error.value)


def test_the_replay_cannot_overwrite_the_run(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="would overwrite it"):
        ns.replay(tmp_path, out=tmp_path)
