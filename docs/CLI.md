# The command line

```text
netscenario <command> [options]
```

| Command | What it does |
| --- | --- |
| `run <file>` | Runs a scenario in network namespaces and writes the result. |
| `validate <file>` | Checks a scenario file without running it. |
| `schema` | Prints the JSON Schema of the scenario format. |
| `doctor` | Reports what this machine can do and how to fix what it cannot ([DOCTOR.md](DOCTOR.md)). |
| `clean` | Ends what a run without a supervisor left behind ([DOCTOR.md](DOCTOR.md)). |

`-h` or `--help` after a command prints the options of that command; `-V` or `--version` prints the version.

## Exit codes

They are part of the interface. A script can rely on them.

| Code | Meaning |
| --- | --- |
| 0 | The command did what was asked (`run`: the run finished; `validate`: the scenario is valid; `doctor`: scenarios can run here). |
| 1 | A run finished and an assertion about its result failed. No command returns it yet: assertions arrive with the `expect` section and the Python layer. |
| 2 | The command line or the scenario is wrong: an unknown option, a file that cannot be read, a scenario with problems, or a scenario that asks for something the backend cannot reproduce exactly (the message names the feature, the backend and the way out). |
| 3 | The machine does not allow the run: a tool or a permission is missing (`doctor` fails, the agent is not next to the program, the namespaces cannot be created). Run `netscenario doctor`. |
| 4 | The run failed after it started: an agent did not start, did not answer or ended badly. The network is torn down all the same. |

Errors go to standard error, results to standard output; with `--json` standard output is JSON and nothing else.

## `run`

```text
netscenario run <file> [--out <dir>] [--seed <n>] [--settle <duration>] [--packets] [--json]
```

| Option | Meaning |
| --- | --- |
| `--out <dir>` | Where to write the result. Default: `netscenario-results/<name>-<UTC time>/` in the current directory, so two runs do not overwrite each other. The directory must be one that you can write to, because the agents in the nodes write the raw records into it. |
| `--seed <n>` | Use this seed and not the one in the file. This is how a run that drew its own seed is repeated: the summary prints `repeat the run with --seed <n>`. |
| `--settle <duration>` | How long to wait after the last packet for the packets still in the network, for example `2s`. Default: one second and twice the longest delay of the scenario. A packet that needs longer counts as lost. Use it for a rate limit that builds a long queue. |
| `--packets` | Also write `packets-<id>.jsonl` with one line for each packet ([RESULTS.md](RESULTS.md)). |
| `--json` | Print `result.json` on standard output instead of the summary. The file is written either way. |

The run, in order: the file is read and checked (exit 2 with the lines if it is wrong), the backend checks that it can reproduce the scenario exactly (exit 2 if not, before anything is created), the network is built, the agents start, the workloads run for the `duration` of the scenario, the result is written, and everything is removed. The summary shows the seed, the packets, the delay and the throughput of each workload; `result.json` has everything ([RESULTS.md](RESULTS.md)).

A run needs `netscenario-agent` next to the `netscenario` program: `cargo build` and `cargo install` put both in the same directory. The program is also its own supervisor: a run starts it again inside the namespaces with the hidden commands `supervise` and `hold`, which are not for people and are not in the help (D-022).

## `validate`

```text
netscenario validate <file> [--json]
```

Checks the syntax, the keys, every value with its unit, and the rules between values. A valid scenario prints its name, the number of nodes, links and workloads, the duration and the features it needs (the rows of [COVERAGE.md](COVERAGE.md) that a backend has to reproduce). A faulty one prints every problem with the line, the column, the path and a hint, and exits with 2.

With `--json` the answer is one JSON object on standard output:

```json
{"valid":true,"file":"scenarios/lte-like.yaml","name":"lte-like","duration":"30s","nodes":2,"links":1,"workloads":1,"features":["point-to-point","ipv4","fixed-delay","jitter","independent-loss","rate-limit","udp-generators","seeded-loss"]}
```

```json
{"valid":false,"file":"bad.yaml","problems":[{"message":"the node 'sever' does not exist","path":"topology.links[0].between[1]","line":8,"column":20,"help":"did you mean 'server'? the nodes are: a, server"}]}
```

`line`, `column`, `path` and `help` are `null` when a problem has none (a problem about the file as a whole has no line).

## `schema`

```text
netscenario schema
```

Prints the JSON Schema of the scenario format ([`schema/scenario-v0.json`](schema/scenario-v0.json) is the same text). Point an editor at it to get completion and checking while you write a scenario.

## `doctor` and `clean`

See [DOCTOR.md](DOCTOR.md).
