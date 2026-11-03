# The Python package

`netscenario` for Python is a typed interface to the Rust core. It calls the core in the same process (D-031), so a scenario is read and checked by the same code as on the command line, a run is the same run, and the errors arrive as exceptions with the details attached. The package is checked with `pyright` in strict mode and ships `py.typed`.

## Installing

```bash
pip install netscenario
```

There are two ways to get the package:

- **A wheel** from PyPI, for Linux on x86_64 and aarch64, built in the manylinux_2_28 containers of the PyPA and installed into a clean environment on other images as a test by every run of the CI. One wheel serves every Python from 3.11 on (the stable ABI). It carries the extension module, the supervisor `netscenario`, the agent `netscenario-agent` and the command line; `pip install` of it gives you `import netscenario` and a command `netscenario` on the `PATH` (the same program as `docs/CLI.md` describes). The plugin, `pytest-netscenario`, is a second, pure Python wheel.
- **A checkout**, for development:

```bash
cargo build                  # the programs 'netscenario' and 'netscenario-agent' that a run needs
uv sync                      # builds the extension module and installs the package
uv run python
```

`just wheel-smoke` builds a wheel of your own platform and installs it into a clean environment, which is the closest thing to what a user gets.

The wheel needs the same machine as a checkout: network namespaces, and `ip`, `tc` (iproute2 6.6.0 or newer for a seed), `unshare`, `nsenter` and `prlimit` on the `PATH` (`ns.doctor()` or `netscenario doctor` says what is missing).

## A first run

```python
import netscenario as ns

scenario = ns.Scenario.load("scenarios/lte-like.yaml")
result = ns.run(scenario)

media = result["media"]
print(media.sent, media.received, f"{media.loss_ratio:.2%}")
assert media.delay is not None
assert media.delay.p99 < 0.150  # seconds
```

A run builds the network in network namespaces, starts the agents in the nodes, lets the workloads run for the duration of the scenario and removes everything again. It needs the same machine as the command line; `ns.doctor()` says what this one can do.

## Scenarios

A scenario is read from a file (`Scenario.load`), from text (`Scenario.from_yaml`) or built in code. All three give the same model, checked by the core.

```python
scenario = (
    ns.Scenario.builder("lte-like", duration="30s", seed=42)
    .nodes("client", "server")
    .link(
        "access", "client", "server",
        forward=ns.Impairments(
            delay="40ms", jitter=ns.Jitter("5ms", "normal"), loss="1%", rate="10mbit"
        ),
        reverse=ns.Impairments(delay="40ms", rate="20mbit"),
    )
    .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
    .build()
)
```

`udp_poisson`, `udp_onoff` and `udp_bursty` add the other kinds of traffic (`rate`, `on`, `off`, `burst_packets`, `gap`, `pause` and `distribution` are the keys of the file, [SCENARIO-FORMAT.md](SCENARIO-FORMAT.md)); the packets are drawn from the seed of the scenario, so the same seed gives the same ones.

Values with a unit are strings, as in the file (`"40ms"`, `"10mbit"`, `"1200b"`, `"1%"`): a bare number is refused, because `40` could be 40 seconds or 40 milliseconds. The built scenario above is equal to the one in `scenarios/lte-like.yaml` (a test compares them). `scenario.needs()` lists the features a backend has to reproduce; `scenario.with_seed(7)` gives a copy; `scenario.data` is the canonical form as a plain `dict`.

## Results

`ns.run` returns a frozen `Result` whose numbers are in **SI units**: times in seconds, rates in bits per second, sizes in bytes, ratios from 0 to 1. The fields are those of `result.json` ([RESULTS.md](RESULTS.md)).

