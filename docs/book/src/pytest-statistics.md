# Statistics and checks

One run is one sample. A claim such as "the loss stays below 5 %" is stable only if the end of the interval that matters is far from the threshold, so netscenario makes the set of runs a first-class thing.

## Repetitions

`repetitions=n` runs the scenario `n` times (5 to 1000) with seeds derived from one seed, and gives a set with an interval for each metric:

```python
from pytest_netscenario import Runner


def test_the_loss_is_low_with_a_confidence_interval(netscenario: Runner) -> None:
    runs = netscenario.run("video-call.yaml", repetitions=5)
    loss = runs.metric("video.loss_ratio")

    assert loss.ci95_high < 0.05                  # 1 % is configured: the claim is "below 5 %"
    assert runs.metric("video.delay.p99").ci95_high < 0.100
```

`ci95_low` and `ci95_high` are the ends of the 95 % interval around the mean (or the median, with `statistic="median"`); `confidence=` changes the level. A bound is not picked by eye: for a count, take four standard deviations of its model each side; for a delay, take its exact physical lower bound and leave room only upward.

## Checks in the file

Instead of asserting in Python you can write what the scenario must show in the file itself, in the section `expect`. The core judges it, prints each check with its value and how far it is from the bound, and the same verdict is in the result and in every report:

```yaml,scenario,run
# file: expect.yaml
version: 0
name: expect
seed: 21
duration: 4s
params:
  rtt: 40ms
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: "{rtt / 2}", rate: 10mbit, loss: { model: random, p: 0.5% } }
      reverse: { delay: "{rtt / 2}", rate: 10mbit }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
expect:
  checks:
    - name: the stream arrives at its rate
      metric: media.throughput.mean
      value: ">= 1.9mbit"
    - name: the delay is that of the link
      metric: media.delay.p99
      value: "< {rtt / 2 + 10ms}"
    - name: the link does not lose more than it was told to
      metric: media.loss_ratio
      value: "< 2%"
```

`netscenario run expect.yaml` exits with 1 when a check does not hold, and the pytest plugin fails a test that ran such a file, with the checks that did not hold, also when its body asserts nothing. The parameter `rtt` has a default and can be set per run, which lets one file serve several tests:

```python
def test_a_longer_path(netscenario):
    result = netscenario.run("expect.yaml", params={"rtt": "100ms"})
    assert result.expect is not None
    print(result.expect.explain())                # which checks hold
```

For a set of runs the bounds are on the statistics of the set (`mean`, `median`, `min`, `max`, `ci95-low`, `ci95-high`); the file [`scenarios/expect-set.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/expect-set.yaml) shows a claim on the worst of ten runs.

## Judge only what the link guarantees

The delay of a stream over a link of 20 ms is never below 20 ms, and a test may say so exactly. The rest is noise and wants room: the machine wakes late, the CI is loaded, a burst of loss is not a mean. [Noisy machines and timers](timers.md) says how to see how much room a machine needs.
