"""Acceptance of release 0.2 for repetitions: an assertion with a confidence interval is stable.

The test runs a scenario eight times and asserts on the interval of the loss, as a user would. The
workflow ``stability.yml`` runs this file a hundred times (``just stability``). The scenario has no
seed in the file, so every call draws a new set, and what is shown is that the verdict does not
depend on the seed.

The numbers, so that the bounds are not picked by eye:

- One run sends 209 packets (1 s at 2 mbit/s with 1200 byte payloads) through a link that loses
  5 % of them at random, so the eight runs send 1672 and lose a binomial number of them: mean
  83.6, standard deviation 8.9.
- The upper end of the interval is the mean of the eight loss ratios plus about
  ``t(7) / sqrt(8) * s = 0.836 * s``, with ``s`` near 0.0151 (the standard deviation of one run),
  so about the mean plus 0.0126.
- The claim ``upper end < 0.10`` is true for the link with room. It fails only if the mean is
  about 0.0874 or more, which is 147 or more lost packets of 1672. The exact binomial probability
  of that is 6.6e-11 for one call (the spread ``s`` of the runs moves the bound a little and not
  the order), so a hundred calls fail with a probability near 1e-8.
- The canary is the same claim with a bound that is false for the link: ``upper end < 0.03``
  holds only if the mean is about 0.0174 or less, which is 29 or fewer lost packets; the exact
  probability is 1.8e-12. A broken interval (a point, or one that is too narrow) would pass the
  stable claim just the same, and the canary shows that a claim can fail: a link that loses 5 %
  must not be able to prove that it loses less than 3 %.
"""

from __future__ import annotations

import pytest

import netscenario as ns

pytestmark = pytest.mark.usefixtures("runnable")

LINK = ns.Impairments(delay="5ms", loss="5%")
RUNS = 8


def lossy() -> ns.Scenario:
    return (
        ns.Scenario.builder("acceptance", duration="1s")
        .nodes("client", "server")
        .link("access", "client", "server", forward=LINK)
        .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        .build()
    )


def test_an_assertion_with_a_confidence_interval_is_stable() -> None:
    runs = ns.run(lossy(), repetitions=RUNS, settle=0.3)
    assert runs.seed.drawn, "no seed in the scenario: every call of this test draws a new set"
    loss = runs.metric("media.loss_ratio")
    assert len({r.seed for r in runs.runs}) == RUNS, "every run has a seed of its own"
    # The claim of a user: the link loses less than 10 %. True with room (see the module)
    assert loss.ci95_high < 0.10
    # The interval holds the mean it is made around
    assert loss.ci95_low <= loss.mean <= loss.ci95_high


def test_canary_a_claim_that_is_false_for_the_link_fails() -> None:
    runs = ns.run(lossy(), repetitions=RUNS, settle=0.3)
    loss = runs.metric("media.loss_ratio")
    # The link loses 5 %: "less than 3 %" must not be provable
    with pytest.raises(AssertionError):
        assert loss.ci95_high < 0.03
