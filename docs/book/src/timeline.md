# Changes over time

A link does not have to stay the same. The `timeline` lists **events** that happen at a moment of the measured phase: a rate that drops, an outage, a link that goes down and comes back. Each event is applied to one link, before the first packet its stream sends at or after the moment.

```yaml,scenario,run
# file: drop-and-outage.yaml
version: 0
name: drop-and-outage
seed: 3
duration: 9s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 20ms }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 4mbit, packet-size: 1200b }
timeline:
  - { at: 2s, link: access, set: { both: { rate: 2mbit } } }      # a rate limit from the 2nd second
  - { at: 5s, link: access, outage: 1s }                           # nothing is delivered for a second
  - { at: 7s, link: access, set: { both: { rate: null } } }       # `null` takes the limit away again
measure:
  series: { bin: 500ms }
```

An event does exactly one of four things:

| Key | What it does |
| --- | --- |
| `set` | Changes values of the link from this moment on. It is a merge patch: a key that is not written stays, a written one replaces the old value, and `null` takes the impairment away. |
| `outage` | Nothing is delivered for this long, also not a packet that was on its way when it began, and the sender notices nothing. |
| `down` | The interface goes down at both ends: the sender gets errors, which are counted as refused packets, and the queues are dropped. |
| `up` | The interface comes up again. |

An event can repeat with `every`, until `until` or `count` times. A flapping link is two lines:

```yaml
timeline:
  - { at: 5s, link: access, outage: 400ms, every: 10s, until: 55s }
```

## How long did it take to recover?

Because the run records a series over time, it can answer the question that matters after an event: how long until the stream is back at its level. Each event that disturbs a stream has a **recovery**:

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("drop-and-outage.yaml"))
for recovery in result["media"].recoveries:
    print(recovery.kind, recovery.link, recovery.recovered, recovery.time)
```

The level of a bin is the share of the packets sent in it that arrived (the goodput for TCP). The resolution of the time to recover is one bin, set with `measure.series.bin` or `--bin`. The baseline, the window and the share that counts as recovered are options of the same section; the [results reference](https://github.com/leonkrix/netscenario/blob/main/docs/RESULTS.md) defines them.

## When the test decides

The moment of an event in the file is fixed in advance. If a test needs to make the change when something happens (when the app has connected), it changes the link while the run goes, and the result records that change as a step too: [Change the link while the test runs](pytest-live.md).

## What a result tells about the steps

Each step is in `events` of the result with the moment the file asked for (`scheduled_ns`), the moment the backend had done it (`applied_ns`), and for a step made at a packet, the number of that packet. `applied_ns - scheduled_ns` is how late the step was, which on a quiet machine is a fraction of a millisecond ([Noisy machines and timers](timers.md)).
