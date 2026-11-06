# Fidelity of the Kernel backend

Does the network do what the scenario says, and by how much does it deviate? This page answers that with measurements (QA-06). Every case below is a scenario with a known answer, run in real network namespaces and measured by the agent; the test judges the measurement against bounds that are written down, each with its reason. The cases are in [`tests/fidelity.rs`](../crates/netscenario-backend-kernel/tests/fidelity.rs), and they run on every pull request on both runner images.

## How to read the tables

- **Expected** is what the configuration predicts or, where the configuration alone does not say, what a model of the scenario predicts (the mean, the deviation and the reordering of the case with a jitter and a rate). Two cases are marked KNOWN: that jitter reorders packets is a property of `netem`, and the expected share is what was seen.
- **Measured** is what this run found. A ✔ means inside the bounds, a ✘ is a failure of the test.
- **Deviation** is measured minus expected.
- **Allowed** is the range in which the test passes, and **Why** says where it comes from.

The bounds follow four rules, in the order of their strength:

1. **Physics never moves.** A delay cannot be shorter than the link makes it. The lower bounds of delays are exact.
2. **Noise only goes one way.** A busy shared machine makes packets later, never earlier. The upper bounds of delays leave room (0.5 ms on a median), and that room is where a CI runner may differ from a laptop. Where an upper bound is about a single packet (the largest delay), it is replaced by a percentile, because one disturbed packet is routine on a shared machine and sixty are not: the 99th for a distribution without a cap, the 99.9th (the ninth largest of 8,000 delays) for the cap of a table. The largest delay of a table keeps its lower bound only, which noise cannot break. An earlier version of the test used the largest delay, and failed on a runner for that reason; so did the cap of the paretonormal table (a delay 2.3 ms above it).
3. **Statistics get four standard deviations.** A count (lost packets) or a mean over thousands of packets is allowed four standard deviations on each side. A fair run lies outside with a probability of about 6 in 100,000.
4. **Known mechanisms are named.** Where the backend does something that a reader may not expect, the case states the number that the mechanism predicts and the bound is around it.

The tests run one at a time (`.config/nextest.toml`), because the sender of the agent spins for the last stretch before each packet and two of them side by side on four cores would disturb each other. The agent is optimised in the development profile as well (`Cargo.toml`, D-061): the debug build of the receiver took half a core at 4,000 packets of 1,200 bytes a second, which on a shared runner lost packets and once ran past the watchdog.

## Paths of several links

Three cases run a client, two routers and a server in a row (D-078): **the delays of three links add up** (10, 15 and 5 ms give a smallest delay of 30 ms, which no packet undercuts), **the losses compound** (3 % and 4 % give 1 - 0.97 x 0.96 = 6.88 %, with the binomial band of 6,000 packets), and **the rate of the path is its smallest rate** (a limit of 2 mbit on the middle link between two links of 10 mbit gives the steady rate of the single-link case). Each also holds the tally to zero unaccounted packets: what entered the first link is lost in a queue, dropped by a node or delivered, link by link. Three canaries (a delay, a loss and a rate that one link of the path does not apply) prove that the bounds fail for a wrong path. A fourth case runs the rate limit over IPv6, where a frame is 20 bytes longer (1,062 bytes for a payload of 1,000), and a fifth the loss of a link that cuts the packet into fragments (a lost fragment loses the packet); each has a canary. The tables of these cases appear in the block below when `just fidelity` has run on a machine with `netem`.

## Jitter from tables

Three cases check the tables of the jitter against the table itself (D-084): `pareto` and `paretonormal` of iproute2, and a table made from a file of samples with a long tail (an exponential shape that reaches 8 deviations, so its entries are scaled by 2.024 to fit 16 bits). Each compares the mean, the median, the standard deviation, the smallest and the largest delay with what the 4,096 entries give: the first three within four standard deviations of the estimate from 8,000 delays, the last two as exact physical bounds (no delay is outside the table) with the quantile at which the smallest or the largest of 8,000 delays lies with a chance of 1 - 1e-9. A canary shows that a jitter of the same mean and deviation with another shape fails the case of the table. The measured tables of these cases come with the next `just fidelity` run on a machine with `netem`.

## Queue disciplines

Three cases overload a link of 2 mbit with a queue of 100 packets by one fifth (2.4 mbit of payload against the 1.932 that it carries) and look at the delay (LNK-09, D-082). **A tail-drop queue** holds its whole length: a packet waits for the 100 frames in front of it, 496.8 ms, plus the delay of 10 ms, and the loss is what the queue and the link cannot carry (16.2 %). **CoDel** and **FQ-CoDel** (one flow, so they behave alike) keep the queue near the target: they leave the dropping state when a packet has waited less than 5 ms and enter it again only after a delay above the target for an interval of 100 ms, in which an offer 24 % above the capacity adds 24 ms of delay, so the delay moves between 5 and 29 ms and the median of the packets is about 32 ms with the delay of the link and the frame in service. The bounds are the model's and the canary swaps the disciplines: a FIFO fails the cases of CoDel and FQ-CoDel on the median and the 99th percentile, and a CoDel fails the case of the FIFO. The acceptance of TCP measures the same queues with senders that react (`python/netscenario/tests/test_acceptance_tcp_models.py`), where the delay falls to the target.

## A rate that changes

The tests of the traces (`tests/traces.rs`, LMD-03, D-091) run whole scenarios and judge what the receiver saw, not cases of the table. **A staircase** between 6 and 2 mbit with steps of 500 ms: the payload that arrives in each step is the rate times the time in the units of the frames (1,000 / 1,042), within 5,000 bytes (the bucket of the class, two frames at the edges, the packet that waits for the barrier of the step), where a `netem` rate delivers 75 % of the bytes of a staircase at every step width (D-070). **A Mahimahi file of one opportunity in every millisecond** is 12.112 mbit: 2,000 packets of 1,472 bytes in two seconds, within 4. **A file that alternates every 100 ms** between one and two opportunities delivers 3,000 packets in two passes, within 24 (0.4 % at steps of 100 ms and the edges). **Steps of 50 ms** are followed to within 2 % under the model and are refused unless the scenario accepts `fast-rate-steps`; **steps of 10 ms** are refused whatever the scenario accepts (the shaper delivers 17 % fewer bytes at 5 ms and 41 % fewer at 2 ms, S19). **A window without an opportunity** is one frame in the window (a shaper cannot stop: a packet in service at a floor of 8 kbit waited a second after the rate returned), and needs `stalled-link`; the stall may last up to one row longer than the trace says (the frame of the stall costs a row, and if the step that ends it is applied late a second frame leaves at that rate and its cost is paid after the rate has returned: the test allows 400 packets fewer for the two stalls of its window and none more than the lateness of the steps, so the 4,000 of a link that does not stall at all are refused). The tests have a canary of their own: with the change of the class left out they fail.

