# What a link can do

A link has two directions, `forward` (from the first node of `between`) and `reverse`, and each direction has its own impairments. `both` sets the same for both. Everything a direction does is a real kernel queue (`netem`, with `tbf` or `htb` for a rate), which is why a result can be trusted to be the network and not a model of it.

```yaml,scenario,run
# file: impaired.yaml
version: 0
name: impaired
seed: 8
duration: 4s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:
        delay: 40ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
        reorder: { share: 2%, gap: 5 }
        duplicate: 0.5%
        rate: 10mbit
      reverse:
        delay: 40ms
        rate: 20mbit
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
```

| Impairment | What it does |
| --- | --- |
| `delay`, `jitter` | A fixed delay and a variation around it: uniform, `normal`, `pareto`, `paretonormal`, or a shape taken from a file of measured round trips. |
| `loss` | `random` loss, or loss in bursts with a Gilbert-Elliott or a four-state model. |
| `reorder`, `duplicate`, `corrupt` | A share of the packets is sent on at once and overtakes the others, which wait for the delay (`reorder`, needs a `delay`); a share is sent twice (`duplicate`); a share is damaged by one flipped bit (`corrupt`). |
| `rate` | A rate limit. With a `queue` it has a buffer and a discipline (`fifo`, `codel`, `fq_codel`, `pie`), so a bottleneck behaves like one. |
| `mtu` | The packet size the link takes; larger packets are cut into fragments and a lost fragment loses the packet. |

The exact keys, their forms and the models of loss are defined in the [scenario format](https://github.com/leonkrix/netscenario/blob/main/docs/SCENARIO-FORMAT.md#impairments-of-a-direction).

## A queue makes a bottleneck

A rate limit alone drops what it cannot send. A queue in front of it holds packets, and its discipline decides what is dropped when it is full:

```yaml,scenario,run
# file: bottleneck.yaml
version: 0
name: bottleneck
seed: 4
duration: 5s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:
        delay: 10ms
        rate: 5mbit
        queue: { discipline: codel, limit: 100p }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 8mbit, packet-size: 1200b }
```

The stream offers 8 mbit into 5 mbit, so more than a third is dropped; CoDel keeps the delay it adds near its target instead of letting the buffer fill. Change `discipline` to `fifo` and compare the delay percentiles.

## A profile instead of numbers

A link can start from a **profile**: a named set of values from a source, such as `lte-good`. What you write next to it wins.

```yaml,scenario
# file: profile.yaml
version: 0
name: profile
seed: 7
duration: 5s
topology:
  nodes: [phone, server]          # the first node of `between` is the terminal
  links:
    - id: access
      between: [phone, server]
      profile: lte-good
      forward: { loss: { model: random, p: 1% } }
workloads:
  - { id: media, kind: udp-cbr, from: phone, to: server, rate: 2mbit, packet-size: 1200b }
```

`netscenario profiles` lists them and `netscenario profiles lte-good` shows one with its values and the sources they come from. A profile sets only what a source states; the [profile library](https://github.com/leonkrix/netscenario/blob/main/docs/PROFILES.md) lists what is there and what waits for a source.

## What the machine cannot do

`netscenario capabilities` lists the features this machine reproduces exactly. A link that asks for one it cannot (for example a seed on a `tc` older than 6.6.0) is refused before anything is built, with the feature, the backend and the way out. That refusal is the design: a run that silently did something else would be worth nothing.
