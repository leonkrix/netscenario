"""Acceptance of release 0.3 for TCP against what is known: Mathis, two paths, queues.

Three experiments, each from a scenario file in ``scenarios/``. The numbers were measured on the
development machine (Debian 13, kernel 6.12); the bounds are far enough from them that the CI
machines, which are slower and noisier, do not change the verdict, and each says what a broken build
would give.

**Throughput against loss and delay** (``tcp-throughput-vs-loss.yaml``). Reno over a path with a
random loss ``p`` and a round trip ``RTT``, no rate, four connections: each has, by Mathis et al.
(1997), the goodput ``MSS / RTT * C / sqrt(p)`` with ``C = sqrt(3/2) = 1.22`` for losses at regular
distances. The test computes ``C`` from the goodput of the window (seconds 3 to 11 of bins of 1 s),
the 1448 bytes of payload of a segment, the round trip of the links and the loss in the file.

- Measured ``C``: 1.23 (standard deviation of one run 0.07, six runs) for 40 ms and 1 %, 1.26 (0.09)
  for 80 ms and 0.5 %, and 1.17 to 1.32 in 20 runs of one connection and 20 s at four other points.
  The loss of ``netem`` is random, not regular, which gives a constant a little above 1.22.
- The mean of five runs has a standard deviation of 0.09 / sqrt(5) = 0.04, so the claim ``1.0 <= C
  <= 1.5`` is 6 deviations from 1.25 on each side. Both points must hold it: the same formula for
  ``p`` four times smaller and the round trip twice as long is the check of the scaling with ``1 /
  (RTT * sqrt(p))``, which a goodput that did not follow the loss or the delay would break at once.
- The canary is ``C <= 1.0``, 6 deviations below the measurement.

**Two paths** (``tcp-multipath.yaml`` and ``tcp-multipath-weights.yaml``). A client and a server
joined by a path of 4 mbit and a path of 8 mbit.

- 16 bulk connections over both, with the weights 1 and 2: each connection stays on one path and a
  path with a connection is full, so the goodput is the sum of the capacities, ``12 mbit * 1448/1514
  = 11.48 mbit``, and not that of the best path (``7.65 mbit``). Measured 11.45 to 11.52 mbit in six
  runs (the chance that no connection of 16 takes the slow path is ``(2/3)^16 = 0.15 %``, and the
  goodput is then 7.65). The bounds are the capacity at the top plus what the window can take out of
  the queues and the paths that were full when it began (the goodput counts bytes in order): 2
  queues of 30 packets of 1514 bytes and the 10 ms one way of 12 mbit, 106 kB or 0.21 mbit over the
  4 bins (measured at most +0.6 %, 11.54 mbit in one run of six). At the bottom 95 % of the
  capacity, 3 mbit above what one path can carry. The canary is "no more than the best path", which
  two paths together must not satisfy.
- 32 light connections with the weights 1 and 7: the weights decide where a connection goes. A
  connection takes the slow path with the probability 1/8, so the share of the bytes on it is the
  share of the connections: 0.125 with a standard deviation of ``sqrt(0.125 * 0.875 / 32) = 0.058``
  for one run, and ``0.026`` for the mean of five. The bound ``0.02 <= share <= 0.23`` is 4
  deviations on each side (measured 0.107 over five runs). The equal weights of the canary give 0.5
  with a deviation of 0.04, 7 deviations above the bound.

**Queues** (``tcp-queue.yaml``). Two reno connections fill a bottleneck of 10 mbit with a round trip
of 40 ms. The time in the queue is the median round trip time of the kernel minus its smallest.

- FIFO of 1000 packets: 1000 * 1514 bytes at 10 mbit is 1.21 s when the queue is full, and the
  connections keep it near full (measured 640 to 970 ms in five runs); the bound is a quarter of the
  full queue, 300 ms.
- CoDel (target 5 ms, interval 100 ms): measured 7.1 to 8.6 ms, and 5.0 for FQ-CoDel with a standard
  deviation of 0.05. CoDel allows a delay above the target for up to one interval before it drops,
  so the standing delay is a small multiple of the target; the bound is 4 targets, 20 ms, which is
  2.3 times the worst measurement and 40 times lower than the FIFO. The goodput of the window stays
  at 9.1 to 9.4 mbit for CoDel (95 to 98 % of the capacity) and 8.55 mbit for FQ-CoDel (89 %), so
  the bounds are 85 % and 80 % of ``10 mbit * 1448/1514``.
- The canary is the FIFO against the claim of the 20 ms."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Any

import pytest

import netscenario as ns

pytestmark = [pytest.mark.usefixtures("runnable"), pytest.mark.once]

RUNS = 5
SEGMENT = 1448
"""The bytes of payload of a segment on a link of Ethernet (1500 bytes of IP, 12 of timestamps)."""
CAPACITY_SHARE = SEGMENT / 1514
"""What is payload of a link rate in frames of 1514 bytes."""
FIRST_BIN = 3
END_BIN = 11


def scenarios(repository: Path) -> Path:
    return repository / "scenarios"


def variant(path: Path, change: Any) -> ns.Scenario:
    """The scenario of a file with a change made to its data."""
    data = ns.Scenario.load(path).data
    change(data)
    return ns.Scenario.from_yaml(json.dumps(data), name=path.name, base_dir=path.parent)


def window_goodput(tcp: ns.Tcp, first: int = FIRST_BIN, end: int = END_BIN) -> float:
    """The mean goodput of a workload over the bins ``first`` to ``end`` (exclusive), bit/s."""
    assert tcp.goodput is not None
    assert tcp.goodput.bin == 1.0
    window = tcp.goodput.bits_per_second[first:end]
    return sum(window) / len(window)


# --- Mathis --------------------------------------------------------------------------------


def mathis_constant(result: ns.Result, rtt: float, loss: float) -> float:
    """``C`` of ``goodput = MSS / RTT * C / sqrt(p)`` for each connection of the workload."""
    tcp = result.tcp["flow"]
    assert tcp.congestion == "reno"
    goodput = window_goodput(tcp) / tcp.connections
    return goodput * rtt * math.sqrt(loss) / (SEGMENT * 8)


def mean_constant(runs: ns.RunSet, rtt: float, loss: float) -> float:
    values = [mathis_constant(result, rtt, loss) for result in runs.results]
    return sum(values) / len(values)


@pytest.fixture(scope="module")
def mathis_a(repository: Path) -> ns.RunSet:
    """40 ms and 1 %: the file."""
    runs = ns.run(
        scenarios(repository) / "tcp-throughput-vs-loss.yaml", repetitions=RUNS, settle=0.5
    )
    assert isinstance(runs, ns.RunSet)
    return runs


@pytest.fixture(scope="module")
def mathis_b(repository: Path) -> ns.RunSet:
    """80 ms and 0.5 %: the file with the delay twice as long and the loss half as large."""

    def change(data: dict[str, Any]) -> None:
        link = data["topology"]["links"][0]
        link["forward"]["delay"] = "40ms"
        link["forward"]["loss"]["p"] = "0.5%"
        link["reverse"]["delay"] = "40ms"

    runs = ns.run(
        variant(scenarios(repository) / "tcp-throughput-vs-loss.yaml", change),
        repetitions=RUNS,
        settle=0.5,
    )
    assert isinstance(runs, ns.RunSet)
    return runs


def test_reno_follows_the_formula_of_mathis_at_two_points(
    mathis_a: ns.RunSet, mathis_b: ns.RunSet
) -> None:
    # 40 ms and 1 %, then 80 ms and 0.5 %: the same C if the goodput goes with 1 / (RTT * sqrt(p))
    assert 1.0 <= mean_constant(mathis_a, 0.040, 0.01) <= 1.5
    assert 1.0 <= mean_constant(mathis_b, 0.080, 0.005) <= 1.5


def test_canary_a_constant_below_the_formula_fails(mathis_a: ns.RunSet) -> None:
    with pytest.raises(AssertionError):
        assert mean_constant(mathis_a, 0.040, 0.01) <= 1.0


# --- Two paths --------------------------------------------------------------------------------

SLOW, WIDE = 4e6 * CAPACITY_SHARE, 8e6 * CAPACITY_SHARE
EXTRA = (2 * 30 * 1514 + (4e6 + 8e6) / 8 * 0.010) * 8 / 4
"""What the window of 4 bins can carry above the capacity, bit/s (queues and paths at its start)."""


@pytest.fixture(scope="module")
def both_paths(repository: Path) -> ns.RunSet:
    runs = ns.run(scenarios(repository) / "tcp-multipath.yaml", repetitions=RUNS, settle=0.5)
    assert isinstance(runs, ns.RunSet)
    return runs


def paths_goodput(result: ns.Result) -> float:
    return window_goodput(result.tcp["flows"], 3, 7)


def test_connections_over_two_paths_carry_the_sum_of_their_rates(both_paths: ns.RunSet) -> None:
    for result in both_paths.results:
        goodput = paths_goodput(result)
        assert result.tcp["flows"].failed_connections == 0, result.tcp["flows"].first_error
        # Nothing is faster than the links, but what was queued at the start leaves in the window
        assert 0.95 * (SLOW + WIDE) <= goodput <= SLOW + WIDE + EXTRA, f"{goodput / 1e6:.2f} mbit"
        assert goodput > WIDE, "more than the best path alone can carry"


def test_canary_a_claim_that_one_path_is_enough_fails(both_paths: ns.RunSet) -> None:
    with pytest.raises(AssertionError):
        assert max(paths_goodput(result) for result in both_paths.results) <= WIDE + EXTRA


def slow_share(result: ns.Result) -> float:
    """The share of the bytes that the slow path carries at the client, of what both carry."""
    by_link = {c.link: c.bytes for c in result.captures if c.node == "client"}
    return by_link["client-slow"] / (by_link["client-slow"] + by_link["client-wide"])


def mean_slow_share(runs: ns.RunSet) -> float:
    values = [slow_share(result) for result in runs.results]
    return sum(values) / len(values)


@pytest.fixture(scope="module")
def weighted(repository: Path) -> ns.RunSet:
    runs = ns.run(
        scenarios(repository) / "tcp-multipath-weights.yaml", repetitions=RUNS, settle=0.5
    )
    assert isinstance(runs, ns.RunSet)
    return runs


@pytest.fixture(scope="module")
def equal_weights(repository: Path) -> ns.RunSet:
    def change(data: dict[str, Any]) -> None:
        for route in data["topology"]["routes"]:
            route["weight"] = 1

    runs = ns.run(
        variant(scenarios(repository) / "tcp-multipath-weights.yaml", change),
        repetitions=RUNS,
        settle=0.5,
    )
    assert isinstance(runs, ns.RunSet)
    return runs


def test_the_weights_of_a_route_decide_which_share_of_the_connections_takes_a_path(
    weighted: ns.RunSet,
) -> None:
    for result in weighted.results:
        assert result.tcp["api"].failed_connections == 0, result.tcp["api"].first_error
    # 1 in 8 by the weights 1 and 7: 0.125, deviation 0.026 for the mean of five runs, 4 each side
    assert 0.02 <= mean_slow_share(weighted) <= 0.23


def test_canary_equal_weights_do_not_pass_the_share_of_the_weights(
    equal_weights: ns.RunSet,
) -> None:
    with pytest.raises(AssertionError):
        assert 0.02 <= mean_slow_share(equal_weights) <= 0.23


# --- Queues --------------------------------------------------------------------------------


def queue_delay(result: ns.Result) -> float:
    """The time in the queue: the median round trip of the kernel less its smallest, seconds."""
    rtt = result.tcp["flow"].rtt
    assert rtt is not None
    return rtt.p50 - rtt.min


def queued(repository: Path, discipline: str) -> ns.Result:
    def change(data: dict[str, Any]) -> None:
        link = next(x for x in data["topology"]["links"] if x["id"] == "bottleneck")
        link["forward"]["queue"]["discipline"] = discipline
        if discipline == "fifo":
            for key in ("target", "interval"):
                del link["forward"]["queue"][key]

    result = ns.run(variant(scenarios(repository) / "tcp-queue.yaml", change), settle=0.5)
    assert isinstance(result, ns.Result)
    return result


@pytest.fixture(scope="module")
def fifo(repository: Path) -> ns.Result:
    return queued(repository, "fifo")


@pytest.fixture(scope="module")
def codel(repository: Path) -> ns.Result:
    return queued(repository, "codel")


@pytest.fixture(scope="module")
def fq_codel(repository: Path) -> ns.Result:
    return queued(repository, "fq-codel")


TARGET = 0.005
FULL_FIFO = 1000 * 1514 * 8 / 10e6


def test_codel_and_fq_codel_keep_the_queue_near_their_target_and_the_link_busy(
    codel: ns.Result, fq_codel: ns.Result
) -> None:
    capacity = 10e6 * CAPACITY_SHARE
    assert queue_delay(codel) <= 4 * TARGET
    assert queue_delay(fq_codel) <= 4 * TARGET
    assert window_goodput(codel.tcp["flow"]) >= 0.85 * capacity
    assert window_goodput(fq_codel.tcp["flow"]) >= 0.80 * capacity


def test_a_fifo_of_the_same_size_keeps_most_of_a_second_in_the_queue(
    fifo: ns.Result, codel: ns.Result
) -> None:
    assert FULL_FIFO / 4 <= queue_delay(fifo) <= FULL_FIFO
    assert queue_delay(fifo) >= 10 * queue_delay(codel)


def test_canary_the_claim_of_codel_fails_for_a_fifo(fifo: ns.Result) -> None:
    with pytest.raises(AssertionError):
        assert queue_delay(fifo) <= 4 * TARGET
