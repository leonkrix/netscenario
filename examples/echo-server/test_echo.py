"""Your own program in a node, tested over a bad link (needs `pytest-netscenario`)."""

from pytest_netscenario import Runner


def test_the_echo_server_answers_over_a_lossy_link(netscenario: Runner) -> None:
    result = netscenario.run("echo.yaml")

    client = result.programs["client"]
    # The client ended by itself with 0; the server was stopped by the run (signal of `shutdown`)
    assert client.succeeded
    assert result.programs["server"].how_it_ended == "stopped"

    sent = client.metrics["sent"].last
    answered = client.metrics["answered"].last
    assert sent == 200
    # A packet is answered if neither it nor its answer is lost: 0.95 * 0.95 = 90.25 % of 200 is
    # 180, with a standard deviation of 4.2; four of them each way round are 17 packets
    assert 163 <= answered <= 198

    rtt = sorted(value for _, name, value in client.readings() if name == "rtt")
    assert len(rtt) == answered
    # The link takes 30 ms each way: no answer comes faster than 60 ms, and the median is near
    assert rtt[0] >= 60
    assert rtt[len(rtt) // 2] < 90
