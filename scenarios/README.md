# Scenarios

Example scenarios in the format of [docs/SCENARIO-FORMAT.md](../docs/SCENARIO-FORMAT.md). They are real: a test reads every file here, checks it against the model and against the JSON Schema, writes it out again and reads that, and expects the same scenario.

| File | What it shows |
| --- | --- |
| [minimal.yaml](minimal.yaml) | The smallest scenario: two nodes, a link without impairment, one stream |
| [lte-like.yaml](lte-like.yaml) | A mobile access link: delay with a normal jitter, loss, and a rate that differs per direction |
| [satellite.yaml](satellite.yaml) | A geostationary hop with `both`, a window for the stream, and `backend: kernel` |
| [lossy-both-ways.yaml](lossy-both-ways.yaml) | Loss in both directions and one stream each way |
| [traffic-patterns.yaml](traffic-patterns.yaml) | The three traffic patterns of release 0.2 besides the constant bit rate: a Poisson process, an on/off source and bursts, with exponential phases and pauses drawn from the `seed` |
| [bandwidth-drop-and-outage.yaml](bandwidth-drop-and-outage.yaml) | A timeline: a rate limit from a moment on, an outage of two seconds, the limit taken away with `null`; applied by the Kernel backend since release 0.2. `netscenario run` on it prints the time to recover after each event, and `series-media.jsonl` shows the quiet second of the outage and the queue that the limit builds (try `--bin 500ms --recovery-basis arrival`) |
| [link-down-and-up.yaml](link-down-and-up.yaml) | A link that goes down for five seconds and comes up again: the sender is refused, nothing is lost that was not sent |
| [program-under-test.yaml](program-under-test.yaml) | A program of your own in a node (`kind: command`) with a readiness check and a shutdown, started by the Kernel backend ([docs/PROGRAMS.md](../docs/PROGRAMS.md); a complete example is [examples/echo-server](../examples/echo-server)) |
| [capture-a-link.yaml](capture-a-link.yaml) | The section `measure`: both interfaces of a lossy link captured as pcap files, and the bins of the series ([docs/SCENARIO-FORMAT.md](../docs/SCENARIO-FORMAT.md#what-is-measured)) |
| [tcp-download.yaml](tcp-download.yaml) | `tcp-bulk`: a download over two connections of 5 mb each, with `reno`, over a link with a rate |
| [request-response.yaml](request-response.yaml) | `request-response`: an API with a request every 20 ms over four connections, a little loss |
| [udp-echo.yaml](udp-echo.yaml) | `echo: true`: every packet of a stream sent back by the far node, and the round trip over a way with 30 ms and a way with 40 ms, each with a loss of 1 % |
| [noisy-radio.yaml](noisy-radio.yaml) | Bursts of loss (both models), reordering, duplication and corruption |
| [three-hops.yaml](three-hops.yaml) | A chain of two routers between a client and a server: three links with their own values, the routes computed by the shortest path (D-078) |
| [fragments.yaml](fragments.yaml) | A link with an MTU of 1000 and packets of 1,400 bytes: the sender cuts them into fragments, a lost fragment loses the packet (D-081) |
| [ipv6.yaml](ipv6.yaml) | A client, a router and a server over IPv6 only: addresses `fd00:100:<n>::/126` on the links, `fd00:200::<n>` on the nodes, a wait of a second before the queues (D-081) |
| [dual-stack.yaml](dual-stack.yaml) | Both families in one network, a flow of each, and a flow with `dont-fragment` that the sender refuses where it does not fit the MTU (D-081) |
| [queues.yaml](queues.yaml) | A bottleneck with a queue of 200 packets and CoDel, offered more than its rate: what the queue drops, and the delay it keeps (D-082) |
| [two-paths.yaml](two-paths.yaml) | Two paths with listed routes and metrics, `routing: manual`, and a link of the fast path that goes down and comes back |
| [manual-routes.yaml](manual-routes.yaml) | Two parallel links with a `cost`, and one route set by hand over the computed ones |
| [dumbbell.yaml](dumbbell.yaml) | The `dumbbell` shortcut: two senders and two receivers share a bottleneck between two routers |
| [tcp-fairness.yaml](tcp-fairness.yaml) | Two reno flows with the same round trip time over a dumbbell with a bottleneck of 10 mbit and a queue: the classic fairness test, judged by the Jain index of their goodputs |
| [tcp-fairness-unequal-rtt.yaml](tcp-fairness-unequal-rtt.yaml) | The same with a longer access link for one flow (long form): the flow with the shorter round trip takes most of the bottleneck |
| [tcp-throughput-vs-loss.yaml](tcp-throughput-vs-loss.yaml) | Reno over a path with a loss of 1 % and a round trip of 40 ms: the goodput to compare with the formula of Mathis |
| [tcp-multipath.yaml](tcp-multipath.yaml) | Sixteen TCP connections over two paths of 4 and 8 mbit with the weights 1 and 2: together they carry the sum of the rates |
| [tcp-multipath-weights.yaml](tcp-multipath-weights.yaml) | 32 light connections over two paths with the weights 1 and 7: the share of the slow path, counted by `capture` |
| [tcp-queue.yaml](tcp-queue.yaml) | Two reno connections over a bottleneck with CoDel: the time in the queue stays near the target, where a FIFO of the same size keeps most of a second |
| [leo-pass.yaml](leo-pass.yaml) | A periodic model (LMD-05): a link of 20 mbit that is in view for 40 s, loses everything for a handover of 6 s and is weak for 14 s, and again |
| [signal-states.yaml](signal-states.yaml) | A Markov chain of three signal states with the chances of the next one (LMD-04): the same seed gives the same states and the same lost packets |
| [flapping-link.yaml](flapping-link.yaml) | Events that repeat (TML-03): an outage of 400 ms every 10 s, and a rate that halves every 20 s |
| [cell-drive.yaml](cell-drive.yaml) | A trace (LMD-03): the rate, the delay and the loss of a drive through a cell from a CSV file ([cell-drive.csv](cell-drive.csv)), one row every 5 s |
| [mahimahi-walk.yaml](mahimahi-walk.yaml) | A downlink from a Mahimahi trace ([walk.mahimahi](walk.mahimahi)) in windows of 50 ms, which the Kernel backend follows only in part: the scenario accepts it with `allow-limited` |
| [parameters.yaml](parameters.yaml) | Parameters (SCN-05): `params:` with defaults, `"{rtt / 2}"` and `"{rate * share}"` in the values, `--param rtt=100ms` on the command line. |
| [templates.yaml](templates.yaml) | A template (SCN-05) for the link that is needed twice, used with `use:` and `with:`. |
| [profile.yaml](profile.yaml) | A link from the profile library (SCN-06): `profile: lte-good` gives the delays of an LTE network, and the loss written next to it wins; the first node of `between` is the terminal |
| [expect.yaml](expect.yaml) | `expect` (ASR-07): three checks on one run, with a bound computed from a parameter; `netscenario run` exits with 1 when one does not hold |
| [expect-set.yaml](expect-set.yaml) | Checks on a set of runs: `repetitions: 10`, and bounds on the 95 % interval around the mean (`ci95-high`, `ci95-low`) and on the worst run (`max`) |
| [include.yaml](include.yaml) | Includes (SCN-05): a link and a list of workloads from files in [parts/](parts/). |
| [star-and-chain.yaml](star-and-chain.yaml) | The `star` shortcut with values for each link, and how the `chain` and `multipath` shortcuts are written |
| [trace-replay.yaml](trace-replay.yaml) | A recorded call ([call.csv](call.csv): the moments and sizes of 695 packets) sent through a link of 2 mbit with a queue |
| [jitter-from-samples.yaml](jitter-from-samples.yaml) | A jitter that takes its shape from a file of measured round trips ([round-trips.txt](round-trips.txt)), read when the scenario is loaded |
| [damaged-payload.yaml](damaged-payload.yaml) | Corruption and a stream with `udp-checksum: false`, so that a damaged packet arrives and is counted |

Each file starts with a line `# yaml-language-server: $schema=...` that points an editor at the JSON Schema of this checkout, for completion and checking while you write (docs/CLI.md has the form for your own files).

`invalid/` holds files that are faulty on purpose, one fault per file, with the message that each must produce in `crates/netscenario-model/tests/snapshots/`. A new kind of fault is a new file there and a new snapshot (`just snapshots`).
