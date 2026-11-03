"""A link that changes in the middle of a run: what the series shows, and the time to recover."""

from pytest_netscenario import Runner


def test_the_drop_and_the_outage_are_in_the_series(netscenario: Runner) -> None:
    result = netscenario.run("bandwidth-drop.yaml")
    media = result["media"]

    # The timeline: four steps, in the order the file gives them (an outage is a start and an end),
    # and each done within a fraction of a second of its moment
    assert [step.kind for step in result.steps] == ["set", "set", "outage-start", "outage-end"]
    assert all(step.lateness < 0.2 for step in result.steps)

    # The series: one bin for each second, from the start of the measured phase
    assert media.series is not None
    bins = media.series.read()
    assert len(bins) >= 14

    # Before the drop all of the stream comes through: 2 mbit of payload, give or take two packets
    for second in (1, 2):
        assert abs(bins[second].by_arrival.bits - 2_000_000) <= 16_000, second

    # While the link is slow (3 s to 5 s) about 58 % of it arrives each second (1.2 mbit of
    # frames, which are the payload and 42 bytes, is 1.15 mbit of payload), and nothing is lost:
    # the rest waits in the queue of the link, which is why the delay of the packets of second 4
    # is above half a second
    for second in (4, 5):
        assert abs(bins[second].by_arrival.bits - 1_151_500) <= 16_000, second
    assert all(bins[second].by_send.lost == 0 for second in range(3, 8))
    delay = bins[4].by_send.delay
    assert delay is not None
    assert delay.p50 > 0.5

    # The outage is a window at the receiver: second 10 is inside it, and nothing arrives
    assert bins[10].by_arrival.received == 0


def test_the_stream_is_back_at_its_level_after_each_event(netscenario: Runner) -> None:
    result = netscenario.run("bandwidth-drop.yaml")
    recoveries = result["media"].recoveries

    # Seen from the sender (the default) a packet that comes late still counts as delivered, so
    # after the drop the stream is at its level at once; after the end of the outage the link is
    # back at that moment and the packets that were lost were sent before it
    assert recoveries, "every event of the timeline that concerns the stream has an entry"
    assert all(recovery.recovered for recovery in recoveries)
    (drop, *_) = [recovery for recovery in recoveries if recovery.kind == "set"]
    assert drop.time == 0

    # The report of the run has the timeline and the times to recover
    text = result.report("md")
    assert "### Timeline" in text
    assert "### Time to recover" in text