| You write | You get |
| --- | --- |
| `result["media"]` | the `Workload` called `media` (also `result.workloads`, and `for w in result`) |
| `w.sent`, `w.received`, `w.lost`, `w.reordered` | packet counts |
| `w.reordering` | how far the reordered packets were out of place, or `None`: `.extent` (`.max`, `.mean`, `.p50`, `.p95`, `.p99`, in packets) and `.late_time` (the same in seconds), RFC 4737 |
| `w.jitter` | the interarrival jitter of RFC 3550 in seconds, or `None`: `.mean` (use this in a test), `.last`, `.max` |
| `w.loss_runs` | the runs of lost packets: `.runs`, `.longest`, `.mean`, `.histogram` (a tuple of `.length` and `.runs`); a burst and scattered loss of the same size differ here |
| `w.corrupted_named`, `w.corrupted_seq` | the damaged packets that the receiving node saw on the wire and did not deliver intact, by sequence number (`None` and `()` where the direction does not corrupt or they could not be named: `w.corrupted_seq_unavailable` says why); `directions` has the exact count for the node |
| `w.corrupted` | datagrams that arrived damaged, in a workload built with `udp_checksum=False`; they are in `w.lost`, not in `w.received` |
| `w.loss_ratio`, `w.delivery_ratio` | 0 to 1 |
| `w.delay.p50`, `.p95`, `.p99`, `.min`, `.mean`, `.max` | seconds; `None` if no packet arrived. Percentiles are the nearest rank, always a measured delay |
| `w.throughput.bits_per_second` | payload bit/s in each second, counted from the first packet sent |
| `w.series` | where the packets by time bin are (`.bin` in seconds, `.bins`, `.file`) or `None` in a result of an older run; `.read()` reads the file into a tuple of `Bin` (`.index`, `.start` in seconds, `.by_send` with `.sent`, `.received`, `.lost`, `.delivered_ratio`, `.delay`, and `.by_arrival` with `.received`, `.bits`, `.delay`, and `.bits_per_second(series.bin)`). Both views use the same bins, counted from the start of the measured phase, the zero of `result.steps` ([RESULTS.md](RESULTS.md)). Each call reads the file again |
| `w.recoveries` | how long the workload took to be back at its level after each event that concerns it, in the order of the events: `.event`, `.kind`, `.link`, `.direction`, `.basis`, `.start`, `.disturbance`, `.baseline`, `.level`, `.time` in seconds or `None` with `.reason`, `.recovered_at`, and `.recovered`. The resolution is one bin |
| `result.programs["app"]` | a program of your own (`kind: command`): `.how_it_ended` (`on-its-own`, `stopped` or `killed`), `.exit_code`, `.exit_signal`, `.succeeded`, `.crashed`, `.started`, `.ready`, `.ended` (seconds from the start of the measured phase, negative before it), `.stdout_file`, `.stderr_file`, `.output` (bytes read and dropped), `.metrics` (by name: `.unit`, `.count`, `.first`, `.last`, `.min`, `.max`), `.readings()` (every reading as `(seconds, name, value)`), `.bad_metric_lines` ([PROGRAMS.md](PROGRAMS.md)) |
| `w.sender.max_lateness` | seconds; large against the gap between packets means the machine shaped the stream |
| `result.seed.value`, `.drawn`, `.loss_reproducible` | the seed, and whether the loss can be repeated |
| `result.seed.queue_checks` | for each direction with loss, the packets its queue counted against the packets the workloads sent (`.link`, `.direction`, `.expected`, `.counted`, `.agrees`) |
| `result.directions` | for each direction that carried a workload, where its packets went (`.link`, `.direction`, `.sent`, `.reached_node`, `.delivered`, `.lost_in_queue`, `.dropped_corrupted`, `.receive_buffer_drops`): lost in the queue, or dropped by the receiving node as damaged ([RESULTS.md](RESULTS.md)) |
| `result.captures` | the captured interfaces (`measure.capture`), two for each link: `.link`, `.node`, `.interface`, `.file` (the pcap, an absolute path), `.packets`, `.bytes`, `.first` and `.last` (seconds from the start of the measured phase, or `None`), `.clock_offset` (the real-time clock minus the monotonic clock, in seconds: the timestamps in the file are on the real-time clock), `.receive_buffer`, `.interface_frames`, `.missing` (frames the interface counted that are not in the file; `None` if it could not check) and `.complete` (`True` only when it checked itself and nothing is missing) |
| `result.steps` | what the run did to the links, in order: `.event`, `.kind`, `.link`, `.direction`, `.scheduled`, `.started`, `.applied` in seconds, `.lateness` (never negative), and for a step done at a barrier `.packet` (the first packet that met it) and `.held` (how long that packet waited), `.at_barrier`; empty without a timeline |
| `result.notes` | what the backend says about how it did the run, one sentence each (for example that a change was made the slow way); empty if the run went as usual |
| `result.provenance` | netscenario version, time, backend and its details, kernel, processor |
| `result.raw` | the decoded `result.json`, unchanged |

