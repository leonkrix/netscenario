# A network scenario as a pytest test

[`test_video_call.py`](test_video_call.py) runs [`video-call.yaml`](video-call.yaml) (a video stream of 1.5 mbit/s over a Wi-Fi link with 30 ms of delay, a jitter of 5 ms and 1 % loss, for five seconds) and asserts on the loss and on the 99th percentile of the delay.

```bash
pip install pytest-netscenario     # also installs netscenario, in exactly the same version
pytest -rA
```

```text
...
test_video_call.py::test_the_video_arrives_in_time
--------------------------------- netscenario ----------------------------------
video-call: kernel (user namespace, iproute2 6.15.0), seed 11
  video caller -> callee: sent 782, received 776, lost 0.77%, reordered 144, delay p50 30.9 ms, p99 41.6 ms
  result: /tmp/pytest-of-you/pytest-0/test_the_video_arrives_in_time0/netscenario/run-1-video-call/result.json
...
2 passed in 13.63s
```

The fixture `netscenario` runs a scenario file that it finds next to the test; the marker `@pytest.mark.netscenario("file.yaml", seed=...)` does the same for `netscenario_result`. The numbers of a run appear in the report of the test (with `-rA` for passing tests as well) and as properties in JUnit files. All of it is described in [docs/PYTEST.md](../../docs/PYTEST.md).

**Choosing the bounds.** The thresholds are far from the configured values (`loss < 0.05` for 1 % configured, `p99 < 100 ms` for 30 ms and a jitter of 5 ms): a test must fail when the link is broken, and not when a run is a little unlucky. The seed is in the file, so the lost packets repeat, but the delays vary a little from run to run. Statistical assertions with repetitions and confidence intervals come with release 0.2.

**On a machine that cannot create namespaces** the test is skipped with the reason, and on a CI (the variable `CI` is set) it fails, so that a test which never ran proves nothing ([docs/PERMISSIONS.md](../../docs/PERMISSIONS.md)).
