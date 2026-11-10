"""A test that decides when the link changes: wait for a moment, then make the link worse."""

import netscenario as ns
from pytest_netscenario import Runner


def test_the_link_gets_worse_when_the_test_says(netscenario: Runner) -> None:
    # `live` names the links the test may change: their queues are built so that any state fits
    with netscenario.start("stream.yaml", live=["access"], packets=True) as run:
        run.wait_until(1.5)  # seconds into the measured phase
        steps = run.link("access").set(
            forward={"delay": "120ms", "loss": {"model": "random", "p": "20%"}}
        )
    result = run.result()
    media = result["media"]

    # The change is a step of the timeline, done between two packets, so the packet that met the
    # old state is known
    (step,) = steps
    assert step.live
    assert step.packet is not None
    assert 0.9 * 125 * 1.5 < step.packet < 1.1 * 125 * 1.5  # 125 packets a second

    # Nothing is lost before the change, about a fifth of the packets after it, and the delay jumps
    packets = media.packets_table()
    lost_at = [n for n, lost in zip(packets["sequence"], packets["lost"], strict=True) if lost]
    assert lost_at
    assert min(lost_at) >= step.packet
    assert media.delay is not None
    assert media.delay.max > 0.120

    # The test chose the moment, so a second run differs; the result says so, and a replay does
    # the change at the same packet
    assert not result.seed.loss_reproducible
    replay = ns.replay(result.out_dir)
    assert replay.reproduced
