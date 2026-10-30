# pytest-netscenario

A pytest plugin for [netscenario](https://github.com/leonkrix/netscenario): test a program on a described network.

```python
import pytest


@pytest.mark.netscenario("scenarios/lte-like.yaml")
def test_media_delay(netscenario_result):
    assert netscenario_result["media"].delay.p99 < 0.150  # seconds
```

The plugin gives the fixture `netscenario` (a runner), the marker `netscenario(...)`, a guard that skips (or, on a CI, fails) when the machine cannot create network namespaces, and a section in the report with the numbers of each run. It needs **Linux** and installs the package `netscenario`, in exactly the same version.

Documentation: [docs/PYTEST.md](https://github.com/leonkrix/netscenario/blob/main/docs/PYTEST.md). The status is pre-alpha.

Licensed under either of Apache License 2.0 or MIT, at your option.
