# The command line

```text
netscenario <command> [options]
```

| Command           | What it does                                                                             |
| ----------------- | ---------------------------------------------------------------------------------------- |
| `run <file>`      | Runs a scenario in network namespaces and writes the result.                             |
| `validate <file>` | Checks a scenario file without running it.                                               |
| `schema`          | Prints the JSON Schema of the scenario format.                                           |
| `doctor`          | Reports what this machine can do and how to fix what it cannot ([DOCTOR.md](DOCTOR.md)). |
| `clean`           | Ends what a run without a supervisor left behind ([DOCTOR.md](DOCTOR.md)).               |

From Python, `pip install` of the wheel puts the same program on the `PATH` as the command `netscenario` (it starts the program that the wheel carries; [PYTHON.md](PYTHON.md)).

`-h` or `--help` after a command prints the options of that command; `-V` or `--version` prints the version.

## Exit codes

They are part of the interface. A script can rely on them.

| Code | Meaning |
| --- | --- |
| 0 | The command did what was asked (`run`: the run finished; `replay`: the same packets were lost, or the loss was not promised to repeat; `validate`: the scenario is valid; `doctor`: scenarios can run here). |
| 1 | A check that the command was asked to make did not hold. `replay`: the replay lost other packets than the original, although the loss repeats from the seed. Assertions on a result (the `expect` section) will return it too. |
| 2 | The command line or the scenario is wrong: an unknown option, a file that cannot be read, a scenario with problems, or a scenario that asks for something the backend cannot reproduce exactly (the message names the feature, the backend and the way out). |
| 3 | The machine does not allow the run: a tool or a permission is missing (`doctor` fails, the agent is not next to the program, the namespaces cannot be created). Run `netscenario doctor`. |
| 4 | The run failed after it started: an agent did not start, did not answer or ended badly. The network is torn down all the same. |
| 130, 143 | The run was interrupted by SIGINT (Ctrl-C) or SIGTERM (`kill`): 128 and the number of the signal. If the workloads had begun, a result of the part that ran was written and printed (see below); otherwise there is none. |

Errors go to standard error, results to standard output; with `--json` standard output is JSON and nothing else.

## `run`

```text
netscenario run <file> [--out <dir>] [--seed <n>] [--settle <duration>] [--json]
                       [--repetitions <n>] [--statistic mean|median] [--confidence <percent>]
                       [--bin <duration>] [--recovery-basis send|arrival] [--recovery-baseline <bins>]
                       [--recovery-window <bins>] [--recovery-fraction <percent>]
                       [--capture <link>[,<link>...]|all] [--packets]
```

