# pytest-netscenario

A pytest plugin for [netscenario](https://github.com/leonkrix/netscenario): run a network scenario as a test and assert on loss, delay and throughput.

```bash
pip install pytest-netscenario     # also installs netscenario, in exactly the same version
```

```python
# test_video_call.py, next to video-call.yaml
def test_the_video_arrives_in_time(netscenario):
    video = netscenario.run("video-call.yaml")["video"]
    assert video.loss_ratio < 0.05
    assert video.delay.p99 < 0.100  # seconds
```

The plugin gives the fixture `netscenario` (a runner that finds the scenario file next to the test), the marker `netscenario(...)` with the fixture `netscenario_result`, a guard that skips (or, on a CI, fails) when the machine cannot create network namespaces, and a section in the report with the numbers of each run. It needs **Linux** with the tools and rights that `netscenario doctor` checks ([permissions](https://github.com/leonkrix/netscenario/blob/main/docs/PERMISSIONS.md)).

A complete example with its scenario file and its output: [examples/python-test](https://github.com/leonkrix/netscenario/tree/main/examples/python-test). Documentation: [docs/PYTEST.md](https://github.com/leonkrix/netscenario/blob/main/docs/PYTEST.md). The status is pre-alpha.

Licensed under either of Apache License 2.0 or MIT, at your option.
