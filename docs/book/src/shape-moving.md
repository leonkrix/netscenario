# A link that moves

A phone in a car, a satellite passing overhead, a signal that comes and goes: the link is not one set of numbers but a **pattern over time**. There are three ways to write it, from the simplest to the most exact.

## States that repeat

A periodic pattern describes a link that is in view for a time, goes quiet for a handover, and is weak for a while, again and again. The link is written once with its values for the good state, and each state lists what differs:

```yaml,scenario,run
# file: pass.yaml
version: 0
name: pass
seed: 4
duration: 9s
topology:
  nodes: [terminal, gateway]
  links:
    - id: hop
      between: [terminal, gateway]
      both: { delay: 35ms, rate: 20mbit, queue: { limit: 100p } }
      model:
        kind: periodic
        states:
          - { id: in-view, for: 3s }
          - { id: handover, for: 1s, outage: true }
          - { id: weak, for: 2s, set: { both: { rate: 4mbit, loss: { model: random, p: 1% } } } }
workloads:
  - { id: stream, kind: udp-cbr, from: terminal, to: gateway, rate: 3mbit, packet-size: 1200b }
measure:
  series: { bin: 500ms }
```

In the handover nothing is delivered, and in the weak state the link has 4 mbit and a little loss. A low orbit satellite is the same pattern with 40 s, 6 s and 14 s: [`scenarios/leo-pass.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/leo-pass.yaml). The same idea with random states is a **Markov chain**: three signal states, a mean time in each drawn from the seed, and the chances of the next one ([`scenarios/signal-states.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/signal-states.yaml)). The same seed gives the same states and the same lost packets.

## A trace you recorded

A drive through a cell can be a CSV file of moments and values. Save this as `cell-drive.csv`:

```text,file=cell-drive.csv
time,rate,delay,loss
0s,18mbit,38ms,0%
2s,14mbit,40ms,0%
4s,6mbit,55ms,1%
6s,2mbit,80ms,3%
8s,18mbit,38ms,0%
```

and refer to it from the link:

```yaml,scenario,run
# file: cell.yaml
version: 0
name: cell
seed: 5
duration: 10s
topology:
  nodes: [phone, server]
  links:
    - id: cell
      between: [phone, server]
      both: { queue: { limit: 200p } }
      model:
        kind: trace
        both: { file: cell-drive.csv }
workloads:
  - { id: stream, kind: udp-cbr, from: server, to: phone, rate: 6mbit, packet-size: 1200b }
measure:
  series: { bin: 1s }
```

The file is read once when the scenario is loaded and **lives in the recorded scenario**, so a replay needs nothing from the original directory. The trace begins again when it is over. A Mahimahi trace works the same way for a rate that follows a recorded sequence of delivery opportunities; the Kernel backend follows it in windows, which is only an approximation, so the scenario has to accept that with `allow-limited` ([`scenarios/mahimahi-walk.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/mahimahi-walk.yaml)). Nothing is approximated without you saying so.

## Events that repeat

An outage every ten seconds, a rate that halves every twenty: `every` with `until` or `count` on a timeline event ([Changes over time](timeline.md)).

```yaml,scenario,run
# file: flapping.yaml
version: 0
name: flapping
seed: 2
duration: 8s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 20ms, rate: 10mbit }
workloads:
  - { id: stream, kind: udp-cbr, from: client, to: server, rate: 4mbit, packet-size: 1200b }
timeline:
  - { at: 2s, link: access, outage: 400ms, every: 3s, until: 7s }
measure:
  series: { bin: 250ms }
```

Two outages fall in the 8 seconds, and for each one the result has the time the stream took to recover. A test can assert that a stream is back at its level within a number of seconds, which is the question after a handover.
