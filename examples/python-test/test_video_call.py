"""A network scenario as a pytest test (install `pytest-netscenario`, then run `pytest`)."""

import pytest

import netscenario as ns
from pytest_netscenario import Runner


def test_the_video_arrives_in_time(netscenario: Runner) -> None:
    # The file is looked for next to this test. If a number is missed the test fails, and the
    # report shows the numbers of the run.
    result = netscenario.run("video-call.yaml")
    video = result["video"]

    assert video.loss_ratio < 0.05  # 1 % is configured: allow noise, catch a broken link
    assert video.delay is not None
    assert video.delay.p99 < 0.100  # seconds: 30 ms of delay and a jitter of 5 ms


@pytest.mark.netscenario("video-call.yaml", seed=1)
def test_another_seed_with_the_marker(netscenario_result: ns.Result) -> None:
    # The marker runs the scenario for the test and gives the result; its seed replaces the file's.
    assert netscenario_result["video"].sent > 0
