# Versioning and compatibility

What a version number promises, and what a release may break (REL-03, D-034). The process of a release is in [RELEASING.md](RELEASING.md).

## One version for everything that is released

The crates, the Python package `netscenario`, the plugin `pytest-netscenario` and the programs inside the wheel (`netscenario` and `netscenario-agent`) are built from one repository and released together under one [semantic version](https://semver.org) (D-004). The version is written once, in `Cargo.toml`; `python/release/version.py` writes and checks the other places, and `pytest-netscenario` depends on exactly the same version of `netscenario`. A pre-release is `0.2.0-rc.1` in Cargo and on GitHub, and `0.2.0rc1` on PyPI (the same version, spelled as Python spells it). The programs and `result.json` write it as Cargo does (`netscenario --version` prints `0.2.0-rc.1`); `netscenario.__version__` and `pip` write it as Python does.

Besides the release there are the versions of the **formats**. They are separate numbers because a file outlives a release.

| Surface                                  | What carries the version                                            | Now |
| ---------------------------------------- | ------------------------------------------------------------------- | --- |
| Scenario files                           | The line `version:` in the file, and `docs/schema/scenario-v0.json` | 0   |
| Results                                  | `format` and `version` in `result.json`                             | 0   |
| Wire format and record file of the agent | The version field in the bytes, [AGENT.md](AGENT.md)                | 1   |

The wire format and the record file are an interface between the supervisor and the agent **of the same release**; they are not meant to be written or read by anything else, and they may change in any release (a reader refuses a version it does not know).

## What is the interface

Compatibility is promised for these surfaces and for nothing else:

- **The scenario format** ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md)): which files are accepted and what they mean. A file that is refused today with "planned for release X" is not an interface.
- **The result** ([RESULTS.md](RESULTS.md)): the names, units and meaning of the fields. A new field is not a change.
- **The command line** ([CLI.md](CLI.md)): the commands, their options, the exit codes and the JSON that `--json` prints. The text for people (progress, tables, error messages) is not an interface.
- **The Python API**: what `netscenario` and `pytest_netscenario` export (`__all__`, the documented names), the errors and their `kind`, the marker, the fixture and the ini option. Names that start with an underscore, and the module `netscenario._native`, are not.
- **The Rust crates**: `netscenario-model` and `netscenario-core` are libraries for others. `netscenario-backend-kernel`, `netscenario-agent` and `netscenario-cli` are published because the others need them; use their programs, not their Rust API.

The numbers a run produces are **not** compared across versions: for the same seed, the same netscenario, the same iproute2 and the same kernel, the loss pattern is the same; a change of any of the four may change it. The result records the seed and the versions it can see ([RESULTS.md](RESULTS.md)).

## Before 1.0 (now)

The version is `0.y.z`.

- **A patch release** (`0.1.1`) fixes errors. It does not break any interface on purpose, and a scenario that ran in `0.1.0` runs in it.
- **A minor release** (`0.2.0`) may break any interface above, and adds features. Every break is named in the changelog under the line of its pull request, and the title of that pull request has an exclamation mark (`feat!:`), which the changelog shows as **Breaking**. A breaking change to the scenario format also says how to rewrite a file.
- **The formats stay at version 0** until 1.0. A reader refuses a version it does not know, so a file from a newer tool does not run silently on an older one.
- If you depend on netscenario, pin the minor release: `netscenario~=0.1.0` in Python (any `0.1.x`), and `"0.1"` in `Cargo.toml`, which Cargo reads as any `0.1.x` as well.
- A pre-release (`-rc.N`) is for trying the release; it is not announced as a version to depend on.

## From 1.0

The usual rules of semantic versioning for every surface above:

- A patch release fixes errors. A minor release adds, and may deprecate: a deprecated thing keeps working for at least one more minor release and says so when it is used. A major release may break, and the changelog has a guide.
- The formats get a version of 1. A change that breaks files raises it, the tool reads the older version for at least one major release, and `netscenario migrate` rewrites a file (SCN-08).
- The lowest supported **Python** follows D-017 (the oldest version that has not reached its end of life) and the lowest supported **Rust** is `rust-version` in `Cargo.toml` (D-020). Raising either is a minor release and is written in the changelog. The **platform** is Linux, and the kernel and iproute2 versions that [COVERAGE.md](COVERAGE.md) and `netscenario doctor` name.

## A faulty release

A release that does not work is **yanked** (crates.io, PyPI): it stays installable for those who pinned it, and nobody gets it by default. It is replaced by a patch release; a version number is never reused. [RELEASING.md](RELEASING.md) has the steps.