## Three canaries

A test that judges must be able to fail. Three tests run a scenario that is wrong on purpose and judge it with the bounds of the real case: **a rate that is not applied** (the link carries all of the offered 3 mbit, and the case for 2 mbit fails), **a delay that is not applied**, and **a loss that is not applied or is applied at the wrong ratio**. A fourth test judges made-up numbers and checks that each wrong number is named. If a bound were too wide to catch these, the canary would pass the wrong network, and the test would fail.

## Measured

What this machine measured. The numbers differ from run to run in the last digits and from machine to machine in the noise; the bounds do not. Each CI run uploads the same tables for its runners, in two halves for each image (the artifacts `fidelity-<os>-part-<n>` of the run), compared below; `just fidelity` rewrites this block with what the machine that runs it measures.

<!-- fidelity:begin -->
Measured on kernel 6.12.115+deb13.1-amd64, iproute2 `tc`: iproute2 6.15.0.

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
| smallest delay | 15.000 ms | 15.015 ms ✔ | +0.015 ms | 15.000 ms to 15.500 ms | uniform jitter reaches down to the delay minus the amount and no further |
| 99th percentile of the delay | 24.900 ms | 24.901 ms ✔ | +0.001 ms | 24.500 ms to 25.500 ms | the upper end of a uniform spread, 15 + 0.99 * 10 ms. The largest delay is not used: one packet disturbed by the machine would fail it, 60 are needed to fail the 99th percentile |
| mean delay | 20.000 ms | 20.034 ms ✔ | +0.034 ms | 19.850 ms to 20.500 ms | the mean of 6,000 uniform delays has a standard deviation of 0.04 ms; four of them below, noise above |
| standard deviation of the delay | 2.887 ms | 2.893 ms ✔ | +0.006 ms | 2.740 ms to 3.040 ms | 5 / sqrt(3) for a uniform distribution of width 10 ms; the estimate has a standard deviation of 0.02 ms, the bound is wider because the measurement adds noise |
| reordered packets | 58.00 % | 58.87 % ✔ | +0.87 pp | 30.00 % to 80.00 % | KNOWN: jitter larger than the gap between packets (1 ms here) reorders them, because `netem` sends each packet at its own time; 58 % were seen |
| interarrival jitter (mean of RFC 3550) | 3.543 ms | 3.497 ms ✔ | -0.046 ms | 3.390 ms to 3.690 ms | MODEL (RFC 3550, appendix A.8): the delays are independent and uniform over 10 ms, a packet every millisecond, and the estimate pairs packets in the order of arrival, so the pair is not independent of its delays (the mean of |D| for neighbours in sequence would be 10 / 3 = 3.33 ms); a simulation of 120 runs of 6,000 packets, written apart from the code under test, gave 3.543 ms with a standard deviation of 0.032 ms between runs; four of them on each side, and 0.02 ms for the machine |
| mean extent of the reordered packets | 3.70 packets | 3.70 packets ✔ | +0.00 packets | 3.57 packets to 3.83 packets | MODEL (RFC 4737, section 4.2): the same simulation, the mean over the reordered packets of the number of arrivals since the first packet that overtook them: 3.70 with a standard deviation of 0.032 between runs, four of them on each side |
| mean late time of the reordered packets | 3.185 ms | 3.188 ms ✔ | +0.003 ms | 3.050 ms to 3.320 ms | MODEL (RFC 4737, section 4.3): the same simulation, the mean time between a reordered packet and the first packet that overtook it: 3.185 ms with a standard deviation of 0.032 ms between runs, four of them on each side, and 0.01 ms for the machine |

#### A delay of 20 ms with a normal jitter of 5 ms

`forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 20.000 ms | 20.042 ms ✔ | +0.042 ms | 19.700 ms to 20.500 ms | the mean of 6,000 normal delays has a standard deviation of 0.07 ms; four of them below, noise above |
| standard deviation of the delay | 5.000 ms | 5.014 ms ✔ | +0.014 ms | 4.700 ms to 5.300 ms | the amount is the standard deviation (spike S4 measured 5.03); the table of `netem` is cut off at four standard deviations |
| 99th percentile of the delay | 31.600 ms | 31.787 ms ✔ | +0.187 ms | 30.400 ms to 32.600 ms | the mean plus 2.326 standard deviations; the estimate has a standard deviation of 0.24 ms, four of them on each side, with noise on the upper side |
| reordered packets | 72.00 % | 72.03 % ✔ | +0.03 pp | 40.00 % to 90.00 % | KNOWN: as with the uniform jitter; 72 % were seen |

#### A delay of 40 ms with a normal jitter of 5 ms and a rate limit of 10 mbit

`forward: { delay: 40ms, jitter: { amount: 5ms, distribution: normal }, rate: 10mbit }`, offered 2mbit with payloads of 1200 bytes for 8 s (1667 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 41.000 ms | 41.150 ms ✔ | +0.150 ms | 40.500 ms to 41.800 ms | 40 ms plus the 0.994 ms that a frame needs at 10 mbit. The mean of 1,667 delays has a standard deviation of 0.12 ms. One qdisc for both gave 42.6 ms, because its rate stage cuts the jitter off (D-030) |
| standard deviation of the delay | 5.000 ms | 4.924 ms ✔ | -0.076 ms | 4.650 ms to 5.350 ms | the amount is the standard deviation; the estimate over 1,667 delays has a standard deviation of 0.09 ms. One qdisc for both gave 4.1 ms |
| reordered packets | 29.00 % | 27.95 % ✔ | -1.05 pp | 15.00 % to 40.00 % | as for any jitter, packets overtake each other when the jitter is larger than the gap (4.8 ms here): the model of the series of the two stages gives 29 % |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the offered 2 mbit is far below the 10 mbit of the link |

#### Loss in bursts: Gilbert-Elliott, 1 % to the bad state, 20 % back, all lost there

`forward: { loss: { model: gilbert-elliott, p: 1%, r: 20%, loss-bad: 100%, loss-good: 0% } }`, offered 4mbit with payloads of 200 bytes for 12 s (30000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| loss | 4.76 % | 4.54 % ✔ | -0.22 pp | 3.52 % to 6.00 % | the share of time in the bad state, p / (p + r) = 1 / 21 (S12: 4.61 % at 100,000 packets); the losses come in runs, so the spread is wider than a binomial: 30,000 packets, a standard deviation of 0.31 percentage points (S12 saw 0.17 at 100,000), four of them on each side |
| mean run of lost packets | 5.00 packets | 4.66 packets ✔ | -0.34 packets | 3.94 packets to 6.06 packets | all packets in the bad state are lost, so a run is a stay in it: 1 / r = 5 packets, about 290 runs, a standard deviation of the mean of 0.26, four of them on each side (S12: 4.99) |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | loss does not reorder |

#### Loss in bursts: the four-state model, 2 % 30 % 20 % 10 % 1 %

`forward: { loss: { model: markov4, p13: 2%, p31: 30%, p32: 20%, p23: 10%, p14: 1% } }`, offered 4mbit with payloads of 200 bytes for 12 s (30000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| loss | 6.34 % | 5.78 % ✔ | -0.56 pp | 4.63 % to 8.04 % | the stationary loss of the chain of four states (S12: 6.37 % at 100,000 packets); 30,000 packets, a standard deviation of 0.42 percentage points, four of them on each side |
| mean run of lost packets | 1.77 packets | 1.69 packets ✔ | -0.07 packets | 1.52 packets to 2.02 packets | the mean length of a run of lost packets of the chain (S12: 1.76); a standard deviation of the mean of 0.06, four of them on each side |

#### Reordering: a delay of 10 ms, 25 % of the packets sent on at once

`forward: { delay: 10ms, reorder: { share: 25% } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets sent on at once | 25.00 % | 24.38 % ✔ | -0.62 pp | 22.80 % to 27.20 % | `reorder P` sends the share P on at once and holds the rest for the delay (D-048, S12: 25.10 %); binomial, 6,000 packets, a standard deviation of 0.56 percentage points, four of them on each side |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | reordering loses nothing |
| reordered packets | 70.00 % | 69.90 % ✔ | -0.10 pp | 55.00 % to 85.00 % | KNOWN: the packets that are held arrive after later ones that went on at once; about 0.75 x (1 - 0.75^10) of the packets when 10 packets are sent within the delay (S12 saw 74.9 % at another rate) |

#### Reordering with a gap: 25 % sent on at once, but never two within 5 packets

`forward: { delay: 10ms, reorder: { share: 25%, gap: 5 } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets sent on at once | 12.50 % | 12.28 % ✔ | -0.22 pp | 10.80 % to 14.20 % | after one packet is sent on at once the next four are held, then each is chosen with 25 %: 1 / (gap - 1 + 1 / share) = 12.5 % (D-048, S12: 12.49 %); a standard deviation of 0.43 percentage points, four of them on each side |

#### Duplication of 20 %

`forward: { duplicate: 20% }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived twice | 20.00 % | 19.60 % ✔ | -0.40 pp | 17.90 % to 22.10 % | `duplicate P` copies a packet with the probability P (S12: 20.14 %); binomial, 6,000 packets, a standard deviation of 0.52 percentage points, four of them on each side |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | duplication loses nothing |

#### Duplication of 20 % with a loss of 10 %: each copy is lost on its own

`forward: { duplicate: 20%, loss: { model: random, p: 10% } }`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived twice | 16.20 % | 15.82 % ✔ | -0.38 pp | 14.30 % to 18.10 % | KNOWN (D-048): a packet arrives twice if it is copied and both survive, 0.2 x 0.9 x 0.9 = 16.2 %, not 20 %; a standard deviation of 0.48 percentage points, four of them on each side |
| loss | 8.20 % | 8.33 % ✔ | +0.13 pp | 6.80 % to 9.60 % | KNOWN: a packet is lost if every copy is: 0.8 x 0.1 + 0.2 x 0.1 x 0.1 = 8.2 %, below the 10 % of a copy; a standard deviation of 0.35 percentage points, four of them on each side |

#### Duplication of 20 % in a direction with a jitter and a rate: two queues in one tree

`forward: { delay: 20ms, jitter: { amount: 5ms }, rate: 10mbit, duplicate: 20% }`, offered 4mbit with payloads of 500 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived twice | 20.00 % | 19.60 % ✔ | -0.40 pp | 17.90 % to 22.10 % | the copy is made in the first queue, where the loss and the rate are, and the tree of two `netem` is accepted by the kernel (S15, D-052); binomial, 6,000 packets, four standard deviations |
| smallest delay | 15.000 ms | 15.445 ms ✔ | +0.445 ms | 15.000 ms to 15.600 ms | the second queue still holds the delay and its jitter: uniform jitter reaches down to the delay minus the amount; the rate adds the time of one frame (0.4 ms) |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the offered 4 mbit and its copies are far below the 10 mbit of the link |

#### Reordering with a rate limit of 2 mbit, offered 3 mbit: the rate holds

`forward: { delay: 10ms, reorder: { share: 25% }, rate: 2mbit }`, offered 3mbit with payloads of 1000 bytes for 6 s (2250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| steady payload rate | 1.9194 mbit/s | 1.9200 mbit/s ✔ | +0.03 % | 1.8810 mbit/s to 1.9580 mbit/s | as for the rate alone: 2 mbit of frames; a packet chosen for reordering would go past the rate if it were in the same queue (S12.9: 25 % of the packets did), so the rate is in a queue of its own in front of it (D-054) |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the backlog stays below the queue of 1,000 packets, as for the rate alone |

#### A corruption of 10 % with a payload of 64 bytes: the node drops the damaged packets

`forward: { corrupt: 10% }`, offered 2048kbit with payloads of 64 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| frames dropped by the node as damaged | 9.43 % | 9.34 % ✔ | -0.09 pp | 8.61 % to 10.26 % | KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (106 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 100 / 106 = 9.43 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, to the packet |
| loss | 9.43 % | 9.34 % ✔ | -0.09 pp | 8.61 % to 10.26 % | with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped |
| dropped packets named by sequence number | 82.18 % | 80.73 % ✔ | -1.45 pp | 78.65 % to 85.71 % | KNOWN (D-040): of the 100 bytes of a 106 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (100 - 18) / 100, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1886 dropped frames, four standard deviations on each side |
| names without a dropped frame | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames |

#### A corruption of 10 % with a payload of 200 bytes

`forward: { corrupt: 10% }`, offered 6400kbit with payloads of 200 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| frames dropped by the node as damaged | 9.75 % | 9.65 % ✔ | -0.10 pp | 8.91 % to 10.59 % | KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (242 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 236 / 242 = 9.75 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, to the packet |
| loss | 9.75 % | 9.65 % ✔ | -0.10 pp | 8.91 % to 10.59 % | with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped |
| dropped packets named by sequence number | 92.45 % | 92.59 % ✔ | +0.14 pp | 90.05 % to 94.85 % | KNOWN (D-040): of the 236 bytes of a 242 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (236 - 18) / 236, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1950 dropped frames, four standard deviations on each side |
| names without a dropped frame | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames |

#### A corruption of 10 % with a payload of 1,200 bytes

`forward: { corrupt: 10% }`, offered 38400kbit with payloads of 1200 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| frames dropped by the node as damaged | 9.95 % | 9.88 % ✔ | -0.07 pp | 9.10 % to 10.80 % | KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (1242 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 1236 / 1242 = 9.95 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, to the packet |
| loss | 9.95 % | 9.88 % ✔ | -0.07 pp | 9.10 % to 10.80 % | with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped |
| dropped packets named by sequence number | 98.56 % | 98.63 % ✔ | +0.07 pp | 97.48 % to 99.63 % | KNOWN (D-040): of the 1236 bytes of a 1242 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (1236 - 18) / 1236, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1990 dropped frames, four standard deviations on each side |
| names without a dropped frame | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames |

#### A corruption of 10 % with a loss of 10 %: the result tells the two apart

`forward: { loss: { model: random, p: 10% }, corrupt: 10% }`, offered 6400kbit with payloads of 200 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets lost in the queue | 10.00 % | 9.98 % ✔ | -0.03 pp | 9.15 % to 10.85 % | the loss comes first and drops 10 % of the packets in the queue; binomial, 20,000 packets, four standard deviations on each side |
| frames dropped by the node as damaged | 9.75 % | 9.84 % ✔ | +0.09 pp | 8.87 % to 10.64 % | the corruption works on the 90 % that the loss let through: 10 % x 236 / 242 = 9.75 % of them are dropped by the node; binomial, about 18,000 frames, four standard deviations on each side |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, to the packet |
| loss | 18.78 % | 18.83 % ✔ | +0.05 pp | 17.67 % to 19.88 % | the program sees both as loss: 0.1 + 0.9 x 0.0975 = 18.78 %; the account of the direction is what tells the two causes apart |

#### A corruption of 10 % in a stream without the checksum of UDP, payload of 64 bytes: the damaged packet arrives

`forward: { corrupt: 10% }`, offered 2048kbit with payloads of 64 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived damaged | 6.06 % | 6.22 % ✔ | +0.16 pp | 5.38 % to 6.74 % | KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (106 bytes with the Ethernet header), 64 of which are payload, and the two bytes of the length field of UDP count 2/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (64 + 2/8) / 106 = 6.06 %; binomial, 20,000 packets, four standard deviations on each side |
| frames dropped by the node as damaged | 3.18 % | 2.93 % ✔ | -0.25 pp | 2.68 % to 3.69 % | the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 2/8) / 106 = 3.18 %; binomial, 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet |
| loss | 9.25 % | 9.15 % ✔ | -0.10 pp | 8.42 % to 10.07 % | a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (106 - 8) / 106 = 9.25 %; binomial, 20,000 packets, four standard deviations on each side |

#### The same with a payload of 200 bytes

`forward: { corrupt: 10% }`, offered 6400kbit with payloads of 200 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived damaged | 8.28 % | 8.20 % ✔ | -0.09 pp | 7.50 % to 9.06 % | KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (242 bytes with the Ethernet header), 200 of which are payload, and the two bytes of the length field of UDP count 3/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (200 + 3/8) / 242 = 8.28 %; binomial, 20,000 packets, four standard deviations on each side |
| frames dropped by the node as damaged | 1.39 % | 1.41 % ✔ | +0.02 pp | 1.05 % to 1.73 % | the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 3/8) / 242 = 1.39 %; binomial, 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet |
| loss | 9.67 % | 9.60 % ✔ | -0.07 pp | 8.83 % to 10.51 % | a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (242 - 8) / 242 = 9.67 %; binomial, 20,000 packets, four standard deviations on each side |

#### The same with a payload of 1,200 bytes

`forward: { corrupt: 10% }`, offered 38400kbit with payloads of 1200 bytes for 5 s (20000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| packets that arrived damaged | 9.67 % | 9.59 % ✔ | -0.08 pp | 8.83 % to 10.51 % | KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (1242 bytes with the Ethernet header), 1200 of which are payload, and the two bytes of the length field of UDP count 5/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (1200 + 5/8) / 1242 = 9.67 %; binomial, 20,000 packets, four standard deviations on each side |
| frames dropped by the node as damaged | 0.27 % | 0.29 % ✔ | +0.01 pp | 0.12 % to 0.42 % | the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 5/8) / 1242 = 0.27 %; binomial, 20,000 frames, four standard deviations on each side |
| packets lost in the queue | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a corrupted packet arrives at the node, so the queue loses nothing |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet |
| loss | 9.94 % | 9.88 % ✔ | -0.06 pp | 9.08 % to 10.79 % | a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (1242 - 8) / 1242 = 9.94 %; binomial, 20,000 packets, four standard deviations on each side |

#### Three links in a row with delays of 10, 15 and 5 ms: the delays add up

a client, two routers and a server in a row: `l1: delay 10ms; l2: delay 15ms; l3: delay 5ms`, offered 2mbit with payloads of 1200 bytes for 6 s (1250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 30.000 ms | 30.017 ms ✔ | +0.017 ms | 30.000 ms to 30.500 ms | no packet can be faster than the three links together: 10 + 15 + 5 ms; the path and the two routers add well under 0.5 ms (0.016 ms for a link in the single-link case) |
| median delay | 30.000 ms | 30.026 ms ✔ | +0.026 ms | 30.000 ms to 31.000 ms | `netem` is exact to the microsecond on each link; 1 ms leaves room for the three hops and a busy shared runner (0.5 ms for one hop in the single-link case) |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | links without loss lose nothing, and a router that forwards drops nothing |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | fixed delays keep the order, also over three hops |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is accounted for link by link: what entered the first link is lost in a queue, dropped by a node or delivered at the end |

#### Three links in a row with losses of 3 % and 4 %: the losses compound

a client, two routers and a server in a row: `l1: loss 3%; l2: loss 4%; l3: none`, offered 8mbit with payloads of 1000 bytes for 6 s (6000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| loss | 6.88 % | 6.62 % ✔ | -0.26 pp | 5.57 % to 8.19 % | a packet is lost if either queue loses it, and the queues draw independently (a seed of their own each): 1 - 0.97 x 0.96 = 6.88 %; binomial, 6,000 packets, a standard deviation of 0.33 percentage points, four of them on each side |
| packets lost in the queue | 6.88 % | 6.62 % ✔ | -0.26 pp | 5.57 % to 8.19 % | the packets that the queues dropped, counted by the tally of each link and added, are the lost ones: nothing else takes a packet on the way; same model and bounds as the loss |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | the packets lost in the first queue, those lost in the second and those delivered add up to the packets sent into the first link, because what enters the second is what arrived at the router |
| median delay | 0.050 ms | 0.012 ms ✔ | -0.038 ms | 0.000 ms to 1.000 ms | links without a delay add only the path (0.016 ms for a link) and two routers; 1 ms for three hops and noise |

#### Three links in a row, the middle one limited to 2 mbit, offered 3 mbit: the slowest link sets the rate

a client, two routers and a server in a row: `l1: rate 10mbit; l2: rate 2mbit; l3: rate 10mbit`, offered 3mbit with payloads of 1000 bytes for 6 s (2250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| steady payload rate | 1.9194 mbit/s | 1.9200 mbit/s ✔ | +0.03 % | 1.8810 mbit/s to 1.9580 mbit/s | the rate of a path is the smallest rate on it, counted in Ethernet frames as for one link: 2 mbit of frames carry 1,000 bytes of payload in 1,042; 2 % for the edges of the one-second bins (about 240 packets each) |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the backlog at the slow link (about 800 packets) stays below its queue of 1,000, and the faster links before and after it hold nothing back, so nothing is lost |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | rate limits keep the order |
| packets not accounted for | 0 packets | 0 packets ✔ | +0.00 packets | 0 packets to 0 packets | every packet is accounted for link by link |

#### A link with an MTU of 600 and a loss of 5 %, packets of 1,400 bytes: a lost fragment loses the packet

a client, two routers and a server in a row: `mtu: 600; forward: loss 5%`, offered 8mbit with payloads of 1400 bytes for 6 s (4286 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| loss | 14.26 % | 13.84 % ✔ | -0.42 pp | 12.12 % to 16.40 % | a packet of 1,400 bytes is cut into 3 fragments by an MTU of 600 (576 bytes of UDP in a fragment: 1,408 bytes of UDP make 3), each is lost on its own with 5 %, and the packet is lost if any fragment is: 1 - 0.95^3 = 14.26 %; binomial, 4,285 packets, a standard deviation of 0.53 percentage points, four of them on each side |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | fragments of one packet are reassembled, and a link without a delay keeps the order of the packets |

#### A rate limit of 2 mbit over IPv6, offered 3 mbit: a frame is 20 bytes longer

a client, two routers and a server in a row: `ip: v6; forward: rate 2mbit`, offered 3mbit with payloads of 1000 bytes for 6 s (2250 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| steady payload rate | 1.8832 mbit/s | 1.8800 mbit/s ✔ | -0.17 % | 1.8630 mbit/s to 1.9030 mbit/s | `netem rate` counts the Ethernet frame, so 1,000 bytes of payload use 1,062 over IPv6 (1,042 over IPv4): 2 mbit carry 1,883 kbit of payload, 1.8 % less than over IPv4; 1 % for the edges of the one-second bins (about 235 packets each), narrow enough to tell the two apart |
| loss | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | the backlog (about 840 packets) stays below the queue of 1,000, so a rate limit alone loses nothing here |
| reordered packets | 0.00 % | 0.00 % ✔ | +0.00 pp | 0.00 % to 0.00 % | a rate limit keeps the order |

#### A delay of 40 ms with a pareto jitter of 10 ms

`forward: { delay: 40ms, jitter: { amount: 10ms, distribution: pareto } }`, offered 8mbit with payloads of 1000 bytes for 8 s (8000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 39.665 ms | 39.721 ms ✔ | +0.056 ms | 39.300 ms to 40.530 ms | MODEL: the 4,096 entries of `pareto.dist` (mean -0.034 amounts, deviation 0.820) at 40 ms + 10 ms x entry / 8192; the mean of 8,000 delays has a standard deviation of 0.092 ms, four of them on each side, and 0.5 ms of noise above |
| median delay | 36.796 ms | 36.844 ms ✔ | +0.048 ms | 36.545 ms to 37.550 ms | MODEL: the median of the table, -0.320 amounts; the median of 8,000 delays has a standard deviation of 0.063 ms, four of them on each side, and 0.5 ms of noise above |
| standard deviation of the delay | 8.197 ms | 8.105 ms ✔ | -0.092 ms | 7.607 ms to 8.990 ms | MODEL: the deviation of the table, 0.820 amounts; the estimate from 8,000 delays of this heavy-tailed shape (kurtosis 6.1) has a standard deviation of 0.148 ms, four of them on each side, and 0.2 ms above for noise |
| smallest delay | 33.334 ms | 33.342 ms ✔ | +0.008 ms | 33.329 ms to 33.850 ms | PHYSICS: the smallest entry is -5461, so no delay is below 40 ms - 10 ms x 0.667 = 33.334 ms (5 us for the rounding); the smallest of 8,000 delays is above the 0.26 % quantile (33.345 ms) with a chance of 1 - 1e-9, and 0.5 ms of noise above |
| largest delay | 79.999 ms | 80.023 ms ✔ | +0.024 ms | at least 79.999 ms | PHYSICS and MODEL: the largest entry is 32767, 4 amounts; 1.07 % of the entries are that large, so among 8,000 delays there is one at the top with a chance of 1 - 1e-37 and none is below 80.0 ms. There is no upper bound: one late packet is routine on a shared machine, the cap is judged on the 99.9th percentile |
| 99.9th percentile of the delay | 79.999 ms | 80.016 ms ✔ | +0.017 ms | 79.999 ms to 80.500 ms | PHYSICS and MODEL: no delay is above the largest entry, 80.0 ms, and 1.07 % of the entries are that large, so the ninth largest of 8,000 delays is at the cap with a chance of 1 - 1e-25; 0.5 ms of noise above, and nine packets would have to be late at once |

#### A delay of 40 ms with a paretonormal jitter of 10 ms

`forward: { delay: 40ms, jitter: { amount: 10ms, distribution: paretonormal } }`, offered 8mbit with payloads of 1000 bytes for 8 s (8000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 39.747 ms | 39.718 ms ✔ | -0.029 ms | 39.374 ms to 40.620 ms | MODEL: the entries of `paretonormal.dist` (mean -0.025 amounts, deviation 0.834); the mean of 8,000 delays has a standard deviation of 0.093 ms, four of them on each side, and 0.5 ms of noise above |
| median delay | 37.595 ms | 37.613 ms ✔ | +0.018 ms | 37.267 ms to 38.420 ms | MODEL: the median of the table, -0.240 amounts; the median of 8,000 delays has a standard deviation of 0.082 ms, four of them on each side, and 0.5 ms of noise above |
| standard deviation of the delay | 8.339 ms | 8.235 ms ✔ | -0.104 ms | 7.851 ms to 9.030 ms | MODEL: the deviation of the table, 0.834 amounts; the estimate from 8,000 delays has a standard deviation of 0.122 ms, four of them on each side, and 0.2 ms above for noise |
| smallest delay | 24.979 ms | 24.987 ms ✔ | +0.008 ms | 24.974 ms to 28.480 ms | PHYSICS: the smallest entry is -12305, so no delay is below 40 ms - 10 ms x 1.502 = 24.979 ms (5 us for the rounding); the smallest of 8,000 delays is above the 0.26 % quantile (27.98 ms) with a chance of 1 - 1e-9, and 0.5 ms of noise above |
| largest delay | 78.805 ms | 78.301 ms ✔ | -0.504 ms | at least 76.968 ms | MODEL: the largest of 8,000 delays is above the 99.74 % quantile (76.97 ms) with a chance of 1 - 1e-9. There is no upper bound: one late packet is routine on a shared machine (2.3 ms above the cap on a runner), the cap is judged on the 99.9th percentile |
| 99.9th percentile of the delay | 77.594 ms | 77.345 ms ✔ | -0.249 ms | 76.422 ms to 79.305 ms | PHYSICS and MODEL: no delay is above the largest entry, 31789, 78.805 ms (0.5 ms of noise above, and nine packets would have to be late at once); the ninth largest of 8,000 delays is the 99.9 % quantile of the table, 77.594 ms, and is above its 99.5 % quantile (76.422 ms, 0.51 % of the entries) with a chance of 1 - 1e-9 |

#### A delay of 100 ms with a jitter of 8 ms in the shape of a file of samples with a long tail

`forward: { delay: 100ms, jitter: { amount: 8ms, distribution: table, table: { file: delays.txt } } }`, offered 8mbit with payloads of 1000 bytes for 8 s (8000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| mean delay | 100.000 ms | 100.086 ms ✔ | +0.086 ms | 99.640 ms to 100.860 ms | MODEL: the table has the mean 0 and the deviation 1 by construction, so the delay has the mean of the `delay`; the mean of 8,000 delays has a standard deviation of 0.089 ms, four of them on each side, and 0.5 ms of noise above |
| median delay | 97.543 ms | 97.700 ms ✔ | +0.157 ms | 97.185 ms to 98.400 ms | MODEL: the samples are exponential (20 ms + 8 ms x Exp(1)), whose median is 0.307 deviations below the mean: 100 ms + 8 ms x (ln 2 - 1) = 97.545 ms; the median of 8,000 delays has a standard deviation of 0.089 ms, four of them on each side, and 0.5 ms of noise above |
| standard deviation of the delay | 8.000 ms | 7.897 ms ✔ | -0.103 ms | 7.500 ms to 8.700 ms | MODEL: the amount is the standard deviation of the shape, also where the entries had to be scaled down to fit 16 bits (the tail reaches 8 deviations, scale 2.024 and the amount goes to `netem` as 16.192 ms); the estimate from 8,000 delays of an exponential shape (kurtosis 9) has a standard deviation of 0.125 ms, four of them on each side, and 0.2 ms above for noise |
| smallest delay | 91.997 ms | 92.008 ms ✔ | +0.011 ms | 91.992 ms to 92.520 ms | PHYSICS: the smallest entry of the table is the first quantile of the samples, 0.0001 deviations above their minimum: 100 ms - 8 ms x 1.0003 = 91.997 ms (5 us for the rounding); 0.5 ms of noise above |
| largest delay | 164.750 ms | 164.762 ms ✔ | +0.012 ms | at least 139.760 ms | MODEL: the largest of 8,000 delays is above the 99.74 % quantile (139.76 ms) with a chance of 1 - 1e-9. A table that was not scaled would stop at 4 deviations (132 ms) and a `normal` one at 3.5. There is no upper bound: one late packet is routine on a shared machine, the cap is judged on the 99.9th percentile |
| 99.9th percentile of the delay | 147.260 ms | 143.619 ms ✔ | -3.641 ms | 134.000 ms to 165.250 ms | PHYSICS and MODEL: the largest entry is 32759 at scale 2.024, 8.09 deviations: no delay is above 164.75 ms (0.5 ms of noise above, and nine packets would have to be late at once); the ninth largest of 8,000 delays is the 99.9 % quantile of the exponential, 100 ms + 8 ms x (ln 1000 - 1) = 147.26 ms, and is above 134.0 ms (the 99.475 % quantile, 0.525 % of the delays) with a chance of 1 - 1e-9 |

#### Delay of a tail-drop queue of 100 packets that is overloaded by one fifth

`forward: { delay: 10ms, rate: 2mbit, queue: { discipline: fifo, limit: 100p } }`, offered 2.4mbit with payloads of 1200 bytes for 12 s (3000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 10.000 ms | 10.018 ms ✔ | +0.018 ms | 10.000 ms to 10.500 ms | PHYSICS: no packet is faster than the delay of the link; the first packet meets an empty queue and the rate stage lets it through, so it is 0.03 ms above (the cases of the rate and the delay) |
| median delay | 506.800 ms | 504.313 ms ✔ | -2.487 ms | 490.000 ms to 512.000 ms | MODEL: the offer is one fifth above the carrying capacity (2.4 mbit of payload against 1.932), so the queue of 100 packets is full from the third second on and a packet waits for the 100 frames in front of it, 100 x 1,242 bytes x 8 / 2 mbit = 496.8 ms, plus the delay of 10 ms: 506.8 ms. The bounds are 3 frames below (a queue that is cut a little by the packet in service) and 1 frame above (the limit and the frame in service) |
| 99th percentile of the delay | 506.800 ms | 506.775 ms ✔ | -0.025 ms | 490.000 ms to 512.000 ms | MODEL: the queue is full for four fifths of the packets, so the 99th percentile is the full queue too |
| loss | 16.20 % | 16.13 % ✔ | -0.07 pp | 15.20 % to 17.20 % | MODEL: of 3,000 packets sent the link carries 12 s x 201.3 per second = 2,415 and the queue holds 100 more when the sending ends: 1 - 2,515/3,000 = 16.2 %; one point for the packet at each edge and the moment the queue fills |

#### Delay of CoDel with a queue of 100 packets that is overloaded by one fifth

`forward: { delay: 10ms, rate: 2mbit, queue: { discipline: codel, limit: 100p } }`, offered 2.4mbit with payloads of 1200 bytes for 12 s (3000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 10.000 ms | 10.026 ms ✔ | +0.026 ms | 10.000 ms to 10.500 ms | PHYSICS: no packet is faster than the delay of the link; the first packet meets an empty queue and the rate stage lets it through, so it is 0.03 ms above (the cases of the rate and the delay) |
| median delay | 32.000 ms | 34.045 ms ✔ | +2.045 ms | 15.000 ms to 44.200 ms | MODEL: CoDel leaves the dropping state when a packet has waited less than the target (5 ms) and enters it again only when the delay has been above the target for an interval (100 ms); an offer that is 24.2 % above the capacity (2.4 against 1.932) adds 0.242 s of delay for each second, so the delay of the queue goes up by 24 ms in the interval: it moves between 5 ms and 29 ms, 17 ms on average. With the delay of 10 ms and the frame in service (5 ms) that is a mean of 32 ms and at most 44 ms; below 15 ms (delay, frame, target) CoDel would have dropped more than it needs. A FIFO of the same length holds 507 ms |
| 99th percentile of the delay | 100.000 ms | 99.587 ms ✔ | -0.413 ms | 15.000 ms to 200.000 ms | MODEL and FIDELITY: the first interval and the ramp of the drops before the control holds are the 1 % at the top, about 100 ms in the measurement (the first 30 packets). The bound is 200 ms, 40 % of the 507 ms that a full queue of 100 packets holds, which is what separates CoDel from a FIFO |
| loss | 19.40 % | 19.33 % ✔ | -0.07 pp | 16.20 % to 21.90 % | MODEL: the link carries 201.3 packets a second and CoDel drops what does not fit, so 1 - 2,415/3,000 = 19.5 %; no queue can deliver more than a full FIFO does (16.2 % lost, the lower end), and a CoDel that kept the link busy for less than 97 % of the time would lose more than 21.9 % |

#### Delay of FQ-CoDel with a queue of 100 packets that is overloaded by one fifth

`forward: { delay: 10ms, rate: 2mbit, queue: { discipline: fq-codel, limit: 100p } }`, offered 2.4mbit with payloads of 1200 bytes for 12 s (3000 packets)

| Quantity | Expected | Measured | Deviation | Allowed | Why |
| --- | --- | --- | --- | --- | --- |
| smallest delay | 10.000 ms | 10.024 ms ✔ | +0.024 ms | 10.000 ms to 10.500 ms | PHYSICS: no packet is faster than the delay of the link; the first packet meets an empty queue and the rate stage lets it through, so it is 0.03 ms above (the cases of the rate and the delay) |
| median delay | 32.000 ms | 34.053 ms ✔ | +2.053 ms | 15.000 ms to 44.200 ms | MODEL: CoDel leaves the dropping state when a packet has waited less than the target (5 ms) and enters it again only when the delay has been above the target for an interval (100 ms); an offer that is 24.2 % above the capacity (2.4 against 1.932) adds 0.242 s of delay for each second, so the delay of the queue goes up by 24 ms in the interval: it moves between 5 ms and 29 ms, 17 ms on average. With the delay of 10 ms and the frame in service (5 ms) that is a mean of 32 ms and at most 44 ms; below 15 ms (delay, frame, target) CoDel would have dropped more than it needs. A FIFO of the same length holds 507 ms |
| 99th percentile of the delay | 100.000 ms | 99.552 ms ✔ | -0.448 ms | 15.000 ms to 200.000 ms | MODEL and FIDELITY: the first interval and the ramp of the drops before the control holds are the 1 % at the top, about 100 ms in the measurement (the first 30 packets). The bound is 200 ms, 40 % of the 507 ms that a full queue of 100 packets holds, which is what separates CoDel from a FIFO |
| loss | 19.40 % | 19.33 % ✔ | -0.07 pp | 16.20 % to 21.90 % | MODEL: the link carries 201.3 packets a second and CoDel drops what does not fit, so 1 - 2,415/3,000 = 19.5 %; no queue can deliver more than a full FIFO does (16.2 % lost, the lower end), and a CoDel that kept the link busy for less than 97 % of the time would lose more than 21.9 % |

<!-- fidelity:end -->

## On the CI runners

The same twelve tests ran on the two images of the CI, each with the stable Rust and with 1.89 (run of the pull request that built the two qdiscs, 2026-10-30; the artifacts `fidelity-<os>-rust-<version>` of that run). The kernel of Ubuntu 22.04 is 6.8.0 and that of 24.04 is 7.0.0; the development machine has 6.12.113; all three use iproute2 6.15.0. Every one of the 4 × 23 quantities was inside its bounds. The cells show the range of the two jobs of an image.

| Case | Quantity | Expected | Development machine | Ubuntu 22.04 | Ubuntu 24.04 |
| --- | --- | --- | --- | --- | --- |
| delay 20 ms | smallest delay | 20.000 ms | 20.008 ms | 20.007 to 20.013 ms | 20.027 to 20.029 ms |
| delay 20 ms | median delay | 20.000 ms | 20.015 ms | 20.024 to 20.025 ms | 20.058 to 20.076 ms |
| delay 20 ms | loss | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| delay 20 ms | reordered packets | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| loss 5 % | loss | 5.00 % | 4.90 % | 4.90 % | 4.90 % |
| loss 5 % | median delay | 0.016 ms | 0.009 ms | 0.008 to 0.015 ms | 0.021 to 0.027 ms |
| loss 5 % | reordered packets | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| rate 2 mbit | steady payload rate | 1.9194 mbit/s | 1.9200 mbit/s | 1.9200 mbit/s | 1.9200 mbit/s |
| rate 2 mbit | loss | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| rate 2 mbit | reordered packets | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| uniform jitter 5 ms | smallest delay | 15.000 ms | 15.010 ms | 15.016 to 15.022 ms | 15.031 to 15.038 ms |
| uniform jitter 5 ms | 99th percentile of the delay | 24.900 ms | 24.901 ms | 24.894 to 24.943 ms | 24.943 to 25.019 ms |
| uniform jitter 5 ms | mean delay | 20.000 ms | 19.978 ms | 20.012 to 20.026 ms | 20.032 to 20.046 ms |
| uniform jitter 5 ms | standard deviation of the delay | 2.887 ms | 2.865 ms | 2.871 to 2.885 ms | 2.883 to 2.889 ms |
| uniform jitter 5 ms | reordered packets | 58.00 % | 58.28 % | 58.77 to 59.00 % | 58.52 to 58.73 % |
| normal jitter 5 ms | mean delay | 20.000 ms | 19.977 ms | 19.980 to 20.037 ms | 20.038 to 20.068 ms |
| normal jitter 5 ms | standard deviation of the delay | 5.000 ms | 5.061 ms | 5.009 to 5.044 ms | 4.981 to 5.002 ms |
| normal jitter 5 ms | 99th percentile of the delay | 31.600 ms | 31.838 ms | 31.863 to 31.994 ms | 31.672 to 31.914 ms |
| normal jitter 5 ms | reordered packets | 72.00 % | 72.05 % | 72.68 to 72.85 % | 71.72 to 72.02 % |
| normal jitter 5 ms + rate 10 mbit | mean delay | 41.000 ms | 41.059 ms | 41.020 to 41.086 ms | 41.067 to 41.266 ms |
| normal jitter 5 ms + rate 10 mbit | standard deviation of the delay | 5.000 ms | 5.008 ms | 4.937 to 4.952 ms | 5.040 to 5.146 ms |
| normal jitter 5 ms + rate 10 mbit | reordered packets | 29.00 % | 28.55 % | 28.49 to 29.39 % | 27.65 to 30.41 % |
| normal jitter 5 ms + rate 10 mbit | loss | 0.00 % | 0.00 % | 0.00 % | 0.00 % |

What the comparison shows:

- **The noise is small and goes one way.** The median of a fixed delay of 20 ms was 20.015 ms on the development machine, 20.024 to 20.025 ms on 22.04 and 20.058 to 20.076 ms on 24.04, always above 20 and far inside the 0.5 ms that the bound allows. The smallest delay (the one that cannot be raised by a single late packet) was 20.007 to 20.029 ms on the runners.
- **The rate and the count of lost packets did not change at all.** 1.9200 mbit/s of payload for a rate of 2 mbit (the Ethernet frame accounting) and 4.90 % (294 of 6,000 packets) lost for 5 % are the same on all five systems. That the seeded loss is the same *pattern* on the kernels 6.8, 6.12 and 7.0 was shown by spike S2; these tables show the same count.
- **The statistics scatter as statistics do.** The standard deviation of the normal jitter was 4.98 to 5.06 ms for a configured 5 ms in its own case and 4.94 to 5.15 ms in the case with a rate. The share of reordered packets was 58.3 % to 59.0 % (uniform) and 71.7 % to 72.9 % (normal); in the case with a rate 27.7 % to 30.4 %, against the 28.8 % of the model.
- **What differs is small.** Ubuntu 24.04 adds about 0.04 ms to the median of a fixed delay compared with 22.04, and the Rust version changes nothing that is measured here.

## What to know when you write a scenario

These are the findings behind the KNOWN rows and the case of the two qdiscs. They are properties of `netem`, the reference of the project, and every one is in [COVERAGE.md](COVERAGE.md) as well.

**The rate counts Ethernet frames.** `rate 2mbit` limits the frames of the link, and a frame is the payload plus 8 bytes of UDP, 20 of IP and 14 of Ethernet. The payload that gets through is `rate × payload / (payload + 42)`: with payloads of 1,000 bytes 1.919 mbit, with payloads of 100 bytes 1.408 mbit. The case measured 1.9200 mbit against a prediction of 1.9194. A scenario that wants 2 mbit of payload has to ask for a rate that is higher by that factor, or use bigger packets.

**A rate limit is a queue, and a queue is a delay.** When more is offered than the link carries, the packets wait, and the waiting counts as delay: in the rate case the delay grew to 3.4 seconds. The queue holds the packets until its limit (derived from the scenario, see [COVERAGE.md](COVERAGE.md), "Queue of the Kernel backend"), and the run has to wait for them before it can say they are lost (`--settle`, [CLI.md](CLI.md)).

**Jitter reorders packets.** `netem` gives every packet its own random delay and sends it at its own moment. If the jitter is larger than the gap between two packets, a later packet overtakes an earlier one. In the jitter cases a packet every millisecond with 5 ms of jitter gave 58 % reordered packets for a uniform and 72 % for a normal jitter (`reordered` in the result counts the packets that arrived after a packet with a higher number, RFC 4737). The distribution of the delays is the configured one: the mean was within 0.13 ms of the configured one and the standard deviation within 0.04 ms of 5 ms (over the runs made while writing this page), and the uniform spread reached 15.01 ms at the lower end. If your program cannot cope with reordering, the jitter has to be smaller than the gap between its packets.

**A rate and a jitter in one direction needed two qdiscs.** This is the finding that the fidelity tests were written to settle. The first version of the backend put everything of a direction into one `netem` qdisc, and the example `lte-like.yaml` (a delay of 40 ms, a normal jitter of 5 ms, a rate of 10 mbit) came out with a median of 42.5 ms. Measured one by one, the 0.99 ms of the rate was the serialisation of a frame, but the rest came from the combination: with a `rate`, `netem` sends a packet at the later of its own moment and the moment of the packet in front of it, so no packet can overtake another, the delays can only grow, and the jitter is cut off from below. The measured jitter was 4.1 ms instead of 5, the mean 42.7 instead of 41.0, the smallest delay 32 ms instead of 21 ms, and nothing was reordered. A model of this behaviour (`netem_enqueue`, a test in the file) gives 42.63 ms, 4.02 ms and no reordering. The scenario says something else: the packets wait for the rate in a queue, in order, and then each travels for its own delay. That is a rate stage followed by a delay stage, and the backend builds exactly that with two qdiscs, the rate in the root and the delay with its jitter in its child (`netem.rs`). Measured: 41.06 ms, 5.01 ms, the smallest delay 21 ms (the table of `netem` is cut off at four standard deviations), and 28.6 % reordered, against 41.01, 4.99 and 28.8 % from the model of the series. The case is in the tables above, and a canary judges the numbers of the old behaviour with its bounds: all three of them fail. A direction with only a jitter, only a rate, or a delay and a rate keeps its one qdisc, because there the two ways are the same.

**The delay is exact; the noise is small.** A fixed delay was 20.010 ms at the smallest and 20.017 ms at the median in the reference run, which is the delay plus 0.01 ms for the path (spike S4 measured 0.016 ms). On the CI runners the path costs a little more; the bounds allow 0.5 ms.

**The loss is the configured ratio, and with a seed it is the same packets.** 5 % of 6,000 packets: 294 lost (4.90 %), inside the 3.9 % to 6.1 % of four standard deviations. The pattern of losses is reproducible with a `seed` and a `tc` of iproute2 6.6.0 or newer ([RESULTS.md](RESULTS.md), "seed").

## What is not measured yet

- **The round trip time** (release 0.1 measures the one-way delay). The RFC 3550 jitter and the extent of reordering are measured against a model for the uniform jitter (case 4); the normal jitter has no such case yet.
- **A uniform jitter with a rate, and the loss together with the two stages.** The code is the same and the unit tests cover the arguments; there is no case that measures them yet.
- **Both directions at once, and several links.** Release 0.1 has one link. The cases impair the forward direction and measure the stream that goes that way.
- **Outages** and the **timeline** (changes over time, release 0.2) have tests that run a whole scenario and judge what the receiver saw (`tests/timeline.rs`, `tests/changes.rs`), not cases in the tables above: the window of an outage in sequence numbers and in time against the moments the run recorded, the packets that a link that is down refuses, the same seed twice. Each new impairment gets its case here in the release that brings it. The burst loss, the reordering, the duplication and the corruption have theirs, and the corruption also without the checksum of UDP (cases 19 to 21).
- **Packet rates above about 1,000 per second** and long runs. The cases send 200 to 1,000 packets per second for 6 to 8 seconds.
- **Other machines than the two CI images and the development machine.** The fidelity of a virtual machine with a busy host depends on the host.

## How to update this page

```bash
just fidelity
```

runs the fidelity tests and rewrites the block between the markers with what this machine measured. Commit the file only from a machine that you want to be the reference. The rest of the page is written by hand.

`just fidelity-test` runs the same tests and judges them without touching this page, and `just fidelity-test 1 2` and `just fidelity-test 2 2` run the two halves. The CI does that: the fidelity tests are most of the time of the tests, and each of them measures time and runs alone on its machine, so the CI cuts them into two halves for each image (Ubuntu 22.04 and 24.04, the current Rust) and runs the four jobs at once, next to the jobs of the other tests. The fidelity tests are not run with Rust 1.89 or as root: the network that `netem` makes does not depend on the version of the compiler or on how the namespaces were made.
