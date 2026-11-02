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


def test_the_loss_is_low_with_a_confidence_interval(netscenario: Runner) -> None:
    # One run is one sample. Five runs with seeds derived from one seed say where the mean of the
    # loss lies: the 95 % interval is [ci95_low, ci95_high]. A claim is stable if the end of the
    # interval that matters is far from the threshold: 1 % is configured, the claim is "below 5 %".
    runs = netscenario.run("video-call.yaml", repetitions=5)
    loss = runs.metric("video.loss_ratio")

    assert loss.ci95_high < 0.05

    # The same for the delay: the mean over the runs of the 99th percentile of each run, in seconds
    assert runs.metric("video.delay.p99").ci95_high < 0.100
