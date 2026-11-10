# A link that changes when the test says

For someone whose test has to react to the program: wait until the application has connected, then make the link worse. `netscenario.start` starts the run and gives it back at once; `run.link("access").set(...)` changes the link while it goes ([docs/PYTHON.md](../../docs/PYTHON.md#changing-a-link-while-the-run-goes)).

```python
with netscenario.start("stream.yaml", live=["access"]) as run:
    run.wait_until(1.5)  # seconds into the measured phase
    run.link("access").set(loss="20%")
result = run.result()
```

The change is a **step of the timeline**, done between two packets: `step.packet` is the number of the first packet that met the new state. The recorded scenario holds the change at the slot of that packet, so `netscenario replay <dir>` (or `ns.replay`) does it there and loses the same packets. Two live runs differ, because the moment was the test's; the result says so in `seed.not_reproducible_because`.

Files: [stream.yaml](stream.yaml) (the scenario, with no timeline), [test_live_change.py](test_live_change.py) (the test). Needs a machine that can create network namespaces (`netscenario doctor`).
