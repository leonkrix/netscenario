# Examples

Two examples, one for each way in which netscenario is used today. Both are tested by the project: the scenarios are checked against the model and the JSON Schema, and the Python test runs in the CI like every other test.

| Folder | For | What it shows |
| --- | --- | --- |
| [udp-protocol/](udp-protocol/) | Someone who builds a UDP protocol or a real-time service and wants to know what a bad link does to a stream shaped like theirs | The command line: a scenario with two streams in opposite directions, the output, and how to read `result.json` |
| [python-test/](python-test/) | Someone who wants the network as part of the test suite | A `pytest` test with assertions on loss and delay, with the fixture and with the marker |

Both need a machine that can create network namespaces. `netscenario doctor` tells whether yours can, and [docs/PERMISSIONS.md](../docs/PERMISSIONS.md) what to change if not.

**What these examples do not show yet:** running *your own program* inside a node. Release 0.1 sends the streams itself (the agent in each node) and measures what the network does to them; your program as a node comes with release 0.2 (WRK-06). The streams are constant-rate UDP: pick the rate and the packet size of your protocol, and the result is what a stream of that shape meets on the link.
