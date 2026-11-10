# One link

The simplest network is two nodes and one link. Most real problems, a bad mobile connection or a long satellite hop, are one link, and most tests need no more.

A link has two directions that need not be alike. This one is a mobile access link: more delay and a lower rate going up, a higher rate going down. A stream goes each way, and the up stream is **echoed** so the round trip is measured:

```yaml,scenario,run
# file: mobile.yaml
version: 0
name: mobile
seed: 42
duration: 5s
topology:
  nodes: [phone, server]
  links:
    - id: radio
      between: [phone, server]
      forward:                       # phone -> server, up
        delay: 40ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
        rate: 10mbit
      reverse:                       # server -> phone, down
        delay: 25ms
        rate: 20mbit
workloads:
  - { id: up, kind: udp-cbr, from: phone, to: server, rate: 1mbit, packet-size: 800b, echo: true }
  - { id: down, kind: udp-cbr, from: server, to: phone, rate: 4mbit, packet-size: 1200b }
```

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("mobile.yaml"))
up, down = result["up"], result["down"]

# The one-way delay of a stream is what the link says (both ends read the same clock).
# The down stream has no jitter, so no packet is faster than the delay of the link;
# the up stream has a jitter of 5 ms, so its delays are spread around 40 ms
assert down.delay.min >= 0.025
assert 0.030 < up.delay.p50 < 0.050

# The echo is the round trip: up and down together, and lost if either way lost it
print(f"round trip median {up.echo.round_trip.p50 * 1000:.0f} ms")
```

Three things this shape teaches:

- **A bound on a delay has an exact lower end, where the link has no jitter.** No packet arrives before the delay of the link (the down stream above), and a test may say so without any room; only the upper end needs room for the machine. With a jitter the delays are spread around the value (the up stream), and the bound is on the percentiles.
- **Loss is per direction.** The up stream loses 1 %; the down stream loses nothing. A stream does not see the loss of the other way unless it echoes.
- **`forward` is from the first node of `between`.** Put the terminal first and `forward` is up, the convention of the profile library.

When you do not know the numbers of your link, start from a profile (`profile: lte-good`, [What a link can do](links.md)) and override what you know.
