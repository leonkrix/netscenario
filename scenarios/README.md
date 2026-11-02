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
| [noisy-radio.yaml](noisy-radio.yaml) | Bursts of loss (both models), reordering, duplication and corruption |
| [damaged-payload.yaml](damaged-payload.yaml) | Corruption and a stream with `udp-checksum: false`, so that a damaged packet arrives and is counted |

Each file starts with a line `# yaml-language-server: $schema=...` that points an editor at the JSON Schema of this checkout, for completion and checking while you write (docs/CLI.md has the form for your own files).

`invalid/` holds files that are faulty on purpose, one fault per file, with the message that each must produce in `crates/netscenario-model/tests/snapshots/`. A new kind of fault is a new file there and a new snapshot (`just snapshots`).
