# Fidelity of the Kernel backend

Does the network do what the scenario says, and by how much does it deviate? This page answers that with measurements (QA-06). Every case below is a scenario with a known answer, run in real network namespaces and measured by the agent; the test judges the measurement against bounds that are written down, each with its reason. The cases are in [`tests/fidelity.rs`](../crates/netscenario-backend-kernel/tests/fidelity.rs), and they run on every pull request on both runner images.

## How to read the tables

- **Expected** is what the configuration predicts, or, for the three cases marked KNOWN, what a named mechanism of `netem` predicts.
- **Measured** is what this run found. A ✔ means inside the bounds, a ✘ is a failure of the test.
- **Deviation** is measured minus expected.
- **Allowed** is the range in which the test passes, and **Why** says where it comes from.

The bounds follow four rules, in the order of their strength:

1. **Physics never moves.** A delay cannot be shorter than the link makes it. The lower bounds of delays are exact.
2. **Noise only goes one way.** A busy shared machine makes packets later, never earlier. The upper bounds of delays leave room (0.5 ms on a median), and that room is where a CI runner may differ from a laptop. Where an upper bound is about a single packet (the largest delay), it is replaced by the 99th percentile, because one disturbed packet is routine on a shared machine and sixty are not. An earlier version of the test used the largest delay, and failed on a runner for that reason.
3. **Statistics get four standard deviations.** A count (lost packets) or a mean over thousands of packets is allowed four standard deviations on each side. A fair run lies outside with a probability of about 6 in 100,000.
4. **Known mechanisms are named.** Where the backend does something that a reader may not expect, the case states the number that the mechanism predicts and the bound is around it.

The tests run one at a time (`.config/nextest.toml`), because the sender of the agent spins for the last stretch before each packet and two of them side by side on four cores would disturb each other.

## Three canaries

A test that judges must be able to fail. Three tests run a scenario that is wrong on purpose and judge it with the bounds of the real case: **a rate that is not applied** (the link carries all of the offered 3 mbit, and the case for 2 mbit fails), **a delay that is not applied**, and **a loss that is not applied or is applied at the wrong ratio**. A fourth test judges made-up numbers and checks that each wrong number is named. If a bound were too wide to catch these, the canary would pass the wrong network, and the test would fail.

## Measured

What this machine measured. The numbers differ from run to run in the last digits and from machine to machine in the noise; the bounds do not. Each CI run uploads the same tables for its runner (the artifacts `fidelity-<os>-rust-<version>` of the run), and `just fidelity` rewrites this block with what the machine that runs it measures.

<!-- fidelity:begin -->
Measured on kernel 6.12.113+deb13-amd64, iproute2 `tc`: iproute2 6.15.0.

#### A fixed delay of 20 ms

