# Decisions

A log of the decisions that are hard to undo, with the reasons and the alternatives. **Each decision is a file of its own in [`decisions/`](decisions/)**, named by its number (`decisions/D-038.md`); this page is the index. Add an entry before building on a decision: a new file with the next number, and a line in the table below. To change a decision, add a new entry that supersedes it; do not edit history.

Status: **accepted**, **proposed** (to be confirmed by an experiment or a first implementation), **superseded**.

| Decision | Title | Status |
| --- | --- | --- |
| [D-001](decisions/D-001.md) | The name is netscenario | Accepted |
| [D-002](decisions/D-002.md) | Rust core with a Python layer | Accepted |
| [D-003](decisions/D-003.md) | Dual license, Apache-2.0 or MIT | Accepted |
| [D-004](decisions/D-004.md) | One repository for the framework; the Tetrys library lives elsewhere | Accepted |
| [D-005](decisions/D-005.md) | The kernel is the reference, and backends declare their fidelity | Accepted |
| [D-006](decisions/D-006.md) | Linux first | Accepted |
| [D-007](decisions/D-007.md) | The scenario format is versioned YAML with a generated JSON Schema | Accepted 2026-10-31 |
| [D-008](decisions/D-008.md) | Traffic and measurement come from agents inside the nodes | Accepted |
| [D-009](decisions/D-009.md) | Call `ip` and `tc` first, netlink later | Accepted |
| [D-010](decisions/D-010.md) | Two Python distributions | Accepted |
| [D-011](decisions/D-011.md) | The feature ledger and the coverage matrix drive the work | Accepted |
| [D-012](decisions/D-012.md) | Working practice follows the sibling projects | Accepted |
| [D-013](decisions/D-013.md) | Findings of the spikes S1 to S6 decide the Kernel backend of release 0.1 | Accepted |
| [D-014](decisions/D-014.md) | The spikes S7 to S10 confirm D-009 and D-010, fix the Rust version, and show what the engine still has to solve | Accepted |
| [D-015](decisions/D-015.md) | The run on the CI runners confirms S7 to S9 and qualifies the timer finding of S8 | Accepted |
| [D-016](decisions/D-016.md) | D-008 is accepted; D-007 stays proposed until the first implementation | Accepted |
| [D-017](decisions/D-017.md) | The lowest supported Python is 3.11, and the rule is the oldest version that has not reached its end of life | Accepted |
| [D-018](decisions/D-018.md) | The Python side uses uv, maturin, hatchling, ruff and pyright in strict mode | Accepted |
| [D-019](decisions/D-019.md) | The bindings are built as an extension module without a Python interpreter, and the crate has no Rust tests | Accepted |
| [D-020](decisions/D-020.md) | The scenario format of release 0.1, the YAML parser serde-saphyr, and Rust 1.89 | Accepted |
| [D-021](decisions/D-021.md) | The model has one set of types, and the positions in the file come from a second reading with the parser | Accepted |
| [D-022](decisions/D-022.md) | A run lives in a PID namespace with the supervisor as its first process, and the supervisor is a child process | Accepted |
| [D-023](decisions/D-023.md) | A backend declares its capabilities as data, and what depends on the machine is part of the declaration | Accepted |
| [D-024](decisions/D-024.md) | `doctor` probes with the commands of a run, and `clean` ends only holders without a supervisor | Accepted |
| [D-025](decisions/D-025.md) | The agent reads CLOCK_MONOTONIC through rustix, counts payload, and records in a binary file | Accepted |
| [D-026](decisions/D-026.md) | A program in a node is read through the supervisor by polling, and its memory is limited with prlimit | Accepted |
| [D-027](decisions/D-027.md) | The result format of release 0.1, how a run is ordered, and what the statistics mean | Accepted |
| [D-028](decisions/D-028.md) | The command line of release 0.1: one program that is also its own supervisor, and what each exit code means | Accepted |
| [D-029](decisions/D-029.md) | Fidelity is measured against bounds with reasons, one test at a time, and what the measurements found is documented before it is decided | Accepted |
| [D-030](decisions/D-030.md) | A direction is built in the order of its stages (loss, rate, delay with jitter), with two qdiscs where one cannot do it | Accepted |
| [D-031](decisions/D-031.md) | Python calls the core in the same process, and the core can be observed and cancelled | Accepted |
| [D-032](decisions/D-032.md) | The pytest plugin is a runner over `netscenario.run`, with a guard for the environment and plain asserts | Accepted |
| [D-033](decisions/D-033.md) | One platform wheel carries the extension module, the supervisor and the agent, and the command | Accepted |
| [D-034](decisions/D-034.md) | A release is a tag; one version for everything; the registries trust the workflow, and a maintainer approves | Accepted |
| [D-035](decisions/D-035.md) | A run takes the way that `doctor` reports: its own rights if it has them and they work, else a user namespace | Accepted |
| [D-036](decisions/D-036.md) | A run counts the packets of each queue with loss and compares them with the packets of the workloads | Accepted |
| [D-037](decisions/D-037.md) | The CI checks the public Rust API against `main`, and a release against the last release | Accepted |
| [D-038](decisions/D-038.md) | Events of the timeline are applied between packets, so that the lost packets stay reproducible | Accepted 2026-10-31 |
| [D-039](decisions/D-039.md) | An outage and a link that is down are two things | Accepted 2026-10-31 |
| [D-040](decisions/D-040.md) | A corrupted packet is dropped by the receiver; the result tells it apart from a lost one, exactly for the node and by sequence where the packet allows it | Accepted 2026-10-31 |
| [D-041](decisions/D-041.md) | A program of your own is a node's workload of kind `command`, and the file says so | Accepted 2026-10-31 |
| [D-042](decisions/D-042.md) | Repetitions are an option of a run, and the interval is a percentile bootstrap | Accepted 2026-10-31 |
| [D-043](decisions/D-043.md) | Reports are static, and a run can be replayed | Accepted 2026-10-31 |
| [D-044](decisions/D-044.md) | "Stable over 100 CI runs" is a workflow | Accepted 2026-10-31 |
| [D-045](decisions/D-045.md) | The parsers of input that is not trusted are fuzzed from release 0.2 | Accepted 2026-10-31 |
| [D-046](decisions/D-046.md) | The result types are `non_exhaustive` from release 0.2 | Accepted 2026-10-31 |
| [D-047](decisions/D-047.md) | Packet capture is done by the agent with `recvmsg` on an `AF_PACKET` socket, and it checks itself against the counter of the interface | Accepted 2026-10-31 |
| [D-048](decisions/D-048.md) | What the `netem` parameters mean, as measured, and which of them the format does not offer | Accepted 2026-10-31 |
| [D-049](decisions/D-049.md) | The impairments of release 0.2 in the format | Accepted 2026-11-01 |
| [D-050](decisions/D-050.md) | The timeline in the format | Accepted 2026-11-01 |
| [D-051](decisions/D-051.md) | `kind: command` in the format | Accepted 2026-11-01 |
| [D-052](decisions/D-052.md) | `duplicate` together with a jitter and a rate is probed on the kernel, not assumed | Accepted 2026-11-01 |
| [D-053](decisions/D-053.md) | How the fuzz targets are built and judged (carrying out D-045) | Accepted 2026-11-01 |
| [D-054](decisions/D-054.md) | The Kernel backend builds the loss models, the reordering and the duplication | Accepted 2026-11-01 |
| [D-055](decisions/D-055.md) | The Kernel backend applies corruption, and the result accounts for every packet of a direction | Accepted 2026-11-01 |
| [D-056](decisions/D-056.md) | A stream without the checksum of UDP is a raw socket, a boolean and a CRC-32 | Accepted 2026-11-01 |
| [D-057](decisions/D-057.md) | The damaged packets of a corrupting direction are named by a tap in the receiving agent | Accepted 2026-11-01 |
| [D-058](decisions/D-058.md) | The pattern of the arrivals is computed in the core from the first arrivals, by the definitions of the RFCs | Accepted 2026-11-01 |
| [D-059](decisions/D-059.md) | A direction keeps its queues for the whole run, and a change writes the whole state into them | Accepted 2026-11-01 |
| [D-060](decisions/D-060.md) | The run does the steps of the timeline by the clock and records each one | Accepted 2026-11-02 |
| [D-061](decisions/D-061.md) | The agent is optimised in the development profile, because it is the instrument (after a CI failure) | Accepted 2026-11-02 |
| [D-062](decisions/D-062.md) | A step of the timeline is done at a barrier of the workload, so the same seed gives the same lost packets | Accepted 2026-11-02 |
| [D-063](decisions/D-063.md) | The changes of a link go through a `tc` that lives for the run | Accepted 2026-11-02 |
| [D-064](decisions/D-064.md) | An outage is a window at the delivery, a link that is down refuses, and the statistics know which packets were refused | Accepted 2026-11-02 |
| [D-065](decisions/D-065.md) | Two workloads in a direction whose queue draws random numbers are not a repeatable experiment, and the result says so | Accepted 2026-11-02 |
| [D-066](decisions/D-066.md) | Ctrl-C stops the workloads and keeps what was recorded | Accepted 2026-11-02 |
| [D-067](decisions/D-067.md) | The default wait for the packets in the network is until the queues are empty | Accepted 2026-11-02 |
| [D-068](decisions/D-068.md) | What the kernel refuses is not declared, and a run follows the same probe as `doctor` | Accepted 2026-11-02 |
| [D-069](decisions/D-069.md) | The bound on the lateness of a step follows what the machine does to a sleeping thread | Accepted 2026-11-02 |
| [D-070](decisions/D-070.md) | The series of a workload is by time bin, in two views, in a file of its own, and the time to recover is defined once and says so in the result | Accepted 2026-11-02 |
| [D-071](decisions/D-071.md) | Poisson, on/off and bursty traffic are three kinds, drawn from a seeded generator that is written out, and a schedule is walked and not looked up | Accepted 2026-11-02 |
| [D-072](decisions/D-072.md) | A program of your own is started in order, waited for, watched, stopped as a group, and reported with its output and metrics | Accepted 2026-11-02 |
| [D-073](decisions/D-073.md) | What is measured is in the scenario, an option of a run replaces a key, and a capture takes both ends of a link | Accepted 2026-11-02 |
| [D-074](decisions/D-074.md) | A set of runs has its own seeds, a named list of metrics, an expanded percentile interval for the mean, and ends with the first failure | Accepted 2026-11-02 |
| [D-075](decisions/D-075.md) | A run records its scenario, and a replay runs it with the recorded seed and compares the lost packets and the machine | Accepted 2026-11-03 |
| [D-076](decisions/D-076.md) | A report is made from a result directory and nothing else, in three formats, and the JUnit failures are the ones a result can know | Accepted 2026-11-03 |
| [D-077](decisions/D-077.md) | Python gets the reports and the replay as thin layers over the native library, and the properties of the plugin come from the core | Accepted 2026-11-03 |
| [D-078](decisions/D-078.md) | A topology is one graph with shortcuts, the routes are automatic, set in part or fully manual, and every node has an address of its own | Accepted 2026-11-03 |
| [D-079](decisions/D-079.md) | A link that needs a queue discipline or a rate that changes is built through a fabric namespace, with a delaying `netem` in front of the shaper | Accepted 2026-11-03 |
| [D-080](decisions/D-080.md) | A packet is accounted for link by link along its path, by the counts of the nodes, where the run can tell which packets enter a link | Accepted 2026-11-04 |
| [D-081](decisions/D-081.md) | The IP family is chosen for the network and for the workload, a link has one MTU, and what fragments is accounted for by frames | Accepted 2026-11-04 |
| [D-082](decisions/D-082.md) | The queue of a direction is the packets that wait for its rate, with a discipline and its parameters, and the run reads its counters | Accepted 2026-11-04 |
| [D-083](decisions/D-083.md) | A scenario reads the files it refers to when it is loaded, from its own directory, and the scenario holds what is made of them | Accepted 2026-11-04 |
| [D-084](decisions/D-084.md) | The jitter takes its shape from a table of `netem`: the tables of iproute2, or one made from a file of samples, scaled to fit 16 bits | Accepted 2026-11-04 |
