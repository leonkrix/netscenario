# Your first test

`pytest-netscenario` makes a network scenario a normal test. Install it next to the package (`pip install netscenario pytest-netscenario`); pytest finds it by itself.

## The scenario next to the test

A scenario is a file. Put it next to the test, here `video-call.yaml`: a video call over a Wi-Fi link that delays by 30 ms, with a jitter of 5 ms and 1 % loss from the caller to the callee.

```yaml,scenario
# file: video-call.yaml
version: 0
name: video-call
seed: 11
duration: 5s
topology:
  nodes: [caller, callee]
  links:
    - id: wifi
      between: [caller, callee]
      forward:                     # caller -> callee
        delay: 30ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
        rate: 20mbit
      reverse:                     # callee -> caller
        delay: 30ms
        rate: 20mbit
workloads:
  - id: video
    kind: udp-cbr
    from: caller
    to: callee
    rate: 1500kbit
    packet-size: 1200b
```

## The test

The fixture `netscenario` runs a scenario and gives you the result. Assertions are plain `assert` on numbers in SI units: seconds, bits per second, ratios from 0 to 1.

```python
# test_video_call.py
from pytest_netscenario import Runner


def test_the_video_arrives_in_time(netscenario: Runner) -> None:
    result = netscenario.run("video-call.yaml")   # looked for next to this file
    video = result["video"]

    assert video.loss_ratio < 0.05                # 1 % is configured: allow noise, catch a broken link
    assert video.delay is not None
    assert video.delay.p99 < 0.100                # seconds
```

Run it with `pytest`. When a number is missed the test fails and the report shows the numbers of the run. The scenario file is looked for next to the test file, then in the directories of the ini option `netscenario_scenarios`, then in the root of the project.

If you do not need the runner, a marker runs the scenario for the test and gives you the result:

```python
import pytest

import netscenario as ns


@pytest.mark.netscenario("video-call.yaml", seed=1)
def test_another_seed_with_the_marker(netscenario_result: ns.Result) -> None:
    assert netscenario_result["video"].sent > 0
```

## Machines that cannot run it

A test that needs namespaces is **skipped with the reason** on a laptop that cannot make them, and **fails** when the variable `CI` is set (GitHub Actions sets it). A missing permission on the CI can never hide as a green run.

## Building the scenario in code

A scenario does not have to be a file. The builder writes the same thing in Python, and everything it builds is checked by the same reader:

```python,run
import netscenario as ns

scenario = (
    ns.Scenario.builder("video-call", duration="5s", seed=11)
    .nodes("caller", "callee")
    .link(
        "wifi",
        "caller",
        "callee",
        forward=ns.Impairments(delay="30ms", loss="1%"),
        reverse=ns.Impairments(delay="30ms"),
    )
    .udp_cbr("video", "caller", "callee", rate="1500kbit", packet_size="1200b")
    .build()
)
result = ns.run(scenario)
assert result["video"].sent > 0
print(result["video"].loss_ratio)
```

Parameters of a scenario file (`params:`) can be set from a test with `params=` on the marker, the fixture or `ns.run`; the next chapters show them at work.
