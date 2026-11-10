# Your first run

The smallest scenario has two nodes, a link between them and one stream of packets from one to the other. Save it as `minimal.yaml`:

```yaml,scenario,run
{{#include ../../../scenarios/minimal.yaml:2:}}
```

`validate` reads the file and checks it without building anything:

```bash
netscenario validate minimal.yaml
```

```text
ok: minimal (minimal.yaml)
  2 nodes, 1 link, 1 workload, duration 5s
  needs: point-to-point, ipv4, udp-generators
```

`run` builds the two nodes in their own namespaces, joins them with the link, sends the stream for five seconds, and measures every packet:

```bash
netscenario run minimal.yaml --seed 1
```

```text
running minimal for 5s ...
netscenario run: minimal
  backend kernel (user namespace, iproute2 6.15.0, reno cubic)
  seed 1

workload media  client -> server  1 mbit/s, 1200 byte packets
  sent 521  received 521  lost 0 (0.00 %)
  delay  min 0.0 ms  median 0.0 ms  p95 0.0 ms  p99 0.0 ms  max 0.1 ms
  jitter  mean 0.004 ms  last 0.003 ms  max 0.009 ms  (RFC 3550)
  throughput per second  1.01 mbit/s  998.4 kbit/s  998.4 kbit/s  998.4 kbit/s  998.4 kbit/s

result: netscenario-results/minimal-20261110T205757Z/result.json
```

The link has no impairment, so nothing is lost and the delay is the machine's own. Make the link worse and run it again. Replace the link with one that has 40 ms of delay, a jitter of 5 ms and 1 % loss in the direction from the client to the server:

```yaml,scenario,run
# file: worse.yaml
version: 0
name: worse
seed: 5
duration: 5s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:
        delay: 40ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
```

The delay is now near 40 ms, a few packets are lost, and the same `seed` gives the same lost packets on the same machine. A run that has no `seed` draws one and prints it, so a surprising run can be repeated with `--seed`.

## From Python

The same run from Python gives a result with typed fields, in SI units (seconds, bits per second, ratios from 0 to 1):

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("worse.yaml"))
media = result["media"]
print(media.sent, media.received, media.lost)
print(f"median delay {media.delay.p50 * 1000:.1f} ms")
assert 0.035 < media.delay.p50 < 0.060
```

## What is in the result directory

`result.json` is the result: the provenance, the seed and, for each workload, the counts, the delay percentiles, the series over time and everything the other chapters mention. Next to it are the raw records of the run and `scenario.json`, the scenario as it ran, with which a run can be replayed. [Results, reports and replay](results.md) goes through them.
