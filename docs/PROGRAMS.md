# Your own program in a node

A workload of `kind: command` starts a program of yours in a node (WRK-06, WRK-10, PRB-08; [D-072](decisions/D-072.md)). The keys are in [SCENARIO-FORMAT.md](SCENARIO-FORMAT.md#a-program-of-your-own); this page says what the run does with them. A working example, a UDP echo server and a client over a bad link, is [examples/echo-server/](../examples/echo-server/).

```yaml
- id: app
  kind: command
  node: server
  run: ["./my-server", "--listen", "{server.addr}:4433"]
  ready: { udp-port: 4433, within: 5s }
  shutdown: { signal: TERM, within: 3s }
  on-exit: fail
```

## What runs, and as whom

- **In the network namespace of the node**, so the program sees the interface of its link and nothing else of the network. It is not in a mount namespace of its own: **it shares the file system of the machine** with you and with everything else you run, and it can read and write whatever you can. Running a scenario file that you did not write is running its programs, so read the `command` entries first ([SECURITY.md](../SECURITY.md)).
- **As you** (inside the user namespace that is uid 0, mapped to you; the files it creates are yours), **without a shell**: `run` is the program and its arguments. A name without a slash is looked for in `PATH`; a path with a slash from the directory below. A program that does not exist or is not executable fails the run before anything is created, naming the path.
- **In the directory of the scenario file** (`RunOptions::working_dir`; the command line and `ns.run` set it from the file, or from the current directory for a scenario built in code, or from `working_dir=`). A relative `ready: { file: ... }` is counted from there too.
- **Leader of a process group of its own.** The signal of `shutdown` goes to the group, so the children of a script go too.
- **With these variables** besides the ones in `env`: `NETSCENARIO_WORKLOAD` (its id), `NETSCENARIO_NODE`, `NETSCENARIO_RUN_DIR` (the absolute directory of the run), `NETSCENARIO_SEED` (if the run has one), and `NETSCENARIO_ADDR_<NODE>` for every node (the name in capitals, `-` as `_`). `{node.addr}` in `run` and in the values of `env` is the address of a node (the address of its own on its loopback, the same whatever the link or the path). Programs have no memory limit.
- **A brace in `run` has to be doubled** (`{{`, `}}`), because a single one is a placeholder. A `sh -c` with JSON in it is easier to read as a script file.

## Start, ready, and the order

The programs start **in the order of the file, each when the one before is ready**; the receivers and the senders of the other workloads start when all of them are, so a server is there before the first packet. `ready` is one of five ways (a UDP or TCP socket in the node, a line of standard output, a file, a delay). A socket is read from the tables of the node (`/proc/<pid>/net`), which needs no help from the program and works for UDP; a socket bound only to the loopback does not count, because the other node cannot reach it. Without `ready` the program is ready when it is started.

A program that **ends before it is ready**, is **not ready within `within`**, or cannot be started, fails the run before any packet, with its id, what it was waited for, and the end of its standard error. The run keeps nothing running: the process groups are ended.

## While the workloads run

The run looks at every program every 50 ms or so: it reads the metric lines (below) and notices an end. A program that **ends by itself** with an exit code other than 0 or by a signal is, by `on-exit`:

- `continue` (default): the run goes on to the end, and the result has `ended: "on-its-own"` with the exit code or signal. A program that crashes under a bad network is a finding, not a fault of the tool; the summary warns, and `netscenario.Program.crashed` is true in Python.
- `fail`: the run stops with an error that names the program and gives the end of its standard error (exit code 4). The logs stay in the run directory.

A program that ends with 0 is not a failure either way.

## Stop

After the workloads and the wait for the packets, every program that is still going gets the signal of `shutdown` (default `TERM`) to its process group. After `within` (default 5 s) the group gets `KILL`. The result says `stopped` (it ended after the signal) or `killed`. The programs end before the receivers are collected, so their last lines count. A cancel (Ctrl-C) ends them too.

## Output

Standard output and standard error go to files of the run directory, `program-<id>.stdout.log` and `.stderr.log`: **the first megabyte of each**. The run reads the rest and drops it, so a program that prints without pause is never blocked; the result counts the bytes that are not in the files. A line of standard output is cut at 64 KiB (the cut lines are counted), and at most 10,000 lines wait to be read between two looks (`lines_dropped` says if more came, and metrics in them are lost). Nothing that was dropped is hidden: the summary has a warning on a line of its own for each.

## Metrics

A line of standard output that starts with `{` and has the key `metric` is a metric line:

```json
{"metric": "frames_per_second", "value": 29.7, "unit": "1/s"}
```

`metric` is a text of 1 to 128 characters without control characters, `value` a finite number, `unit` an optional text; **no other key** (a misspelled `valeu` should be seen, not read as a missing value). Anything else the program prints is its own output and is not looked at. A line that looks like a metric and is not valid is **counted, and the first five are kept with the reason** (`bad_metric_lines`, `bad_metric_examples`; a warning in the summary); it does not stop the run. A program reports at most 256 different metrics.

The result has a summary for each name (count, first, last, min, max, the unit of the first reading that had one) and `metrics-<id>.jsonl`, every reading as a line `{"t_ns": ..., "metric": ..., "value": ...}`. **The time of a reading is the time the run read the line**, in nanoseconds from the start of the measured phase, which is within 50 ms of when the program wrote it (negative before the phase began, for a program that reports while it starts). The exit code is not a metric: it is `exit_code` or `exit_signal` of the program.

## The result

`programs` in `result.json` has an entry for each program ([RESULTS.md](RESULTS.md)); in Python `result.programs["app"]` is a `netscenario.Program`. The summary of the command line prints each with its times, how it ended, its output and its metrics.

## What the run cannot count

A program sends packets of its own into the queues of its link. The run does not count or order them, so:

- the tally of the directions (`directions`) is empty and the queue is not held to the number of packets the workloads sent;
- if the queue of a direction the program sends into draws random numbers (loss, duplication, corruption, reordering), `seed.loss_reproducible` is false with the reason, as it is for two workloads in one direction ([D-065](decisions/D-065.md)).
