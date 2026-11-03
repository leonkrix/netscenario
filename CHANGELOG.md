# Changelog

Every release of netscenario, newest first. The lines are the titles of the pull requests; the rules for what
a version may change are in [docs/VERSIONING.md](docs/VERSIONING.md).

## 0.2.0 (2026-11-03)

Release 0.2 "Time and noise". This is the state of `0.2.0-rc.1`; the release only removes the notes that say it is a candidate.

A link can change while a run goes (a rate that drops, an outage, a link that goes down and up), and the same seed still gives the same lost packets. There are more impairments: burst loss (Gilbert-Elliott and four states), reordering, duplication and corruption, and a stream can be sent without the UDP checksum so that a damaged packet arrives and is counted. There is more traffic: Poisson, on/off and bursty streams. A node can run **your own program**, started in order, waited for, supervised, with its output and its metrics in the result, and a link can be captured as pcap files. A result has more statistics (the extent of reordering, the interarrival jitter, the runs of lost packets), a series by time bin in two views (what was sent and what arrived) and the time to recover after each event. Several runs with seeds derived from one seed give confidence intervals (a seeded bootstrap with its measured coverage, [docs/STATISTICS.md](https://github.com/leonkrix/netscenario/blob/main/docs/STATISTICS.md)), a run can be replayed from its directory, and reports are made as Markdown, JUnit and one HTML file ([docs/REPORTS.md](https://github.com/leonkrix/netscenario/blob/main/docs/REPORTS.md)). All of it works from the command line, from Python and as pytest tests. The release files carry an attestation of their provenance, and the parsers of input that is not trusted are fuzzed every week. The [README](https://github.com/leonkrix/netscenario#readme) says what 0.2 does and does not do yet, and [docs/ACCEPTANCE.md](https://github.com/leonkrix/netscenario/blob/main/docs/ACCEPTANCE.md) how each criterion of the release is proven.

**Breaking.** The format of the scenario file and of the result is version 0, which a minor release may change ([docs/VERSIONING.md](https://github.com/leonkrix/netscenario/blob/main/docs/VERSIONING.md)): the files of 0.1 still read and run, and the result has new fields. What breaks is the Rust API of `netscenario-model` and `netscenario-core` ([#50](https://github.com/leonkrix/netscenario/pull/50), [#52](https://github.com/leonkrix/netscenario/pull/52), [#53](https://github.com/leonkrix/netscenario/pull/53), [#55](https://github.com/leonkrix/netscenario/pull/55), [#61](https://github.com/leonkrix/netscenario/pull/61), [#76](https://github.com/leonkrix/netscenario/pull/76), [#79](https://github.com/leonkrix/netscenario/pull/79), [#80](https://github.com/leonkrix/netscenario/pull/80)):

- The result types are `non_exhaustive` ([#50](https://github.com/leonkrix/netscenario/pull/50)), so that a new field is no longer a break from now on.
- `Loss` is an enum and not a struct ([#52](https://github.com/leonkrix/netscenario/pull/52)); `Scenario` has the fields `timeline` ([#53](https://github.com/leonkrix/netscenario/pull/53)) and `measure` ([#80](https://github.com/leonkrix/netscenario/pull/80)); `Workload` is an enum of the kinds and `non_exhaustive` ([#55](https://github.com/leonkrix/netscenario/pull/55), [#76](https://github.com/leonkrix/netscenario/pull/76)); `UdpCbr` has `udp_checksum` ([#61](https://github.com/leonkrix/netscenario/pull/61)); `Command` has `on_exit` ([#79](https://github.com/leonkrix/netscenario/pull/79)).
- `Feature` and `Workload` have new variants; `Backend` has new methods with defaults ([#79](https://github.com/leonkrix/netscenario/pull/79), [#80](https://github.com/leonkrix/netscenario/pull/80)).
- `RunOptions` lost `bin`, `recovery` and `export_packets`, which `MeasureOverrides` replaces ([#80](https://github.com/leonkrix/netscenario/pull/80)); `series::Input` takes a `Schedule` ([#76](https://github.com/leonkrix/netscenario/pull/76)).

In Python the arguments `bin`, `recovery_*`, `capture` and `packets` of `run` now replace a key of the section `measure` of the scenario ([#80](https://github.com/leonkrix/netscenario/pull/80)), and the properties that the pytest plugin writes to a JUnit file are text, so `0.0` is `0` ([#85](https://github.com/leonkrix/netscenario/pull/85)). A scenario with `repetitions` as a key is refused, because it is an option of a run (`--repetitions`, `repetitions=`). The command line exits with 1 for a replay that did not reproduce ([#83](https://github.com/leonkrix/netscenario/pull/83)). The wire format of the agent is version 2 ([#61](https://github.com/leonkrix/netscenario/pull/61)), so the programs of a run have to be of one release, which they are when they come from one installation.

### Features

- **Breaking:** Mark the result types non-exhaustive ([#50](https://github.com/leonkrix/netscenario/pull/50))
- Suggest the name that was meant for misspelled keys, values and units ([#51](https://github.com/leonkrix/netscenario/pull/51))
- **Breaking:** Read burst loss, reordering, duplication and corruption in the scenario format ([#52](https://github.com/leonkrix/netscenario/pull/52))
- **Breaking:** Read the timeline in the scenario format ([#53](https://github.com/leonkrix/netscenario/pull/53))
- **Breaking:** Read workloads of kind command in the scenario format ([#55](https://github.com/leonkrix/netscenario/pull/55))
- Apply the burst loss models, the reordering and the duplication in the Kernel backend ([#59](https://github.com/leonkrix/netscenario/pull/59))
- Apply the corruption in the Kernel backend and account for every packet of a direction ([#60](https://github.com/leonkrix/netscenario/pull/60))
- **Breaking:** Send a stream without the UDP checksum and count the packets that arrive damaged ([#61](https://github.com/leonkrix/netscenario/pull/61))
- Name the packets that arrive damaged by sequence number with a tap in the receiving agent ([#62](https://github.com/leonkrix/netscenario/pull/62))
- Report the extent of reordering, the interarrival jitter and the runs of lost packets ([#63](https://github.com/leonkrix/netscenario/pull/63))
- Build the queues of a direction for every state of the timeline and write a state into them as a whole ([#64](https://github.com/leonkrix/netscenario/pull/64))
- Do the steps of the timeline by the clock and record each one in the result ([#65](https://github.com/leonkrix/netscenario/pull/65))
- Do a step of the timeline at a barrier of the workload, so that the same seed gives the same lost packets ([#66](https://github.com/leonkrix/netscenario/pull/66))
- Change a link through one tc that lives for the run and let the backend add notes to the result ([#68](https://github.com/leonkrix/netscenario/pull/68))
- Take links down and up and make an outage a window at the delivery as steps of the timeline, and leave refused packets out of the statistics ([#69](https://github.com/leonkrix/netscenario/pull/69))
- Say that two workloads in a direction whose queue draws random numbers are not a repeatable experiment, and derive the bound of the corruption run test ([#70](https://github.com/leonkrix/netscenario/pull/70))
- Stop the workloads and keep the result when the command line is interrupted with Ctrl-C ([#71](https://github.com/leonkrix/netscenario/pull/71))
- Add a series by time bin for each workload and the time to recover after an event ([#75](https://github.com/leonkrix/netscenario/pull/75))
- **Breaking:** Add the traffic kinds udp-poisson, udp-onoff and udp-bursty with a seeded schedule that the agent walks ([#76](https://github.com/leonkrix/netscenario/pull/76))
- **Breaking:** Run programs of your own in the nodes, with readiness, supervision, logs and metrics ([#79](https://github.com/leonkrix/netscenario/pull/79))
- **Breaking:** Add the section measure and the capture of the interfaces of a link as pcap files ([#80](https://github.com/leonkrix/netscenario/pull/80))
- Add repetitions with derived seeds and bootstrap intervals ([#81](https://github.com/leonkrix/netscenario/pull/81))
- Add repetitions to the Python package and the pytest plugin ([#82](https://github.com/leonkrix/netscenario/pull/82))
- Add replay of a run from its result directory ([#83](https://github.com/leonkrix/netscenario/pull/83))
- Add reports as Markdown, JUnit and HTML from result directories ([#84](https://github.com/leonkrix/netscenario/pull/84))
- Add reports and replay to Python, and the report data of a run to the plugin ([#85](https://github.com/leonkrix/netscenario/pull/85))

### Fixes

- Wait for the queues of the links to empty before the receivers stop, so that packets in a queue do not count as lost ([#72](https://github.com/leonkrix/netscenario/pull/72))
- Refuse what the kernel refuses before the network is built, by declaring the features from the probe of the machine and not from the version of tc alone ([#73](https://github.com/leonkrix/netscenario/pull/73))

## 0.2.0-rc.1 (2026-11-03)

The first release candidate of 0.2. It tried the packages and the release itself, with the provenance of the release files, before `0.2.0`. The changes are those of `0.2.0` above.

## 0.1.0 (2026-10-31)

The first release. You describe a point-to-point link in a file or in code (delay, jitter, loss, a rate, different values in each direction), run it in network namespaces with the Kernel backend, send constant-rate UDP streams through it, and get loss, one-way delay percentiles and throughput back, per packet if you ask for it, together with the seed and the versions that made the result. From the command line (`netscenario run`, `validate`, `schema`, `doctor`, `clean`), from Python, and as `pytest` tests with the plugin `pytest-netscenario`. Wheels for Linux on x86_64 and aarch64; the crates `netscenario-cli` and `netscenario-agent` for the command line. The [README](https://github.com/leonkrix/netscenario#readme) says what 0.1 does and does not do yet, and [docs/ACCEPTANCE.md](https://github.com/leonkrix/netscenario/blob/main/docs/ACCEPTANCE.md) how each criterion of the release is proven.

This is the state of `0.1.0-rc.2`; the release only removes the notes "not published yet" from the documentation and adds the badges. The format of the scenario file and of the result is version 0: a minor release may change them, and the changelog says how ([docs/VERSIONING.md](https://github.com/leonkrix/netscenario/blob/main/docs/VERSIONING.md)).

### Features

- Add the Rust workspace, the lints, the tooling and a first command line ([#11](https://github.com/leonkrix/netscenario/pull/11))
- Add the Python packages, the bindings crate and the Python tooling ([#12](https://github.com/leonkrix/netscenario/pull/12))
- Add the scenario model with strict units, validation and the JSON Schema ([#16](https://github.com/leonkrix/netscenario/pull/16))
- Add the backend interface, the capability check and the Kernel declaration ([#17](https://github.com/leonkrix/netscenario/pull/17))
- Provision real namespaces in the Kernel backend with a supervisor, limits and crash tests ([#18](https://github.com/leonkrix/netscenario/pull/18))
- Record the seed of a run, show lines in capability errors and test root mode and seeded loss on the CI ([#19](https://github.com/leonkrix/netscenario/pull/19))
- Add the doctor and clean commands, and wait for the last words of a supervisor that died ([#20](https://github.com/leonkrix/netscenario/pull/20))
- Add the agent that sends and receives the packets of a workload and records each one ([#21](https://github.com/leonkrix/netscenario/pull/21))
- Read the output of programs in the nodes, write to them, and limit their memory ([#22](https://github.com/leonkrix/netscenario/pull/22))
- Run a scenario in the nodes and write a result with statistics, provenance and the seed ([#23](https://github.com/leonkrix/netscenario/pull/23))
- Add the commands run, validate and schema, and make the program its own supervisor ([#24](https://github.com/leonkrix/netscenario/pull/24))
- Let a run be observed and cancelled, and show its progress on a terminal ([#28](https://github.com/leonkrix/netscenario/pull/28))
- Add the Python bindings and the typed Python package ([#29](https://github.com/leonkrix/netscenario/pull/29))
- Add the pytest plugin with a runner, a marker, a guard for the environment and a report ([#30](https://github.com/leonkrix/netscenario/pull/30))
- Build the wheels with the programs inside and test them in a clean environment ([#31](https://github.com/leonkrix/netscenario/pull/31))
- Add the release workflow, one version for everything, the changelog and the versioning policy ([#33](https://github.com/leonkrix/netscenario/pull/33))
- Count the packets in the queue of each direction with loss and compare them with the packets of the workloads ([#37](https://github.com/leonkrix/netscenario/pull/37))

### Fixes

- Build a direction with a jitter and a rate from two qdiscs, in the order of its stages ([#26](https://github.com/leonkrix/netscenario/pull/26))
- Take the rights of the process for a run when it has them and they work ([#36](https://github.com/leonkrix/netscenario/pull/36))

## 0.1.0-rc.2 (2026-10-31)

The second release candidate. Nothing changes in how a run works. It tests the changed release process (one environment for each registry, one upload to PyPI) and brings the descriptions of the packages and the READMEs that PyPI shows in line with what release 0.1 does: the example is self-contained now, and a program of your own in the network, which comes with release 0.2, is no longer promised.

## 0.1.0-rc.1 (2026-10-31)

The first release candidate of 0.1: the Kernel backend, the command line, the Python package and the pytest plugin, as described in the [README](https://github.com/leonkrix/netscenario#readme) (what 0.1 does, and what it does not do yet). It exists to try the packages and the release itself before `0.1.0`.

### Features

- Add the Rust workspace, the lints, the tooling and a first command line ([#11](https://github.com/leonkrix/netscenario/pull/11))
- Add the Python packages, the bindings crate and the Python tooling ([#12](https://github.com/leonkrix/netscenario/pull/12))
- Add the scenario model with strict units, validation and the JSON Schema ([#16](https://github.com/leonkrix/netscenario/pull/16))
- Add the backend interface, the capability check and the Kernel declaration ([#17](https://github.com/leonkrix/netscenario/pull/17))
- Provision real namespaces in the Kernel backend with a supervisor, limits and crash tests ([#18](https://github.com/leonkrix/netscenario/pull/18))
- Record the seed of a run, show lines in capability errors and test root mode and seeded loss on the CI ([#19](https://github.com/leonkrix/netscenario/pull/19))
- Add the doctor and clean commands, and wait for the last words of a supervisor that died ([#20](https://github.com/leonkrix/netscenario/pull/20))
- Add the agent that sends and receives the packets of a workload and records each one ([#21](https://github.com/leonkrix/netscenario/pull/21))
- Read the output of programs in the nodes, write to them, and limit their memory ([#22](https://github.com/leonkrix/netscenario/pull/22))
- Run a scenario in the nodes and write a result with statistics, provenance and the seed ([#23](https://github.com/leonkrix/netscenario/pull/23))
- Add the commands run, validate and schema, and make the program its own supervisor ([#24](https://github.com/leonkrix/netscenario/pull/24))
- Let a run be observed and cancelled, and show its progress on a terminal ([#28](https://github.com/leonkrix/netscenario/pull/28))
- Add the Python bindings and the typed Python package ([#29](https://github.com/leonkrix/netscenario/pull/29))
- Add the pytest plugin with a runner, a marker, a guard for the environment and a report ([#30](https://github.com/leonkrix/netscenario/pull/30))
- Build the wheels with the programs inside and test them in a clean environment ([#31](https://github.com/leonkrix/netscenario/pull/31))
- Add the release workflow, one version for everything, the changelog and the versioning policy ([#33](https://github.com/leonkrix/netscenario/pull/33))
- Count the packets in the queue of each direction with loss and compare them with the packets of the workloads ([#37](https://github.com/leonkrix/netscenario/pull/37))

### Fixes

- Build a direction with a jitter and a rate from two qdiscs, in the order of its stages ([#26](https://github.com/leonkrix/netscenario/pull/26))
- Take the rights of the process for a run when it has them and they work ([#36](https://github.com/leonkrix/netscenario/pull/36))