What a run measures is in the scenario, in the section `measure` ([SCENARIO-FORMAT.md](SCENARIO-FORMAT.md#what-is-measured)). The options from `--bin` to `--packets` have the name of a key and replace it for this run (the result records the scenario as it ran, with its hash); the others (`--out`, `--seed`, `--settle`, `--json`) say how this machine runs it and are not in the file.

| Option | Meaning |
| --- | --- |
| `--out <dir>` | Where to write the result. Default: `netscenario-results/<name>-<UTC time>/` in the current directory, so two runs do not overwrite each other. The directory must be one that you can write to, because the agents in the nodes write the raw records into it. |
| `--seed <n>` | Use this seed and not the one in the file. This is how a run that drew its own seed is repeated: the summary prints `repeat the run with --seed <n>`. |
| `--settle <duration>` | How long to wait after the last packet for the packets still in the network, for example `2s`. Default: until the queues of the links are empty (a rate limit holds the packets that are sent faster than it lets them go), then 0.3 s for the receivers to read, at most two minutes; the result says how long it took (`timing.settle_ns`). With the option the wait is that long and a packet that needs longer counts as lost. |
| `--bin <duration>` | Replaces `measure.series.bin`: the width of a bin of the series of each workload, for example `500ms`. Default `1s`; at least 1 ms, and the measured phase may have at most a million bins (a wrong value is a mistake of the command line, exit 2, before anything is built). Every run writes `series-<id>.jsonl`, the packets sent and arrived by bin ([RESULTS.md](RESULTS.md), D-070); the bin is also the resolution of the time to recover. |
| `--recovery-basis send\|arrival` | Replaces `measure.recovery.basis`: the view the time to recover is made from (`recoveries` in the result, and a line for each in the summary): `send` (default) counts what got through of the packets sent in a bin, `arrival` what the receiver got in it. See [RESULTS.md](RESULTS.md) for which one answers which question. |
| `--recovery-baseline <bins>` | Replaces `measure.recovery.baseline-bins`: how many bins before the disturbance make the baseline level. Default 5. |
| `--recovery-window <bins>` | Replaces `measure.recovery.window-bins`: how many bins in a row have to be back at the level. Default 3. |
| `--recovery-fraction <percent>` | Replaces `measure.recovery.fraction`: the share of the baseline that has to be reached again, above 0 and at most 100 (`90` or `90%`). Default 90. |
| `--capture <links>` | Replaces `measure.capture`: capture the interfaces of these links (ids separated by commas, or `all`) at both ends as `capture-<link>-<node>.pcap`, see [RESULTS.md](RESULTS.md#captures-prb-09-d-047-d-073). The list of the file is not added to but replaced. |
| `--packets` | Switches `measure.packets` on: also write `packets-<id>.jsonl` with one line for each packet ([RESULTS.md](RESULTS.md)). |
| `--repetitions <n>` | Run the scenario `n` times, one after the other, with seeds derived from the seed of the scenario (or of `--seed`); `n` is from 5 to 1,000. `--out` is then the directory of the set: `run-001`, `run-002`, ... hold the runs and `runset.json` holds the values of every run and an interval for each metric. The summary shows the headline metrics of each workload with their intervals. The set ends with the first failure and has no `runset.json` then. See [STATISTICS.md](STATISTICS.md). |
| `--statistic mean\|median` | With `--repetitions`: what the interval is around. Default `mean`. |
| `--confidence <percent>` | With `--repetitions`: the level of the interval, above 0 and below 100 (`95` or `95%`). Default 95. |
| `--json` | Print `result.json` (with `--repetitions`: `runset.json`) on standard output instead of the summary. The file is written either way. |

**Ctrl-C** (or SIGTERM) while the workloads run stops them, waits for the packets that are still in the network (the settle time), writes the result of the part that ran and exits with 130 (143). The summary says how long it ran, and `result.json` has `timing.interrupted_ns`. The steps of the timeline that were not due are not done, and the packets that were not sent are not lost: the result counts what the senders sent. **Ctrl-C a second time quits at once**; the network is removed all the same (D-022). The supervisor, the nodes and the agents are in a process group of their own, so the signal of the terminal reaches the program only and the run can stop in an orderly way (D-066).

On a terminal, one line shows how far the run is (`running, 12 of 30 s`) and is erased before the summary; in a file or a CI log, where standard error is not a terminal, one line says what runs. Nothing about progress goes to standard output.

The run, in order: the file is read and checked (exit 2 with the lines if it is wrong), the backend checks that it can reproduce the scenario exactly (exit 2 if not, before anything is created), the network is built, the agents start, the workloads run for the `duration` of the scenario, the result is written, and everything is removed. The summary shows the seed, the packets, the delay and the throughput of each workload; `result.json` has everything ([RESULTS.md](RESULTS.md)).

A run needs `netscenario-agent` next to the `netscenario` program: `cargo build` and `cargo install` put both in the same directory. The program is also its own supervisor: a run starts it again inside the namespaces with the hidden commands `supervise` and `hold`, which are not for people and are not in the help (D-022).

## `replay`

```text
netscenario replay <dir> [--out <dir>] [--scenario <file>] [--json]
```

Runs a recorded run again and says whether the same packets were lost (ENV-03, [D-075](decisions/D-075.md)). `<dir>` is the directory of a run (the `--out` of `netscenario run`; for a run of a set, `run-001` and so on). It holds `scenario.json`, the scenario exactly as it ran, and the replay uses it with the **recorded seed** and waits for the packets in the network as long as the original did.

| Option | Meaning |
| --- | --- |
| `--out <dir>` | Where to write the result of the replay. Default: `netscenario-results/<name>-replay-<UTC time>/`. It must not be the directory that is replayed. |
| `--scenario <file>` | Also compare this scenario file with the recorded scenario, field by field, and replay nothing if they differ: the error names each field with the value of the run and the value of the file. The replay always runs the recorded scenario and never the file. A file without a `seed` does not differ from a run that drew its seed. |
| `--json` | Print the comparison as JSON (`format` `netscenario-replay`) instead of the summary. |

Before anything is run, the directory is checked: `result.json` and `scenario.json` have to be there, the scenario has to be valid, and its fingerprint has to be the `scenario.hash` of the result (exit code 2, with both fingerprints, if not). A run written by a version that did not record the scenario cannot be replayed, and the directory of a set is not a run.

The summary sets the replay against the original:

- **The machine**: the backend and its details (the version of `tc`), the kernel and the processor, field by field. The lost packets of a seed depend on the kernel's `netem` and on `tc`, so a replay on another machine says "NOT the one of the original" and names what differs.
- **The lost packets** of each workload: the same ones, or how many differ and the first sequence numbers that were lost in only one run. Delays and rates are not compared; they vary from run to run.
- **The verdict**: *reproduced*; *not reproduced* (exit code 1: other packets although both runs said that their loss repeats from the seed); *not promised* (other packets, but the loss of a run does not repeat from its seed, because of an outage, corruption or a program that sends: the notes say which, exit code 0); *not comparable* (the original was interrupted).

The programs of your own (`kind: command`) run in the directory where `replay` is run, because the directory of the original is not recorded.

Exit codes: 0, 1 (not reproduced), 2 (the directory, the file or the options are wrong), 3, 4 and 130 or 143 as for `run`.

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

**In an editor.** Editors with the YAML language server (VS Code with the Red Hat YAML extension, JetBrains IDEs, Neovim with `yaml-language-server`) take the schema from a comment on the first line of the file:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/leonkrix/netscenario/main/docs/schema/scenario-v0.json
version: 0
```

They then complete the keys and values (the loss models, the signals, the directions), show the description of each key, and mark a wrong value while you type. The example scenarios of this repository carry the same line with a relative path to the schema of their own checkout. The schema describes the shape; the rules between values (a node must exist, a jitter needs a delay) are checked by `netscenario validate`, which an editor does not run. The URL above follows `main`, so it can be ahead of the release you have installed; for a fixed version use the file of that tag, or `netscenario schema > scenario.schema.json` and a path to it.

## `doctor` and `clean`

See [DOCTOR.md](DOCTOR.md).
