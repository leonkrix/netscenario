# Scenarios

Example scenarios in the format of [docs/SCENARIO-FORMAT.md](../docs/SCENARIO-FORMAT.md). They are real: a test reads every file here, checks it against the model and against the JSON Schema, writes it out again and reads that, and expects the same scenario.

| File | What it shows |
| --- | --- |
| [minimal.yaml](minimal.yaml) | The smallest scenario: two nodes, a link without impairment, one stream |
| [lte-like.yaml](lte-like.yaml) | A mobile access link: delay with a normal jitter, loss, and a rate that differs per direction |
| [satellite.yaml](satellite.yaml) | A geostationary hop with `both`, a window for the stream, and `backend: kernel` |
| [lossy-both-ways.yaml](lossy-both-ways.yaml) | Loss in both directions and one stream each way |
| [bandwidth-drop-and-outage.yaml](bandwidth-drop-and-outage.yaml) | A timeline: a rate limit from a moment on, an outage of two seconds, the limit taken away with `null`; applied by the Kernel backend since release 0.2 |
| [link-down-and-up.yaml](link-down-and-up.yaml) | A link that goes down for five seconds and comes up again: the sender is refused, nothing is lost that was not sent |
| [program-under-test.yaml](program-under-test.yaml) | A program of your own in a node (`kind: command`) with a readiness check and a shutdown; started by the Kernel backend from release 0.2 |
| [noisy-radio.yaml](noisy-radio.yaml) | Bursts of loss (both models), reordering, duplication and corruption |
| [damaged-payload.yaml](damaged-payload.yaml) | Corruption and a stream with `udp-checksum: false`, so that a damaged packet arrives and is counted |

Each file starts with a line `# yaml-language-server: $schema=...` that points an editor at the JSON Schema of this checkout, for completion and checking while you write (docs/CLI.md has the form for your own files).

`invalid/` holds files that are faulty on purpose, one fault per file, with the message that each must produce in `crates/netscenario-model/tests/snapshots/`. A new kind of fault is a new file there and a new snapshot (`just snapshots`).
