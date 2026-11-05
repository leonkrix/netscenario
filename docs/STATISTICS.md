# Repetitions and confidence intervals

A single run is one sample: the same scenario with another seed loses other packets, and a delay percentile moves a little from run to run. `netscenario run --repetitions n` runs the scenario `n` times, one after the other, and says for each metric where its mean (or its median) lies as far as `n` runs can tell (ASR-02, ASR-03; the decisions are [D-042](decisions/D-042.md) and [D-074](decisions/D-074.md)).

```text
netscenario run scenarios/lte-like.yaml --repetitions 10 --out results/lte
```

In Python the same is `ns.run(scenario, repetitions=10)`, which returns a `RunSet` ([PYTHON.md](PYTHON.md#repetitions-and-confidence-intervals)), and in pytest the marker `repetitions=` with the fixture `netscenario_runs` ([PYTEST.md](PYTEST.md)).

## What is written

```text
results/lte/
  run-001/  result.json, flow-0.records, series-media.jsonl, ...   (a whole run, see RESULTS.md)
  run-002/
  ...
  runset.json                                                       (the set)
```

- **The seeds.** Run number `i` (from 0) uses the seed `derive_repetition_seed(set seed, i)`. The seed of the set is the `seed` of the scenario or `--seed`; without either the set draws one and prints it (`repeat the set with --seed 42`). The derivation is pinned like the seeds of the links, so a published set can be repeated by any later version. The runs of one set do not repeat each other's loss patterns.
- **Every run of the set can be replayed** with `netscenario replay results/lte/run-003` (each directory has its `scenario.json`, [CLI.md](CLI.md#replay)).
- **The runs are one after the other.** They share the machine, and a run beside another would measure the other.
- **A set ends with the first failure** (a run that fails, Ctrl-C, a run that was cut short). The finished runs stay in their directories and there is no `runset.json`: an interval from fewer runs than were asked for would be a made-up number.

## `runset.json`

JSON with `format` `netscenario-runset` and `version` 0 (a field may change its meaning in a minor release before 1.0, as for the result). The fields:

| Field | What it is |
| --- | --- |
| `scenario` | The name, and the hash of the scenario **without its seed**, because every run has its own. |
| `provenance` | Where and with what the first run was made. |
| `seed` | `value` and `drawn`. |
| `repetitions` | How many runs. |
| `confidence` | `statistic` (`mean` or `median`), `level` (0.95), `resamples` (10,000) and `method` (below). |
| `runs` | For each run: `index`, `seed`, `directory` (relative) and `loss_reproducible` (from its `result.json`). |
| `metrics` | For each metric that **every** run has, by name: `values` (one per run), `mean`, `median`, `min`, `max`, `stddev` (divided by n - 1) and `interval`. |
| `partial` | Metrics that only some runs have (`present_in` of `of`). They have no interval. |

`interval` is `{statistic, estimate, low, high, level, method, tail, resamples, runs}`: `estimate` is the mean (or median) of the values, `low` and `high` the ends, `tail` the probability left out at each end.

### The metrics

The names are the same in the library (`netscenario_core::metric_values`), the command line, Python and the reports. Times are in seconds, rates in bits per second, ratios from 0 to 1.

| Name | What |
| --- | --- |
| `<workload>.sent`, `.received`, `.lost`, `.duplicates`, `.reordered` | counts of packets |
| `<workload>.loss_ratio` | `lost / sent` |
| `<workload>.delay.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | one-way delay; not there if no packet arrived |
| `<workload>.jitter.last`, `.mean`, `.max` | interarrival jitter (RFC 3550); not there for fewer than two arrivals |
| `<workload>.throughput.mean` | the payload that arrived, over the duration of the workload |
| `<workload>.loss_runs.longest`, `.mean` | the runs of lost packets, in packets |
| `<workload>.round_trip.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | a UDP workload with `echo: true`: the round trip of the packets that came back (D-087); not there if none did |
| `<workload>.echo.received`, `.lost`, `.loss_ratio` | a UDP workload with `echo: true`: the packets that came back, those that did not (lost on the way there or back), and their share |
| `<tcp>.goodput.mean` | a TCP workload: the payload that arrived over the transfer, from the first connect to the last end (D-086) |
| `<tcp>.completion` | the time from the first connect to the end of the last connection that did its work; not there if none did |
| `<tcp>.bytes_sent`, `.bytes_received`, `.connections`, `.failed_connections`, `.retransmitted_segments` | counts |
| `<tcp>.rtt.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | the smoothed round trip time of the kernel; not there without samples of `TCP_INFO` |
| `<tcp>.sent`, `.answered`, `.failed` | `request-response`: the requests |
| `<tcp>.latency.min` ... `.max`, `<tcp>.round_trip.min` ... `.max` | `request-response`: the time from the moment a request was due, and from the moment it was sent, to the last byte of its response; answered requests only |
| `<program>.<metric>.last`, `.min`, `.max` | a metric that a program of your own reported, in the unit of the program |

A metric that is the same in every run (the packets sent by a constant stream) has a point as its interval.

## The method

The interval is a **seeded bootstrap**: the `n` values are drawn again and again with replacement (10,000 times, each draw `n` values), the mean (or the median) is taken of each draw, and the interval is the stretch that holds the middle 95 % of those.

- **The mean uses the expanded percentile interval** (Hesterberg, "What teachers should know about the bootstrap", 2015). The plain percentile interval is too narrow for a few runs, because the bootstrap draws from the values and not from the distribution, so it shows less spread than the distribution has. The correction moves the ends outwards: the probability left out at each end is the normal probability beyond `t * sqrt(n / (n - 1))`, where `t` is the critical value of Student's t with `n - 1` degrees of freedom. For 5 runs that is 0.095 % at each end instead of 2.5 %, for 30 runs 1.9 %, for 100 runs 2.3 %. For normal data the result has the width of the classical t interval.
- **The median uses the plain percentile interval**, whose coverage is already near the level (below).
- **The random numbers** are the published `xoshiro256++` of the schedules, seeded from the seed of the set and the name of the metric. The same set gives the same interval on every machine and in every version, and the interval of a metric does not change when another metric appears.
- **It refuses what it cannot answer honestly:** fewer than 5 or more than 1,000 repetitions, a level outside 0 to 1, fewer than 1,000 draws, a value that is not a finite number.

## What the interval promises, and what it does not

The coverage is the share of intervals, made from fresh samples of a distribution whose mean is known, that hold the true value. It was measured on simulated data, 20,000 intervals for each cell and 1,000 draws for each interval (the standard error of a share is about 0.2 to 0.3 percentage points). The measurement is the ignored test `measure_coverage` of `netscenario_core::bootstrap` (`cargo test --release -p netscenario-core --lib bootstrap::tests::measure_coverage -- --ignored --nocapture`), and the tests of the module keep the shares in bounds derived from them.

| Runs n | Mean, uniform data | Mean, skewed data (exponential) | The same, plain percentile (no correction) | Median, skewed data |
| ---: | ---: | ---: | ---: | ---: |
| 5 | 91.5 % | 86.3 % | 78.1 % | 93.8 % |
| 10 | 95.0 % | 90.6 % | 86.2 % | 93.3 % |
| 20 | 95.0 % | 92.4 % | 90.3 % | 93.7 % |
| 30 | 95.2 % | 92.8 % | 91.5 % | 94.0 % |
| 50 | 94.8 % | 93.6 % | 92.6 % | 94.4 % |

The target is 95 %. Read the table like this:

- **For few runs the interval is rough.** An interval from 5 to 10 runs of a skewed metric holds the true mean 86 to 91 % of the time. The usable minimum is 5; for a metric with a heavy tail (a high percentile of the delay, `max`) use 20 runs or more.
- **A heavy tail is hard for every method.** A percentile of the delay is a number made from the few packets in the tail of one run, and it varies much between runs. Its mean over the runs is stable; the `max` of the runs is not, and the interval of a maximum says little.
- **The interval is about the mean (or the median) of the metric over runs, for this scenario on this machine.** It says nothing about another machine, another kernel or another load, and it is no prediction for the next run: a single run lies in a wider range than the mean does (that range is `stddev`).
- **Runs that are not independent break it.** Every run of the set starts a new network, and a different seed gives different random draws, but effects of the machine (a busy neighbour on a shared runner) can be the same for the whole set. The `provenance` and the spread between the runs show it; the interval does not.
- **The loss of a run may not be reproducible** (a program that sends, an outage, corruption): `loss_reproducible` is in the list of the runs and the summary warns. The set is valid then, but a single run of it cannot be repeated from its seed.

### What "stable" means

An assertion on an interval is stable if repeating the whole set with other seeds gives the same verdict, except in the rare case of a threshold that lies inside the interval. Compare the **end** of the interval that is on the dangerous side with the threshold (`loss_ratio.high < 0.05`, `delay.p99.high < 0.080`), and choose a threshold with room: a threshold that lies near the mean fails about half of the time however many runs are made, because the claim is then neither true nor false for the scenario. The width of the interval falls with the square root of the number of runs, so four times the runs give half the width.
