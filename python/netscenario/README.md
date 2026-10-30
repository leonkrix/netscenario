# netscenario

Declarative network scenarios for tests: describe a network (nodes, links, delay, jitter, loss, rate), run a program in it, measure per packet, and assert on the numbers.

```python
import netscenario as ns

result = ns.run(ns.Scenario.load("scenarios/lte-like.yaml"))
assert result["media"].delay.p99 < 0.150  # seconds
```

The wheel carries the extension module and the two programs that a run needs (`netscenario` and `netscenario-agent`), and puts the command `netscenario` on the `PATH`. It is for **Linux** (x86_64 and aarch64); a run creates network namespaces, which most machines allow without root, and `netscenario doctor` says what to change on one that does not.

The pytest plugin is the package `pytest-netscenario`.

- Documentation, the scenario format and the feature list: <https://github.com/leonkrix/netscenario>
- What a version promises: [docs/VERSIONING.md](https://github.com/leonkrix/netscenario/blob/main/docs/VERSIONING.md). The status is pre-alpha: a minor release may break things, and the changelog says so.

Licensed under either of Apache License 2.0 or MIT, at your option.
