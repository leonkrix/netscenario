# Install and check the machine

netscenario runs on Linux. It needs the tools `ip`, `tc`, `unshare`, `nsenter` and `prlimit` (the packages `iproute2` and `util-linux`) and the right to create network namespaces. Debian and Ubuntu 22.04 give an ordinary user that right; Ubuntu 24.04 needs one setting, which [Running in a CI](ci.md) and the [permissions page](https://github.com/leonkrix/netscenario/blob/main/docs/PERMISSIONS.md) describe.

## Install

For Python, which also gives you the command line:

```bash
pip install netscenario pytest-netscenario
```

The wheel carries the two programs it needs, so the Python package needs nothing else. For the command line without Python:

```bash
cargo install netscenario-cli netscenario-agent
```

Both programs have to be installed side by side, which is why both crates are named.

## Ask the machine

`doctor` looks at the machine and says what it can do and, for everything that is missing, how to fix it:

```bash
netscenario doctor
```

The exit code is 0 when scenarios can run here and 3 when something has to be fixed first, so a script or a CI job can use it as the first step. The command makes its own namespaces for the probe and touches nothing of the host.

A second command says which features of a scenario this machine reproduces exactly:

```bash
netscenario capabilities
```

A feature that the machine cannot reproduce is not approximated: a scenario that uses it is refused before anything is built, and the message names the feature and what to change. This is the rule that makes a result worth reading.

From Python the same report is `netscenario.doctor()`.

## Where the files go

A run writes its result into a directory: `--out`, or `netscenario-results/<name>-<time>/` in the current directory. Give it a place you can write to, because the programs in the namespaces write the raw records there too.