What is measured is in the section `measure` of the scenario (`ScenarioBuilder.measure(bin="500ms", recovery_basis=..., capture=["access"], packets=True)` builds it). The arguments of `ns.run` with the names of its keys replace them for one run: `bin` (the width of a bin in seconds, 1 by default), `recovery_basis` (`"send"`, the default, or `"arrival"`), `recovery_baseline` (5 bins), `recovery_window` (3 bins), `recovery_fraction` (0.9, a share from above 0 to 1), `capture` (a list of link ids) and `packets`. A value that makes no sense is a `ValueError` before anything is built, and one that the scenario cannot take (a link that does not exist) is a `ScenarioError` with the path of the key. `working_dir` is the directory the programs of your own run in (default: the directory of the scenario file, or the current one).

```python
result = ns.run(scenario, bin=0.5, recovery_basis="arrival")
media = result["media"]
(drop, *_) = [r for r in media.recoveries if r.kind == "set"]
assert drop.time is not None and drop.time < 5.0, drop.reason   # the receiver was back within five seconds
silent = [b.start for b in media.series.read() if b.by_arrival.received == 0]
```

A run without a `seed` draws one and records it; `ns.run(scenario, seed=result.seed.value)` repeats it.

## Repetitions and confidence intervals

One run is one sample. `repetitions` runs the scenario several times with seeds derived from one seed and returns a `netscenario.RunSet` with an interval for each metric (ASR-02, ASR-03; the method, its coverage and its limits are in [STATISTICS.md](STATISTICS.md)):

```python
runs = ns.run(scenario, repetitions=10, seed=42)      # 5 to 1000 runs, one after the other
loss = runs.metric("media.loss_ratio")                # a SetMetric: every run has it
assert loss.ci95_high < 0.05                          # the upper end of the 95 % interval of the mean
print(loss.mean, loss.low, loss.high, loss.stddev)    # the interval is for the level of the set
p99 = runs.metric("media.delay.p99")                  # seconds: the mean over the runs of the p99 of a run
print(runs.results[3]["media"].lost)                  # the Result of run number 4, read from its directory
```

