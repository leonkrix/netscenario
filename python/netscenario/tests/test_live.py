"""A run that is changed from test code, and the asyncio API.

They run in real network namespaces and stop where the machine cannot make them (and fail on the
CI).
"""

from __future__ import annotations

import asyncio
import threading
import time
from pathlib import Path

import pytest

import netscenario as ns

pytestmark = pytest.mark.usefixtures("runnable")


def scenario(duration: str = "2s") -> ns.Scenario:
    return (
        ns.Scenario.builder("py-live", duration=duration, seed=5)
        .nodes("client", "server")
        .link("access", "client", "server", forward=ns.Impairments(delay="5ms"))
        .udp_cbr("media", "client", "server", rate="3mbit", packet_size="1000b")
        .build()
    )


def lost_packets(directory: Path) -> list[int]:
    import json

    lines = (directory / "packets-media.jsonl").read_text(encoding="utf-8").splitlines()
    return [row["sequence"] for row in map(json.loads, lines) if row.get("lost") is True]


def test_a_change_from_the_code_lands_at_a_packet_and_the_replay_loses_the_same_packets(
    tmp_path: Path,
) -> None:
    with ns.start(
        scenario(), live=["access"], out=tmp_path / "live", packets=True, settle=1
    ) as run:
        run.wait_until(1.0)
        steps = run.link("access").set(forward={"loss": {"model": "random", "p": "30%"}})
    result = run.result()
    assert [(s.kind, s.link, s.direction, s.live) for s in steps] == [
        ("set", "access", "forward", True)
    ]
    step = steps[0]
    assert step.packet is not None
    assert 300 < step.packet < 500, step
    # What the run says is what the call returned
    assert [s.packet for s in result.steps] == [step.packet]
    assert result.steps[0].live
    assert result.live_links == ("access",)
    live_lost = lost_packets(result.out_dir)
    assert live_lost
    assert min(live_lost) >= step.packet
    assert 60 < len(live_lost) < 170
    assert not result.seed.loss_reproducible
    assert any("code" in why for why in result.seed.not_reproducible_because)
    # The replay does the change at the same packet and loses the same packets
    replayed = ns.replay(result.out_dir, out=tmp_path / "again")
    again = replayed.result()
    assert again.steps[0].packet == step.packet
    assert not again.steps[0].live
    assert lost_packets(again.out_dir) == live_lost
    assert replayed.reproduced, replayed.verdict


def test_what_the_code_may_not_do_is_refused_and_the_run_goes_on() -> None:
    with ns.start(scenario(), live=["access"], settle=0.5) as run:
        run.wait_until(0.3)
        with pytest.raises(ValueError, match="not declared live"):
            run.link("elsewhere")
        link = run.link("access")
        with pytest.raises(ValueError, match="at least one key"):
            link.set()
        with pytest.raises(ns.ScenarioError):
            link.set(delay=None, jitter="5ms")
        link.set(delay="20ms", jitter=ns.Jitter("5ms", "normal"))
        with pytest.raises(ns.CapabilityError, match="distribution"):
            link.set(jitter="5ms")
    assert len(run.result().steps) == 2  # one `both` change, a step for each direction


def test_down_and_up_and_both_directions_are_steps() -> None:
    with ns.start(scenario(), live=["access"], settle=0.5) as run:
        run.wait_until(0.3)
        link = run.link("access")
        both = link.set(delay="10ms")
        assert [(s.direction, s.packet is not None) for s in both] == [
            ("forward", True),
            ("reverse", False),
        ]
        assert link.down().kind == "down"
        time.sleep(0.1)
        assert link.up().kind == "up"
    assert [s.kind for s in run.result().steps] == ["set", "set", "down", "up"]


def test_the_events_come_in_order_until_the_run_ends() -> None:
    with ns.start(scenario("1s"), settle=0.2) as run:
        names = [type(event).__name__ for event in run.events()]
    assert names[:3] == ["Provisioned", "ReceiversReady", "Started"]
    assert names[-1] == "Finished"
    assert run.done
    assert run.result()["media"].sent > 0


def test_leaving_the_block_with_an_exception_cancels_the_run_and_tears_the_network_down() -> None:
    began = time.monotonic()
    run = ns.start(scenario("30s"))

    def leave() -> None:
        with run:
            run.wait_until(0.2)
            raise KeyError

    with pytest.raises(KeyError):
        leave()
    assert time.monotonic() - began < 10, "the run did not stop"
    with pytest.raises(ns.Cancelled):
        run.result()


def test_a_wait_for_a_moment_after_the_run_ended_says_so() -> None:
    with (
        ns.start(scenario("1s"), settle=0.2) as run,
        pytest.raises(ns.RunError, match="ended before"),
    ):
        run.wait_until(100.0)


def test_a_link_the_scenario_does_not_have_cannot_be_declared_live() -> None:
    with pytest.raises(ValueError, match="no link 'nowhere'"):
        ns.start(scenario(), live=["nowhere"])


def test_run_async_returns_the_result_and_does_not_block_the_loop() -> None:
    async def main() -> tuple[ns.Result, int]:
        ticks = 0

        async def tick() -> None:
            nonlocal ticks
            while True:
                await asyncio.sleep(0.05)
                ticks += 1

        ticker = asyncio.create_task(tick())
        try:
            result = await ns.run_async(scenario("1s"), settle=0.2)
        finally:
            ticker.cancel()
        return result, ticks

    result, ticks = asyncio.run(main())
    assert result["media"].sent > 0
    assert ticks > 10, "the loop was blocked while the run went"


def test_a_cancelled_task_cancels_the_run_and_leaves_no_thread_behind() -> None:
    before = threading.active_count()

    async def main() -> float:
        task = asyncio.create_task(ns.run_async(scenario("30s")))
        await asyncio.sleep(1.0)
        began = time.monotonic()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        return time.monotonic() - began

    took = asyncio.run(main())
    assert took < 10, "the cancelled run did not stop"
    # The worker threads of the executor end with the loop
    deadline = time.monotonic() + 5
    while threading.active_count() > before and time.monotonic() < deadline:
        time.sleep(0.05)
    assert threading.active_count() <= before


def test_a_live_change_and_the_events_from_asyncio() -> None:
    async def main() -> tuple[tuple[ns.TimelineStep, ...], list[str], ns.Result]:
        async with await ns.start_async(scenario("2s"), live=["access"], settle=0.5) as run:
            names: list[str] = []

            async def listen() -> None:
                async for event in run.events():
                    names.append(type(event).__name__)

            listener = asyncio.create_task(listen())
            await run.wait_until(0.5)
            steps = await run.link("access").set(delay="30ms")
            result = await run.result()
            await listener
        return steps, names, result

    steps, names, result = asyncio.run(main())
    assert steps[0].live
    assert steps[0].packet is not None
    assert names[0] == "Provisioned"
    assert names[-1] == "Finished"
    assert "StepDone" in names
    assert result.steps[0].packet == steps[0].packet


def test_the_result_is_the_same_object_every_time_it_is_asked_for() -> None:
    run = ns.start(scenario("1s"), settle=0.2)
    assert run.result() is run.result()


def test_a_scenario_whose_checks_need_a_set_of_runs_cannot_be_started_live() -> None:
    needs_a_set = (
        ns.Scenario.builder("py-live", duration="1s")
        .nodes("client", "server")
        .link("access", "client", "server")
        .udp_cbr("media", "client", "server", rate="1mbit", packet_size="1000b")
        .expect("media.loss_ratio", mean="< 5%", repetitions=5)
        .build()
    )
    with pytest.raises(ValueError, match="set of 5 runs"):
        ns.start(needs_a_set)
