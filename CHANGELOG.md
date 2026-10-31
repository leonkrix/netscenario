# Changelog

Every release of netscenario, newest first. The lines are the titles of the pull requests; the rules for what
a version may change are in [docs/VERSIONING.md](docs/VERSIONING.md).

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

