# netscenario

Declarative network scenarios for tests: describe a network (nodes, links, delay, jitter, loss, rate), send traffic through it, measure per packet, and assert on the numbers.

```bash
pip install netscenario
netscenario doctor        # what this machine can do, and how to fix what it cannot
```

```python
import netscenario as ns

scenario = ns.Scenario.from_yaml("""
version: 0
name: video-call
seed: 11
duration: 5s
topology:
  nodes: [caller, callee]
  links:
    - id: wifi
      between: [caller, callee]
      forward: { delay: 30ms, jitter: { amount: 5ms, distribution: normal }, loss: { model: random, p: 1% } }
      reverse: { delay: 30ms }
workloads:
  - { id: video, kind: udp-cbr, from: caller, to: callee, rate: 1500kbit, packet-size: 1200b }
""")

result = ns.run(scenario, out="video-call-result")
video = result["video"]
print(
    f"{video.sent} packets sent, {video.loss_ratio:.1%} lost, p99 delay {video.delay.p99 * 1000:.0f} ms"
)
assert video.delay.p99 < 0.100  # seconds
```

The wheel carries the extension module and the two programs that a run needs (`netscenario` and `netscenario-agent`), and puts the command `netscenario` on the `PATH`. It is for **Linux** (x86_64 and aarch64) with the tools `ip`, `tc`, `unshare`, `nsenter` and `prlimit` (packages `iproute2` and `util-linux`). A run creates network namespaces, which Debian and Ubuntu 22.04 allow without root; Ubuntu 24.04 needs one setting, and a container needs more: [permissions](https://github.com/leonkrix/netscenario/blob/main/docs/PERMISSIONS.md). If something does not work, start with `netscenario doctor` and the [troubleshooting guide](https://github.com/leonkrix/netscenario/blob/main/docs/TROUBLESHOOTING.md).

The pytest plugin is the package `pytest-netscenario`.

- Documentation, the scenario format and the feature list: <https://github.com/leonkrix/netscenario>
- What a version promises: [docs/VERSIONING.md](https://github.com/leonkrix/netscenario/blob/main/docs/VERSIONING.md). The status is pre-alpha: a minor release may break things, and the changelog says so.

Licensed under either of Apache License 2.0 or MIT, at your option.
