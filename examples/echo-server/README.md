# Your own program in a node

The scenario [`echo.yaml`](echo.yaml) starts a UDP echo server ([`echo_server.py`](echo_server.py)) in one node and a client ([`echo_client.py`](echo_client.py)) in the other, over a link with 30 ms of delay and 5 % loss each way. It shows what a `kind: command` workload does:

- the programs run **in the namespace of their node**, as you, in the directory of the scenario file, **without a shell** (`run` is a list); `{server.addr}` is the address of the node, and the environment has `NETSCENARIO_*` besides;
- the server is started first, and the client when the server has a UDP socket open on port 4433 (`ready: { udp-port: 4433, within: 5s }`: the run reads the sockets of the node, the server needs no help);
- the client **reports what it measured** as lines of JSON on its standard output, `{"metric": "rtt", "value": 61.2, "unit": "ms"}`, and the result has them: a summary for each name, and every reading with its time in a file;
- at the end the server gets `TERM` (and `KILL` after 2 s, if it ignores it); the output of both is in files of the run directory.

```bash
netscenario run examples/echo-server/echo.yaml
```

```text
program server  in server  python3 echo_server.py 10.100.0.2 4433
  started -0.521 s, ready -0.500 s, ended 6.367 s (stopped, signal 15)
  output  0 bytes in program-server.stdout.log, 0 bytes of standard error in program-server.stderr.log

program client  in client  python3 echo_client.py 10.100.0.2 4433 200
  started -0.500 s, ready -0.500 s, ended 4.028 s (on-its-own, exit code 0)
  output  10732 bytes in program-client.stdout.log, 0 bytes of standard error in program-client.stderr.log
  metric answered  1 readings, last 177, min 177, max 177
  metric rtt  177 readings, last 60.102605 ms, min 60.042706, max 60.467596
  metric sent  1 readings, last 200, min 200, max 200
```

(Times are in seconds from the start of the measured phase; a program is started before it, so its start is negative.)

[`test_echo.py`](test_echo.py) asserts on it with `pytest`: how many packets were answered (with bounds from the loss that is configured: 0.95 × 0.95 of 200, four standard deviations) and that no answer was faster than the two delays of the link allow.

**The lost packets of this run are not promised to repeat**, although the file has a `seed`: the packets of the programs enter the queue of the link in an order that the run does not control, and the result says so (`seed.not_reproducible_because`). The bounds are for that reason wide.

**A program that prints JSON from a shell command** has to double the braces in `run` (`{{` and `}}`), because a single brace is a placeholder; a script file, as here, has no such problem. See [docs/PROGRAMS.md](../../docs/PROGRAMS.md).
