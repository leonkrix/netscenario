# Noisy machines and timers

A packet leaves at the slot the plan gives it only as exactly as the machine wakes a sleeping core, and a step of a timeline is done only as fast as `tc` answers. How late that is depends on the processor, its idle states, the timer slack of the process and the load. No figure can be promised, so netscenario **measures the machine and reports it**.

## The report

```bash
netscenario doctor --timers
```

```text
Timers
  ok    clock                  CLOCK_MONOTONIC from tsc: a reading costs 30 ns, the smallest step seen is 20 ns
  ok    sleep of 200 us        late by 54.0 us (median), 59.0 us (99th percentile), 64.7 us (largest) in 1000 sleeps
  ok    sleep of 1 ms          late by 54.5 us (median), 60.6 us (99th percentile), 84.6 us (largest) in 300 sleeps
  info  timer slack            50.0 us (the kernel may delay a timer this much to batch it with others); governor: performance
  info  load                   load average 10.30 of the last minute on 16 processors, 1 tasks runnable (this one included)
  ok    change of a qdisc      31.1 us median, 58.0 us (99th percentile), 70.4 us (largest) in 500 changes of a netem through the `tc` that lives for a run
```

It takes about two seconds and makes its own namespace for the last line. A late timer is a **warning**, never a failure: the exit code is the one of the rest of `doctor`, and a run is not refused because of a timer.

| Line | What it means for your results |
| --- | --- |
| clock | Every packet reads the clock. A slow source (`hpet`, `acpi_pm`) limits the rate a sender can keep. |
| sleep of 200 µs / 1 ms | How much later than asked a sleep woke up. A sender sleeps until 150 µs before the slot of a packet and **spins for the rest**, so a 99th percentile above 150 µs means packets may leave late. A median of about 50 µs is usual: it is the timer slack of the process. |
| timer slack | How far the kernel may delay a timer to batch it with others, and the governor of the first processor. A `performance` governor wakes sooner than `powersave`. |
| load | More load than processors makes every timer here later than on an idle machine. |
| change of a qdisc | What a timeline step waits for. About 0.03 ms through the `tc` that lives for the run; 2 ms and more means the steps will be late by that much. |

## What a run records

Every run measures the same for about 40 ms just before it builds the network and records it in `provenance.timers` of the result. A result you get from somebody else's CI says on what kind of machine it was made, and the Markdown and HTML reports have a line for it:

```python,run
import netscenario as ns

report = ns.doctor(timers=True)
for sleep in report.timers.sleeps:
    print(f"sleep of {sleep.asked * 1e6:.0f} us: p99 {sleep.p99 * 1e6:.0f} us late")
assert report.ok
```

A run measures few sleeps, so its 99th percentile is close to the largest and each entry says how many there were.

The precision of an **idle** machine is one thing; what the run actually met is another, and the result has both. Each sender reports `max_lateness`, the most it was late for a packet, and each step in `events` has the moment it was due and the moment it was done (`applied_ns - scheduled_ns`).

## Designing bounds for a noisy machine

- A **lower** bound is physical and exact: no packet arrives before the delay of the link. Keep it tight.
- An **upper** bound needs room: add the lateness the machine showed (`max_lateness`, or the 99th percentile above) and then some.
- Judge **sets of runs** with an interval rather than one run.
- On a loaded CI machine, a timer that is far later than on your laptop is a reason to widen the room, not to doubt the tool.
