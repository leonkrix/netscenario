"""Acceptance of release 0.3 for TCP: two flows share a bottleneck fairly, and a longer round trip
costs a share.

Two reno flows run over the dumbbell of ``scenarios/tcp-fairness.yaml`` (a bottleneck of 10 mbit
with a tail-drop queue of 30 packets, a round trip of 18 ms without the queue) and the same with a
longer access link for one of them (``scenarios/tcp-fairness-unequal-rtt.yaml``, 58 ms). Each
scenario runs six times with derived seeds; the shares are the mean goodput of each flow over the
bins of 1 s from the 3rd second to the 11th, where the flows stop (the start-up and the drain of the
queue are left out).

The numbers, so that no bound is picked by eye. They were measured on the development machine
(Debian 13, kernel 6.12) over 10 runs of the equal link and 14 of the unequal one, and the bounds
are far enough from them that the CI machines, which are slower and noisier, do not change the
verdict:

- **The sum of the goodputs** is the capacity of the bottleneck and nothing else: the queue (30
  packets, more than the 15 of the bandwidth-delay product) never empties after the window of the
  two flows has been halved, so the link is busy all the time. Measured 9.56 mbit in every run,
  which is 10 mbit * 1448/1514 (1448 bytes of payload in a frame of 1514) to the third digit. The
  upper bound is that capacity plus what the window can take out of the network that was put in it
  before: the goodput counts bytes in order, so the bytes that were in the queue (30 packets of 1514
  bytes) and in flight (the one-way delay of at most 29 ms at 10 mbit) when the window began can
  leave in it, 82 kB or 82 kbit/s over the 8 bins (measured at most +0.3 %, 9.59 mbit in one run of
  10); the lower bound is 95 % of the capacity, for a stall of a shared machine, and a queue that is
  too short or a sender that does not fill the link would be below.
- **The imbalance** ``d = (a - b) / (a + b)`` of the two goodputs has a mean of 0 (the two flows are
  the same) and a standard deviation of 0.06 to 0.11 between runs (0.105 for these bins, the worst
  of three series). The Jain index of two flows is ``J = (a + b)^2 / (2 (a^2 + b^2)) = 1 / (1 +
  d^2)``, so a run with ``d = 0.1`` has J = 0.990 and the mean of J over runs is about 0.99.
- **The claim ``mean J >= 0.90``** is ``mean d^2 <= 0.111``. The mean of ``d^2`` over six runs is
  ``sigma^2 = 0.011`` with a standard deviation of ``sigma^2 * sqrt(2 / 6) = 0.0064``, so the bound
  is 15 deviations away (for ``sigma = 0.105``), and one run in which a flow starves the other (d =
  0.8, J = 0.61) moves the mean J by only 0.05; two of them make the claim fail, as they should.
- **The link that must fail it:** with 22 ms more on one access link the shorter flow takes 80 % of
  the bottleneck (d = 0.55 to 0.69, J = 0.69 to 0.83, mean 0.72 over 14 runs, standard deviation
  0.045). The claim ``mean J >= 0.90`` fails by (0.90 - 0.72) / (0.045 / sqrt(6)) = 10 deviations,
  and the canary shows it.
- **The exponent of the round trip.** The goodput ratio of the two flows is ``(rtt_b / rtt_a) **
  alpha`` with the round trip times that the kernel measures (median, queue included: about 42 ms
  and 84 ms). Equal loss rates give ``alpha = 1`` (the formula of Mathis for reno); two flows that
  lose in the same events of a tail-drop queue give ``alpha = 2``. Measured 1.98 to 2.14 with a
  standard deviation of 0.28 of one run, so the mean of six has 0.12. The claim ``1 <= alpha <= 3``
  is 8 deviations from the measurement on each side. The bias is in the right direction and at least
  the 1/RTT of the formula, which is what a dumbbell with one queue shows. The canary ``alpha <=
  1.2`` is 6.5 deviations from the measurement.

``alpha`` above 2 would be a link that favours the short flow beyond what AIMD does; below 1, a link
that favours the long one. The workflow ``stability.yml`` runs these tests many times (``just
stability-tcp``)."""

from __future__ import annotations

import math
from pathlib import Path

import pytest

import netscenario as ns

pytestmark = [pytest.mark.usefixtures("runnable"), pytest.mark.once]