- **Names.** `<workload>.<what>` in SI units, as in the table of [STATISTICS.md](STATISTICS.md#the-metrics): `media.loss_ratio`, `media.delay.p99` (seconds), `media.throughput.mean` (bits per second), and `<program>.<metric>.last|min|max` for the metrics of a program of your own. `runs.metric("media.loss_ratoi")` is a `KeyError` that names the metrics that look like it.
- **The interval** (`SetMetric.interval`) has `estimate`, `low`, `high`, `level`, `method`, `tail`, `resamples` and `runs`. `statistic="median"` makes it an interval around the median, `confidence=0.9` changes the level. `ci95_low` and `ci95_high` are the short names for the 95 % level and raise a `ValueError` for a set made with another level, instead of giving a number that does not mean what its name says; `low` and `high` are for the level of the set.
- **A metric that only some runs have** (the delay of a run in which no packet arrived) is not in `runs.metrics`; `runs.partial` lists it, and `runs.metric(name)` says in how many runs it was there.
- **The seeds.** `runs.seed` is the seed of the set (`drawn` if the scenario had none, and `seed=runs.seed.value` repeats the set), and `runs.runs[i].seed` the seed of run `i`.
- **Failure.** A set ends with the first run that fails, with that run's exception, and has no `RunSet`; the directories of the finished runs stay in `out`. The observer is told `RepetitionStarted(index, total)` before the events of each run.
- **What to assert.** Compare the end of the interval that is on the dangerous side with a threshold that has room: `loss.ci95_high < 0.05` for a link that loses 1 %. A threshold near the mean fails about half of the time however many runs there are ([STATISTICS.md](STATISTICS.md#what-stable-means)).

## Reports

The reports of the command line ([REPORTS.md](REPORTS.md)) are functions too. They are made from a result directory and nothing else, so the same directory gives the same text:

```python
result = ns.run(scenario, out="results/lte")
summary = result.report()                       # Markdown, for the summary of a CI job
junit = ns.report("results/lte", "junit")       # one test case for each workload, metrics as properties
Path("report.html").write_text(result.report("html"))   # one file with inline charts, nothing to fetch
runs.report("md")                               # a set of runs: the headline metrics with their intervals
```

`ns.report(directory, format)` takes `"md"` (the default), `"junit"` or `"html"` and returns the text; `Result.report` and `RunSet.report` are the same for their own `out_dir`, which has to be still there (the HTML reads the series and the records from it). A directory that is not a result, a file that cannot be read and a wrong format are a `ValueError`. `ns.junit_properties(result)` gives the properties that the report writes for a result or a set, as `(name, value)` pairs: the pytest plugin writes the same list, because both come from the native library.

## Replay

`ns.replay(directory)` runs a recorded run again and says whether the same packets were lost (ENV-03; [CLI.md](CLI.md#replay) has the details and [D-075](decisions/D-075.md) the reasons):

```python
first = ns.run(scenario, seed=42, out="results/first")
again = ns.replay(first.out_dir)                 # the recorded scenario and the recorded seed
assert again.reproduced, again.verdict
print(again.workloads[0].lost_original, again.other_machine, again.result()["media"].lost)
```

The result is a `Replay`: the `verdict` (`"reproduced"`, `"differs"`, `"not-promised"` or `"not-comparable"`), `workloads` with the lost packets of the original and of the replay (and the first sequence numbers that only one of them lost), `provenance` with the fields in which the machine differs, `other_machine`, and `reasons`. **A replay that lost other packets is returned and not raised**: `again.reproduced` is false and the verdict `"differs"` says it failed where both runs promised the loss repeats from the seed; `"not-promised"` is a difference that the notes explain (an outage, corruption, a program that sends). `again.result()` is the `Result` of the replay, which is a run of its own in `out` (default: a new temporary directory). A directory that cannot be replayed (not a run, the directory of a set, a run from before the scenario was recorded, a scenario that is not the one of the result) is a `ValueError` with the reason; the other errors are those of `ns.run`.

## Errors

Every error is a `netscenario.NetscenarioError`. They match the exit codes of the command line ([CLI.md](CLI.md)):

| Exception | When | Exit code |
| --- | --- | --- |
| `ScenarioError` | the scenario is not valid; `.problems` lists every one with `path`, `line`, `column` and `help` | 2 |
| `CapabilityError` | the backend cannot reproduce what the scenario asks for exactly; nothing was created; `.refusals` has the feature, the path, the reason and the way out | 2 |
| `BackendError` | the machine does not allow the run (a tool or a permission is missing; `ns.doctor()` says what) | 3 |
| `RunError` | the run failed after it started | 4 |
| `Cancelled` | the run was stopped by a `CancelToken` | – |

`str(error)` is the full message, with the lines of the file and a mark under the place, as the command line prints it. A scenario built in code has problems with paths and without lines, because there is no file.

## Watching and stopping a run

```python
def show(event: ns.events.RunEvent) -> None:
    if isinstance(event, ns.events.Progress):
        print(f"{event.elapsed:.0f} of {event.total:.0f} s")

token = ns.CancelToken()
result = ns.run(scenario, observer=show, cancel=token)   # token.cancel() from any thread stops it
```

The events are `Provisioned`, `ReceiversReady`, `Started`, `Progress` (once a second), `Settling`, `Collecting`, `TearingDown` and `Finished`, a `RepetitionStarted` at the start of each run of a set, and a `StepDone` (the event of the timeline and how late it was) for each step of a timeline, between `Started` and `Settling` ([ARCHITECTURE.md](ARCHITECTURE.md), "Watching and stopping a run"). An exception in the observer stops the run and is raised.

`ns.run` blocks, but it gives up the GIL and looks for interrupts every 100 ms. **Ctrl-C** (`KeyboardInterrupt`) and `pytest-timeout` stop the run: the network is torn down first, then the interrupt is raised. A cancel from Python is an error and not a partial result: the option that keeps what was recorded (`RunOptions::partial_on_cancel`, D-066) is for the command line. A run cannot be left behind: everything runs in a PID namespace that the kernel ends when the run's supervisor dies (D-022).

## The programs of a run

Python cannot be the supervisor of a run (it has several threads and cannot move into a user namespace), so a run starts the program `netscenario`, which is its own supervisor, and `netscenario-agent` in the nodes. They are looked for in this order: the directory in `NETSCENARIO_BIN_DIR`; `netscenario/_bin` inside the package (where a wheel puts them); in a checkout, `target/release` or `target/debug`, whichever was built last. If they are not found, `ns.run` raises `BackendError` and lists the places it looked.

## Not yet

`asyncio` (0.3), per-packet data as frames (0.3). The pytest plugin is described in [PYTEST.md](PYTEST.md).
