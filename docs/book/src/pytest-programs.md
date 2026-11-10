# A program of your own in the network

A workload can also be **your own program**, started in a node: a server, a client, the service you are testing. It then sends real packets through the network, and you test what it does over a bad link instead of only what the generators measure.

`command` runs code, so it is written in the scenario file where everybody can see it. The example is a UDP echo server on one side and a client on the other, over a link with 30 ms of delay each way and 5 % loss each way:

```yaml,scenario
{{#include ../../../examples/echo-server/echo.yaml:2:}}
```

Three things in it are worth knowing:

- **`{server.addr}`** is the address of a node, filled in when the network is built.
- **`ready`** says when the program is ready: the client is started when the server has a UDP socket open on port 4433, not after a guessed sleep.
- **`shutdown`** says how a program that does not end by itself is stopped (here `SIGTERM`, then the run waits up to two seconds).

A program reports what it measured as lines of JSON on its standard output, `{"metric": "rtt", "value": 61.2}`, and the result has them. The test reads the exit of each program and its metrics:

```python
{{#include ../../../examples/echo-server/test_echo.py}}
```

The bounds in that test are derived and written next to it: a packet is answered if neither it nor its answer is lost, so 90.25 % of 200 packets is about 180 with a standard deviation of 4.2, and no answer comes faster than the 60 ms the link takes both ways.

The programs run in the directory of the scenario file (change it with `working_dir=`), without a shell, with the environment you give them. Read [Programs of your own](https://github.com/leonkrix/netscenario/blob/main/docs/PROGRAMS.md) for the limits a run sets on them, and the [security notes](https://github.com/leonkrix/netscenario/blob/main/SECURITY.md) before you run a scenario file you did not write: a `command` is code.
