# Examples

Five examples, one for each way in which netscenario is used today. All are tested by the project: the scenarios are checked against the model and the JSON Schema, and the Python test runs in the CI like every other test.

| Folder | For | What it shows |
| --- | --- | --- |
| [udp-protocol/](udp-protocol/) | Someone who builds a UDP protocol or a real-time service and wants to know what a bad link does to a stream shaped like theirs | The command line: a scenario with two streams in opposite directions, the output, and how to read `result.json` |
| [echo-server/](echo-server/) | Someone who writes a server or a client and wants to run **their own program** over a bad link | `kind: command`: a UDP echo server and a client in two nodes, readiness by a UDP socket, metrics as lines of JSON, a `pytest` test on them |
| [python-test/](python-test/) | Someone who wants the network as part of the test suite | A `pytest` test with assertions on loss and delay, with the fixture and with the marker, and one on a **confidence interval** over repetitions |
| [live-change/](live-change/) | Someone whose test has to decide **when** the link changes | `netscenario.start` and `run.link(...).set(...)`: the test waits for a moment, makes the link worse, and the change is a step of the timeline at the packet it met, which a replay repeats |
| [timeline/](timeline/) | Someone who wants to know what a link that **changes during the run** does to a stream | A bandwidth drop and an outage, the series by second in both views, the time to recover after each event, and the report |

All need a machine that can create network namespaces. `netscenario doctor` tells whether yours can, and [docs/PERMISSIONS.md](../docs/PERMISSIONS.md) what to change if not.

**What these examples do not show yet:** programs that send TCP (release 0.3).
