"""The tables of a result and their frames (PYT-05).

The tests without a library run in the minimum environment (``just py-test-min``), where neither
pandas nor polars is installed; the others are skipped there.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

import netscenario as ns
from netscenario.frames import Table


def test_a_table_has_columns_of_one_length_and_gives_its_rows() -> None:
    table = Table({"a": [1, 2], "b": ["x", None]})
    assert len(table) == 2
    assert table.names == ("a", "b")
    assert list(table["a"]) == [1, 2]
    assert table.to_dicts() == [{"a": 1, "b": "x"}, {"a": 2, "b": None}]
    assert len(Table({})) == 0
    with pytest.raises(ValueError, match="one length"):
        Table({"a": [1], "b": [1, 2]})


@pytest.mark.parametrize(("library", "method"), [("pandas", "to_pandas"), ("polars", "to_polars")])
def test_without_the_library_the_error_names_the_extra(
    monkeypatch: pytest.MonkeyPatch, library: str, method: str
) -> None:
    # A module that is None in sys.modules cannot be imported, as in the minimum environment
    monkeypatch.setitem(sys.modules, library, None)
    with pytest.raises(ImportError, match=rf"netscenario\[{library}\]"):
        getattr(Table({"a": [1]}), method)()


def test_a_table_becomes_a_pandas_frame() -> None:
    pandas = pytest.importorskip("pandas")
    frame = Table({"a": [1, 2], "b": [0.5, None]}).to_pandas()
    assert isinstance(frame, pandas.DataFrame)
    assert list(frame.columns) == ["a", "b"]
    assert frame["a"].tolist() == [1, 2]


def test_a_table_becomes_a_polars_frame() -> None:
    polars = pytest.importorskip("polars")
    frame = Table({"a": [1, 2], "b": [0.5, None]}).to_polars()
    assert isinstance(frame, polars.DataFrame)
    assert frame.columns == ["a", "b"]
    assert frame["a"].to_list() == [1, 2]


@pytest.mark.usefixtures("runnable")
def test_the_packets_the_series_and_the_steps_of_a_run_are_tables(tmp_path: Path) -> None:
    scenario = (
        ns.Scenario.builder("py-frames", duration="2s", seed=3)
        .nodes("client", "server")
        .link("access", "client", "server", forward=ns.Impairments(delay="5ms", loss="10%"))
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .build()
    )
    result = ns.run(scenario, out=tmp_path / "out", packets=True, settle=0.3)
    media = result["media"]

    packets = media.packets_table()
    assert len(packets) >= media.sent
    lost = [bool(flag) for flag in packets["lost"]]
    assert sum(lost) == media.lost
    delays = [d for d in packets["delay"] if d is not None]
    assert delays
    assert min(delays) >= 0.005, "faster than the link allows"
    assert all(d is None for d, flag in zip(packets["delay"], lost, strict=True) if flag)

    assert media.series is not None
    series = media.series.table()
    assert len(series) == media.series.bins
    assert sum(series["sent"]) == media.sent
    assert series.names[:2] == ("bin", "start")

    steps = result.steps_table()
    assert len(steps) == 0
    assert "live" in steps.names


@pytest.mark.usefixtures("runnable")
def test_a_run_without_the_export_says_how_to_get_it(tmp_path: Path) -> None:
    scenario = (
        ns.Scenario.builder("py-frames", duration="1s")
        .nodes("client", "server")
        .link("access", "client", "server")
        .udp_cbr("media", "client", "server", rate="1mbit", packet_size="1200b")
        .build()
    )
    result = ns.run(scenario, out=tmp_path / "out", packets=False, settle=0.2)
    with pytest.raises(ValueError, match="packets=True"):
        result["media"].packets_table()
