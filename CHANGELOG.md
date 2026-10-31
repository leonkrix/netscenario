# Changelog

Every release of netscenario, newest first. The lines are the titles of the pull requests; the rules for what
a version may change are in [docs/VERSIONING.md](docs/VERSIONING.md).

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

