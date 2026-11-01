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
| `w.sender.max_lateness` | seconds; large against the gap between packets means the machine shaped the stream |
| `result.seed.value`, `.drawn`, `.loss_reproducible` | the seed, and whether the loss can be repeated |
| `result.seed.queue_checks` | for each direction with loss, the packets its queue counted against the packets the workloads sent (`.link`, `.direction`, `.expected`, `.counted`, `.agrees`) |
| `result.directions` | for each direction that carried a workload, where its packets went (`.link`, `.direction`, `.sent`, `.reached_node`, `.delivered`, `.lost_in_queue`, `.dropped_corrupted`, `.receive_buffer_drops`): lost in the queue, or dropped by the receiving node as damaged ([RESULTS.md](RESULTS.md)) |
| `result.steps` | what the run did to the links, in order: `.event`, `.kind`, `.link`, `.direction`, `.scheduled`, `.started`, `.applied` in seconds, `.lateness` (never negative), and for a step done at a barrier `.packet` (the first packet that met it) and `.held` (how long that packet waited), `.at_barrier`; empty without a timeline |
| `result.notes` | what the backend says about how it did the run, one sentence each (for example that a change was made the slow way); empty if the run went as usual |
| `result.provenance` | netscenario version, time, backend and its details, kernel, processor |
| `result.raw` | the decoded `result.json`, unchanged |

A run without a `seed` draws one and records it; `ns.run(scenario, seed=result.seed.value)` repeats it.

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

The events are `Provisioned`, `ReceiversReady`, `Started`, `Progress` (once a second), `Settling`, `Collecting`, `TearingDown` and `Finished`, and a `StepDone` (the event of the timeline and how late it was) for each step of a timeline, between `Started` and `Settling` ([ARCHITECTURE.md](ARCHITECTURE.md), "Watching and stopping a run"). An exception in the observer stops the run and is raised.

`ns.run` blocks, but it gives up the GIL and looks for interrupts every 100 ms. **Ctrl-C** (`KeyboardInterrupt`) and `pytest-timeout` stop the run: the network is torn down first, then the interrupt is raised. A run cannot be left behind: everything runs in a PID namespace that the kernel ends when the run's supervisor dies (D-022).

## The programs of a run

Python cannot be the supervisor of a run (it has several threads and cannot move into a user namespace), so a run starts the program `netscenario`, which is its own supervisor, and `netscenario-agent` in the nodes. They are looked for in this order: the directory in `NETSCENARIO_BIN_DIR`; `netscenario/_bin` inside the package (where a wheel puts them); in a checkout, `target/release` or `target/debug`, whichever was built last. If they are not found, `ns.run` raises `BackendError` and lists the places it looked.

## Not yet

Starting your own program in a node (release 0.2), repetitions and confidence intervals (0.2), `asyncio` (0.3), per-packet data as frames (0.3). The pytest plugin is described in [PYTEST.md](PYTEST.md).