RUNS = 6
FIRST_BIN = 3
"""The first bin of the window: the start-up of the two flows is left out."""
END_BIN = 11
"""The bin after the window: the flows stop at 11 s, and the queue drains after that."""
CAPACITY = 10e6 * 1448 / 1514
"""What the bottleneck carries as payload, in bits per second (1448 bytes in a frame of 1514)."""
EXTRA = (30 * 1514 + 10e6 / 8 * 0.029) * 8 / (END_BIN - FIRST_BIN)
"""What the window can carry above the capacity, bit/s (the queue and the flight at its start)."""
FAIR = 0.90
"""The bound on the mean Jain index; see the module."""


def shares(result: ns.Result) -> tuple[float, float]:
    """The mean goodput of the two flows over the window, in bits per second."""
    out: list[float] = []
    for flow in (result.tcp["first"], result.tcp["second"]):
        assert flow.congestion == "reno"
        assert flow.goodput is not None
        assert flow.goodput.bin == 1.0, "the bins of the file are the bins of the window"
        window = flow.goodput.bits_per_second[FIRST_BIN:END_BIN]
        out.append(sum(window) / len(window))
    return out[0], out[1]


def jain(a: float, b: float) -> float:
    """The index of Jain for two flows: 1 for equal shares, 1/2 when one takes everything."""
    return (a + b) ** 2 / (2 * (a * a + b * b))


def mean_jain(runs: ns.RunSet) -> float:
    values = [jain(*shares(result)) for result in runs.results]
    return sum(values) / len(values)


def exponent(result: ns.Result) -> float:
    """The ``alpha`` of ``goodput_a / goodput_b == (rtt_b / rtt_a) ** alpha`` of one run."""
    a, b = shares(result)
    rtt_a = result.tcp["first"].rtt
    rtt_b = result.tcp["second"].rtt
    assert rtt_a is not None
    assert rtt_b is not None
    return math.log(a / b) / math.log(rtt_b.p50 / rtt_a.p50)


@pytest.fixture(scope="module")
def equal(repository: Path) -> ns.RunSet:
    runs = ns.run(repository / "scenarios" / "tcp-fairness.yaml", repetitions=RUNS, settle=0.5)
    assert isinstance(runs, ns.RunSet)
    return runs


@pytest.fixture(scope="module")
def unequal(repository: Path) -> ns.RunSet:
    runs = ns.run(
        repository / "scenarios" / "tcp-fairness-unequal-rtt.yaml", repetitions=RUNS, settle=0.5
    )
    assert isinstance(runs, ns.RunSet)
    return runs


def test_two_flows_with_the_same_round_trip_share_the_bottleneck_fairly(equal: ns.RunSet) -> None:
    for result in equal.results:
        total = sum(shares(result))
        # Nothing is faster than the link, and the link is kept busy (the queue is deeper than the
        # bandwidth-delay product)
        assert 0.95 * CAPACITY <= total <= CAPACITY + EXTRA, f"{total / 1e6:.3f} mbit/s"
    assert mean_jain(equal) >= FAIR


def test_canary_unequal_round_trips_are_not_fair_and_the_claim_fails(unequal: ns.RunSet) -> None:
    # The same link capacity is used, so the sum is still the capacity: it is the shares that differ
    for result in unequal.results:
        a, b = shares(result)
        assert a > b, "the flow with the shorter round trip takes the larger share"
        assert 0.95 * CAPACITY <= a + b <= CAPACITY + EXTRA
    with pytest.raises(AssertionError):
        assert mean_jain(unequal) >= FAIR


def test_the_share_falls_with_the_round_trip_at_least_as_fast_as_one_over_rtt(
    unequal: ns.RunSet,
) -> None:
    alphas = [exponent(result) for result in unequal.results]
    alpha = sum(alphas) / len(alphas)
    # Between the formula of Mathis (equal loss rates, 1) and a tail-drop queue that both flows lose
    # in (2), with room for the spread of six runs (0.12) on each side
    assert 1.0 <= alpha <= 3.0, f"alpha {alpha:.2f} from {alphas}"


def test_canary_a_claim_that_the_share_follows_one_over_rtt_exactly_fails(
    unequal: ns.RunSet,
) -> None:
    alphas = [exponent(result) for result in unequal.results]
    with pytest.raises(AssertionError):
        assert sum(alphas) / len(alphas) <= 1.2
