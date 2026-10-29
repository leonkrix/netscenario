# Scenario format (draft 0)

**Status: draft.** This is the shape the format is expected to have. It changes freely until 0.1 is released. Anything not in this document is not part of the format. A JSON Schema is generated from the Rust types (`netscenario schema`), and that schema, not this page, is the final authority once it exists.

The format is YAML (JSON is accepted, since it is a subset). A scenario can also be built in code with the same model.

## Overview

```yaml
version: 0            # format version, required
name: lte-burst-loss  # for reports
seed: 42              # one seed; component seeds are derived from it
duration: 30s
backend: kernel       # optional: kernel | engine | proxy | auto (default auto)
topology: { ... }     # nodes and links
timeline: [ ... ]     # events over time
mobility: { ... }     # positions over time (0.6)
workloads: [ ... ]    # what runs on the nodes
probes: [ ... ]       # extra measurements
expect: [ ... ]       # optional assertions
```

## Values

| Kind | Examples | Rules |
| --- | --- | --- |
| Duration | `40ms`, `2.5s`, `1m` | unit required; `ns`, `us`, `ms`, `s`, `m`, `h` |
| Rate | `10mbit`, `500kbit`, `1gbit` | bits per second; `bps` accepted; no bare numbers |
| Size | `1200b`, `64kb`, `1mb` | bytes |
| Probability | `2%`, `0.02` | `0` to `1` |
| Name | `client`, `access` | letters, digits, `-`; unique inside its kind |

A number without a unit is an error. A unit in the wrong place is an error. The error names the path in the file.

## Topology

```yaml
topology:
  nodes:
    - client
    - { id: router, role: router }
    - server
  links:
    - id: access
      between: [client, router]
      forward:                      # client -> router
        delay: 40ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
        rate: 10mbit
        queue: { limit: 100p, discipline: fifo }
      reverse:                      # router -> client
        delay: 40ms
        rate: 20mbit
    - id: core
      between: [router, server]
      both: { delay: 5ms, rate: 1gbit }   # same in both directions
```

- A link has two directions, `forward` (first node to second) and `reverse`. `both` sets both.
- Addresses are assigned by the tool (`10.<scenario>.<link>.<n>`; IPv6 from a unique-local prefix), unless a node or link sets them. Programs get them from the environment (see Workloads).
- Routing is derived for chains and stars; for other graphs, `routes` can be given.
- Shortcuts for common shapes are planned: `topology: { kind: dumbbell, senders: 2, bottleneck: { rate: 10mbit, delay: 20ms } }`.

### Loss models

```yaml
loss: { model: random, p: 2% }
loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }
loss: { model: markov4, p13: 2%, p31: 30%, p32: 10%, p23: 20%, p14: 0.5% }
loss: { model: trace, file: traces/lte-loss.csv }
```

`p` is the chance of entering the lossy state, `r` the chance of leaving it. The names follow the `netem` documentation so that the mapping is checkable.

## Timeline

```yaml
timeline:
  - { at: 10s, link: access, set: { forward.rate: 2mbit } }
  - { at: 20s, link: access, outage: 2s }
  - { at: 25s, every: 1s, until: 30s, link: access, set: { forward.delay: 80ms } }
```

An event has a time (`at`), a target (`link`, optionally a direction) and an action (`set`, `outage`, `down`, `up`, `replace-model`). Times are relative to the start of the workloads, not to the setup. The result records when each event was really applied.

## Link models

```yaml
topology:
  links:
    - id: air
      between: [drone, ground]
      model:
        kind: geometry                 # 0.6
        propagation: log-distance
        frequency: 2.4ghz
        exponent: 2.7
        rate-by-snr: table/wifi-rates.csv
    - id: radio
      between: [a, b]
      model: { kind: trace, file: traces/verizon-lte.up, format: mahimahi }   # 0.3
```

A model replaces the static values of the link it is attached to. The kinds: `static` (default), `steps` (what `timeline` is shorthand for), `trace`, `markov`, `periodic`, `geometry`, and `plugin`.

## Mobility (0.6)

```yaml
mobility:
  space: { dimensions: 3, unit: m }
  nodes:
    drone:
      model: waypoints
      start: [0, 0, 100]
      path: [{ to: [2000, 0, 100], speed: 25 }, { to: [2000, 2000, 150], speed: 25 }]
    ground: { model: static, at: [0, 0, 2] }
    sat:    { model: orbit, file: orbits/leo-1.tle }
```

Other models: `random-waypoint`, `random-walk`, `gauss-markov`, and `trace` (CSV, BonnMotion, ns-2 `setdest`). Links with a `geometry` model read the positions.

## Workloads

```yaml
workloads:
  - id: media                       # built-in traffic generator
    kind: udp-cbr
    from: client
    to: server
    rate: 2mbit
    packet-size: 1200b
    start: 0s
    stop: 30s

  - id: app                         # your program
    kind: command
    node: server
    run: ["./target/release/my-server", "--listen", "{server.addr}:4433"]
    ready: { udp-port: 4433, within: 5s }
    stop: { signal: TERM, within: 3s }
    metrics: { stdout: json-lines }
```

- `{node.addr}` and `{link.node.addr}` in commands, and the environment variables `NETSCENARIO_ADDR_<NODE>`, give your program the addresses.
- A `command` runs in the namespace of its node, as the invoking user, with the working directory of the scenario file. It is the only way for a scenario to run code, and it always says so in the file.
- Built-in kinds (see FEATURES.md): `udp-cbr`, `udp-poisson`, `udp-onoff`, `trace-replay`, `tcp-bulk`, `request-response`, and later `container`.
- The agent that runs a built-in workload writes per-packet records into the result.

## Probes and expectations

```yaml
probes:
  - { id: cap, kind: capture, link: access }       # packet capture of a link

expect:
  - { metric: media.delivered_ratio, ci95-low: ">= 0.97" }
  - { metric: media.delay.p99, "<": 150ms }
repetitions: 10
```

`expect` is for simple thresholds that belong with the scenario. Anything more is test code in Python or Rust.

## Parameters and sweeps (0.3, 0.5)

```yaml
params:
  loss: 2%
  rtt: 80ms
topology:
  links:
    - id: access
      between: [client, server]
      both: { delay: "{rtt / 2}", loss: "{loss}" }
matrix:
  loss: [0%, 1%, 2%, 5%, 10%]
  rtt: [20ms, 80ms, 300ms]
```

Expressions are deliberately small (units, `+ - * /`); the scenario file is data, not a programming language.

## Limited features

A backend that can do a feature only in part refuses it unless the scenario accepts the restriction:

```yaml
allow-limited: [trace-driven-delay]
```

The result records every accepted restriction.

## Open questions

1. How strict should address assignment be (fixed plans versus freely chosen)?
2. Should `timeline` and `model: steps` be one thing? (Probably yes; the timeline is the readable shorthand.)
3. Is a separate `environment` section (kernel version, permissions) a requirement of a scenario, or only a property of the run? (Probably the second.)
4. Names of the loss-model parameters: `netem` names or readable names, or both with aliases?
5. How are traces versioned and shared (inline, files, content hashes)?
6. Where do sweeps end and a test harness begin? (The format only describes the grid; running and judging it is the job of the tool.)
