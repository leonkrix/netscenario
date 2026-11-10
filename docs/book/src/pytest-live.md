# Change the link while the test runs

Sometimes the moment of a change is the test's: "when the app has connected, make the link worse". A scenario file fixes its changes in advance (see [Changes over time](timeline.md)); a test can also change a link **while the run goes**.

The link has to be declared **live** when the run starts. The queues of a direction are built from every state it goes through and are not rebuilt later, so the run needs to know which links the code may change and builds those so that any state fits. A link that is not declared live is refused.

```yaml,scenario,run
# file: stream.yaml
version: 0
name: live-stream
duration: 4s
seed: 7
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: 30ms, rate: 4mbit }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1000b }
```

```python,run
import netscenario as ns

with ns.start("stream.yaml", live=["access"], packets=True) as run:
    run.wait_until(1.5)                      # seconds into the measured phase
    steps = run.link("access").set(
        forward={"delay": "120ms", "loss": {"model": "random", "p": "20%"}}
    )
result = run.result()

(step,) = steps
print("the packet that met the old state:", step.packet)
assert step.live and step.packet is not None
assert result["media"].delay.max > 0.120
```

`start` returns at once with a `Run`. `wait_until` waits for a moment of the measured phase, `run.link(...).set(...)` changes a link (the keys are those of a `set` in the file), and `down()` and `up()` take it away and give it back. Leaving the `with` waits for the end of the run, and `run.result()` gives the result.

## What the result records

A change from code is **a step of the timeline**, made between two packets: the sender is held before its next packet, the step is done, and the sender goes on. The step has `live=True` and `packet`, the number of the first packet that met the new state, so the result says exactly where the change landed.

The moment is the test's, so two live runs differ, and the result says so (`seed.not_reproducible_because`). What it records is the scenario with the change as an event at the slot of that packet, and **a replay of that directory does the change at the same packet and loses the same packets**:

```python
replay = ns.replay(result.out_dir)
assert replay.reproduced
```

## With `asyncio`

The same run handle works from async code and never blocks the loop. Cancelling the task cancels the run and waits until the network is torn down, so no thread is left behind:

```python,run
import asyncio

import netscenario as ns


async def main() -> None:
    async with await ns.start_async("stream.yaml", live=["access"]) as run:
        await run.wait_until(1.0)
        await run.link("access").down()
        await asyncio.sleep(0.5)
        await run.link("access").up()
    result = await run.result()
    print("lost:", result["media"].lost)


asyncio.run(main())
```

`run.events()` is an async iterator over the events of the run (when the network is built, when the workloads start, ...). `run_async(...)` is the one-call form for a scenario that is only to be awaited. A change that the backend cannot do exactly (a state its queues do not fit) is refused to the caller as an error, before anything is touched, and the run goes on.