`forward: { delay: 20ms }`, offered 2mbit with payloads of 1200 bytes for 6 s (1250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 20.000 ms | 20.010 ms ✔ | +0.010 ms | 20.000 ms to 20.500 ms | no packet can be faster than the link; the path adds 0.016 ms (spike S4) |
| median delay | 20.000 ms | 20.017 ms ✔ | +0.017 ms | 20.000 ms to 20.500 ms | `netem` is exact to the microsecond; 0.5 ms leave room for a busy shared runner |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a link without loss loses nothing |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a fixed delay keeps the order |

#### A random loss of 5 %

`forward: { loss: { model: random, p: 5% } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| loss | 5.00 % | 4.90 % ✔ | -0.10 pp | 3.87 % to 6.13 % | binomial: 6,000 packets, a standard deviation of 0.28 percentage points, four of them on each side |
| median delay | 0.016 ms | 0.009 ms ✔ | -0.007 ms | 0.000 ms to 0.500 ms | a link without a delay adds only the path (0.016 ms); 0.5 ms for noise |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | loss does not reorder |

#### A rate limit of 2 mbit, offered 3 mbit

`forward: { rate: 2mbit }`, offered 3mbit with payloads of 1000 bytes for 6 s (2250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| steady payload rate | 1.9194 mbit/s | 1.9200 mbit/s ✔ | +0.03 % | 1.8810 mbit/s to 1.9580 mbit/s | `netem rate` counts the Ethernet frame, so 1,000 bytes of payload use 1,042; 2 % for the edges of the one-second bins (about 240 packets each) |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the backlog (about 800 packets) stays below the queue of 1,000, so a rate limit alone loses nothing here |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a rate limit keeps the order |

#### A delay of 20 ms with a uniform jitter of 5 ms

`forward: { delay: 20ms, jitter: { amount: 5ms } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 15.000 ms | 15.010 ms ✔ | +0.010 ms | 15.000 ms to 15.500 ms | uniform jitter reaches down to the delay minus the amount and no further |
| 99th percentile of the delay | 24.900 ms | 24.906 ms ✔ | +0.006 ms | 24.500 ms to 25.500 ms | the upper end of a uniform spread, 15 + 0.99 * 10 ms. The largest delay is not used: one packet disturbed by the machine would fail it, 60 are needed to fail the 99th percentile |
| mean delay | 20.000 ms | 20.056 ms ✔ | +0.056 ms | 19.850 ms to 20.500 ms | the mean of 6,000 uniform delays has a standard deviation of 0.04 ms; four of them below, noise above |
| standard deviation of the delay | 2.887 ms | 2.885 ms ✔ | -0.002 ms | 2.740 ms to 3.040 ms | 5 / sqrt(3) for a uniform distribution of width 10 ms; the estimate has a standard deviation of 0.02 ms, the bound is wider because the measurement adds noise |
| reordered packets | 58.00 % | 58.87 % ✔ | +0.87 pp | 30.00 % to 80.00 % | KNOWN: jitter larger than the gap between packets (1 ms here) reorders them, because `netem` sends each packet at its own time; 58 % were seen |

#### A delay of 20 ms with a normal jitter of 5 ms

`forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 20.000 ms | 20.083 ms ✔ | +0.083 ms | 19.700 ms to 20.500 ms | the mean of 6,000 normal delays has a standard deviation of 0.07 ms; four of them below, noise above |
| standard deviation of the delay | 5.000 ms | 4.967 ms ✔ | -0.033 ms | 4.700 ms to 5.300 ms | the amount is the standard deviation (spike S4 measured 5.03); the table of `netem` is cut off at four standard deviations |
| 99th percentile of the delay | 31.600 ms | 31.558 ms ✔ | -0.042 ms | 30.400 ms to 32.600 ms | the mean plus 2.326 standard deviations; the estimate has a standard deviation of 0.24 ms, four of them on each side, with noise on the upper side |
| reordered packets | 72.00 % | 71.52 % ✔ | -0.48 pp | 40.00 % to 90.00 % | KNOWN: as with the uniform jitter; 72 % were seen |

#### A delay of 40 ms with a normal jitter of 5 ms and a rate limit of 10 mbit

`forward: { delay: 40ms, jitter: { amount: 5ms, distribution: normal }, rate: 10mbit }`, offered 2mbit with payloads of 1200 bytes for 8 s (1667 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 42.600 ms | 42.823 ms ✔ | +0.223 ms | 41.800 ms to 43.600 ms | KNOWN: not 40 + 1.0 ms. Because the rate stage forces the order, delays can only grow: a model of `sch_netem` gives 42.6 ms (see docs/FIDELITY.md) |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | KNOWN: with a rate limit the packets stay in order, jitter or not |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the offered 2 mbit is far below the 10 mbit of the link |

<!-- fidelity:end -->

## What to know when you write a scenario

These are the findings behind the KNOWN rows. They are properties of `netem`, the reference of the project, and every one is in [COVERAGE.md](COVERAGE.md) as well.

**The rate counts Ethernet frames.** `rate 2mbit` limits the frames of the link, and a frame is the payload plus 8 bytes of UDP, 20 of IP and 14 of Ethernet. The payload that gets through is `rate × payload / (payload + 42)`: with payloads of 1,000 bytes 1.919 mbit, with payloads of 100 bytes 1.408 mbit. The case measured 1.9200 mbit against a prediction of 1.9194. A scenario that wants 2 mbit of payload has to ask for a rate that is higher by that factor, or use bigger packets.

**A rate limit is a queue, and a queue is a delay.** When more is offered than the link carries, the packets wait, and the waiting counts as delay: in the rate case the delay grew to 3.4 seconds. The queue holds the packets until its limit (derived from the scenario, see [COVERAGE.md](COVERAGE.md), "Queue of the Kernel backend"), and the run has to wait for them before it can say they are lost (`--settle`, [CLI.md](CLI.md)).

**Jitter reorders packets.** `netem` gives every packet its own random delay and sends it at its own moment. If the jitter is larger than the gap between two packets, a later packet overtakes an earlier one. In the jitter cases a packet every millisecond with 5 ms of jitter gave 58 % reordered packets for a uniform and 72 % for a normal jitter (`reordered` in the result counts the packets that arrived after a packet with a higher number, RFC 4737). The distribution of the delays is the configured one: the mean was within 0.13 ms of the configured one and the standard deviation within 0.04 ms of 5 ms (over the runs made while writing this page), and the uniform spread reached 15.01 ms at the lower end. If your program cannot cope with reordering, the jitter has to be smaller than the gap between its packets.

**Jitter and a rate limit in one direction raise the mean delay.** With a rate limit the packets leave in order: a packet cannot leave before the one in front of it. So the delays can only grow, and the jitter is cut off from below. For a delay of 40 ms with a normal jitter of 5 ms, a rate of 10 mbit and a packet every 4.8 ms, a model of `netem_enqueue` (a packet leaves at the later of its own moment and its predecessor's moment, plus its serialisation of 0.99 ms) gives a mean of 42.6 ms; the model is a test in the file ([`the_model_of_netem_gives_the_mean_of_a_jitter_with_a_rate_limit`](../crates/netscenario-backend-kernel/tests/fidelity.rs)) and the measurement was 42.6 to 42.8 ms. Without the rate limit the same jitter gives 40 ms. The example scenario `lte-like.yaml` has exactly this combination, which is how it was found: its median delay is 42.5 ms, not 40. Whether such a scenario should be refused, run with a notice in the result, or counted as limited is open (PLAN, section 12a).

**The delay is exact; the noise is small.** A fixed delay was 20.010 ms at the smallest and 20.017 ms at the median in the reference run, which is the delay plus 0.01 ms for the path (spike S4 measured 0.016 ms). On the CI runners the path costs a little more; the bounds allow 0.5 ms.

**The loss is the configured ratio, and with a seed it is the same packets.** 5 % of 6,000 packets: 294 lost (4.90 %), inside the 3.9 % to 6.1 % of four standard deviations. The pattern of losses is reproducible with a `seed` and a `tc` of iproute2 6.6.0 or newer ([RESULTS.md](RESULTS.md), "seed").

## What is not measured yet

- **The round trip time** (release 0.1 measures the one-way delay) and the **RFC 3550 jitter** (release 0.2).
- **Both directions at once, and several links.** Release 0.1 has one link. The cases impair the forward direction and measure the stream that goes that way.
- **Burst loss, duplication, corruption, outages** (release 0.2) and the **timeline** (changes over time, release 0.2) have no cases yet. Each new impairment gets its case here in the release that brings it.
- **Packet rates above about 1,000 per second** and long runs. The cases send 200 to 1,000 packets per second for 6 to 8 seconds.
- **Other machines than the two CI images and the development machine.** The fidelity of a virtual machine with a busy host depends on the host.

## How to update this page

```bash
just fidelity
```

runs the fidelity tests and rewrites the block between the markers with what this machine measured. Commit the file only from a machine that you want to be the reference. The rest of the page is written by hand.
