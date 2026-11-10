# Results, reports and replay

A run writes a directory. Its heart is `result.json`: everything the run measured, with the provenance that lets a reader judge it.

## What is in a result

| Part | What it tells |
| --- | --- |
| `provenance` | The version of netscenario, the time, the backend and the `tc` it used, the kernel, the processor, and the **precision of the timers** of the machine when the run began. |
| `seed` | The seed, whether the run drew it, and whether the same seed gives the same lost packets. If not, it says why. |
| `workloads` | For each stream: the counts, the delay percentiles, the jitter, the loss runs, the reordering, the throughput and the series over time. |
| `events` | Each step of the timeline as it was done: when it was due, when it was done, and for a step at a packet, which packet. |
| `expect` | The verdict of the checks of the file. |

The numbers are in SI units in the Python layer (seconds, bits per second, ratios from 0 to 1). In `result.json` times are nanoseconds. [Results reference](https://github.com/leonkrix/netscenario/blob/main/docs/RESULTS.md) defines every field.

## In Python

The scenario of the first run:

```yaml,scenario
{{#include ../../../scenarios/minimal.yaml:2:}}
```

Then:

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("minimal.yaml"), packets=True)
media = result["media"]

print(f"lost {media.lost} of {media.sent}")
print(f"p99 delay {media.delay.p99 * 1000:.2f} ms")
print("timers:", result.provenance.timers.sleeps[0].p99)

# One row for each packet, as a table that becomes a pandas or polars frame
packets = media.packets_table()
print(len(packets), packets.names)
```

The per-packet records, the series over time and the steps of the timeline are **tables** of plain Python values. `to_pandas()` and `to_polars()` turn them into frames; install `netscenario[pandas]` or `netscenario[polars]` for that. Nothing is imported unless you call it.

## Reports

`netscenario report <dir>` turns a result directory into a report. It reads the files of the directory and nothing else, so a report can be made on another machine, later.

```bash
netscenario report out --format md       # Markdown, for the summary of a CI job
netscenario report out --format junit    # JUnit XML, for the test report of a CI
netscenario report out --format html     # one HTML file with the plots
```

The Markdown report of a run begins with where and with what it was made, then the checks of the file, then a table of the workloads. The HTML is one file, readable offline.

## Replay

`scenario.json` in the directory is the scenario **as it ran**, with the files it read and the changes made from code. `replay` runs a recorded run again and compares the lost packets:

```bash
netscenario replay out
```

If the loss repeats from the seed, a replay loses exactly the same packets and the exit code is 0; if the packets differ, the exit code is 1 and the comparison says which. A run whose loss was not promised to repeat (a machine whose `tc` cannot apply a seed, an outage, a change made by a test) says so in `seed.not_reproducible_because`, and the replay compares what it can.

## Sets of runs

`--repetitions n` runs a scenario `n` times with derived seeds and writes `runset.json` with an interval for each metric. A claim about a set (`expect` with `repetitions`) is judged on that interval: [Statistics and checks](pytest-statistics.md).
