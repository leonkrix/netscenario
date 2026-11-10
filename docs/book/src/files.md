# The scenario file

A scenario is a short YAML file. It says **what the network is**, **what traffic goes through it** and, if you want, **what must come out**. The same file runs from the command line, from Python and from the pytest plugin.

```yaml,scenario
# file: annotated.yaml
version: 0                      # the format of the file
name: annotated                 # shown in results and reports
seed: 3                         # one seed for every random part; leave it out and a run draws one and prints it
duration: 10s                   # how long the workloads run
topology:
  nodes: [client, server]       # a node is a host (or, with `role: router`, a router)
  links:
    - id: access
      between: [client, server] # the first node is where "forward" starts
      both: { delay: 20ms, rate: 10mbit }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
```

## The parts

| Key | What it holds |
| --- | --- |
| `version`, `name`, `seed`, `duration` | The format, a name, the seed and how long the traffic runs. |
| `topology` | The nodes and the links between them, or a shortcut for a shape (`chain`, `star`, `dumbbell`, `multipath`). Routers and routes. |
| `workloads` | The traffic: UDP streams, TCP transfers, request/response, a recorded trace, or a program of your own. |
| `timeline` | Changes at moments: a rate that drops, an outage, a link that goes down and comes back. |
| `measure` | What is recorded beyond the numbers: the bins of the series, packet captures, a record of every packet. |
| `expect` | What the run must show; the verdict is part of the result. |
| `params` | Values that the file takes from the command line or from a test. |

## Values have units

A number always has its unit, so a wrong unit is an error and not a silent factor of a thousand: `20ms`, `2s`, `10mbit`, `1500kbit`, `1200b`, `1%`. An unknown key is refused with the line, the column and a hint, never ignored:

```text
error: `both` cannot be combined with `forward` or `reverse`
 --> scenarios/invalid/both-and-forward.yaml:9:13
  |
8 |       between: [client, server]
9 |       both: { delay: 10ms }
  |             ^
  = path: topology.links[0].both
  = help: use `both` for the same values in both directions, or `forward` and `reverse` for different ones
```

## Look before you run

```bash
netscenario validate annotated.yaml     # reads and checks, builds nothing
netscenario show annotated.yaml         # the nodes, the links, the routes and the path of each workload
netscenario expand annotated.yaml       # the long form, where a shortcut has become nodes and links
```

```text
annotated: 2 nodes, 1 links, routing auto

nodes
  client host
  server host

links (forward is from the first node to the second)
  access client - server
      both     delay 20ms, rate 10mbit

routes (the smaller metric is used first; * is listed in the file)
  client to server via access next server metric 100
  server to client via access next client metric 100

paths of the workloads
  media: client -[access]-> server
```

`show` is how you see that the network is the one you meant before a run spends time on it.

## An editor that knows the format

`netscenario schema` prints the JSON Schema of the format. Put this as the first line of a file and an editor with the YAML language server completes keys and checks values while you write:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/leonkrix/netscenario/main/docs/schema/scenario-v0.json
```

## Parameters, templates and includes

A value can be an expression over a parameter (`"{rtt / 2}"`), a link that is needed twice can be a template, and a part of a file can be included from another file. [`scenarios/parameters.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/parameters.yaml), [`scenarios/templates.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/templates.yaml) and [`scenarios/include.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/include.yaml) show each; the [scenario format](https://github.com/leonkrix/netscenario/blob/main/docs/SCENARIO-FORMAT.md) defines them. A file a scenario refers to (a trace, a table of delays) is read once when the scenario is loaded and lives in the recorded scenario, so a replay needs nothing from the original directory.
