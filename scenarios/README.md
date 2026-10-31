# Scenarios

Example scenarios in the format of [docs/SCENARIO-FORMAT.md](../docs/SCENARIO-FORMAT.md). They are real: a test reads every file here, checks it against the model and against the JSON Schema, writes it out again and reads that, and expects the same scenario.

| File | What it shows |
| --- | --- |
| [minimal.yaml](minimal.yaml) | The smallest scenario: two nodes, a link without impairment, one stream |
| [lte-like.yaml](lte-like.yaml) | A mobile access link: delay with a normal jitter, loss, and a rate that differs per direction |
| [satellite.yaml](satellite.yaml) | A geostationary hop with `both`, a window for the stream, and `backend: kernel` |
| [lossy-both-ways.yaml](lossy-both-ways.yaml) | Loss in both directions and one stream each way |
| [noisy-radio.yaml](noisy-radio.yaml) | Bursts of loss (both models), reordering, duplication and corruption; applied by the Kernel backend from release 0.2 |

`invalid/` holds files that are faulty on purpose, one fault per file, with the message that each must produce in `crates/netscenario-model/tests/snapshots/`. A new kind of fault is a new file there and a new snapshot (`just snapshots`).
