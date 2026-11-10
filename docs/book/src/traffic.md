# Traffic

A `workload` is something that sends packets through the network. All of them run at the same time, each from a node to a node, and each has its own numbers in the result.

## UDP streams

The constant bit rate stream is the simplest: a packet of a fixed size at a fixed rate. Three more patterns are drawn from the seed, so they repeat:

```yaml,scenario,run
# file: patterns.yaml
version: 0
name: patterns
seed: 7
duration: 6s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 20ms }
workloads:
  - { id: steady, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
  - { id: requests, kind: udp-poisson, from: client, to: server, rate: 800kbit, packet-size: 400b }
  - id: sensor
    kind: udp-onoff
    from: client
    to: server
    rate: 400kbit
    packet-size: 200b
    on: 500ms
    off: 1500ms
    distribution: exponential
  - id: video
    kind: udp-bursty
    from: server
    to: client
    packet-size: 1200b
    burst-packets: 10
    gap: 200us
    pause: 100ms
    distribution: exponential
```

`udp-cbr` is a constant rate, `udp-poisson` has exponential gaps, `udp-onoff` sends in phases and pauses, and `udp-bursty` sends bursts of packets that follow each other closely. Every packet carries a sequence number and a send time, so the receiver can tell **lost**, **reordered**, **duplicated** and **damaged** apart, and measure the one-way delay exactly (both ends read the same clock).

For each stream the result has the counts, the delay percentiles, the jitter (RFC 3550), the loss runs (how the lost packets cluster), the reordering and the throughput over time. A recorded stream can be replayed from a file with `kind: trace-replay` (the moments and sizes of a real call, from a CSV file or a capture), and `echo: true` sends every packet back so you get the round trip.

## TCP

TCP workloads use the kernel TCP of the machine. The congestion control is a key, and the result has the goodput, the retransmissions and the round trip of each connection:

```yaml,scenario,run
# file: download.yaml
version: 0
name: download
seed: 5
duration: 8s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 20ms, rate: 20mbit }
workloads:
  - { id: download, kind: tcp-bulk, from: server, to: client, bytes: 3mb, connections: 2, cc: reno }
```

`tcp-bulk` moves a number of bytes (or sends until `stop`). `request-response` is an API: a request of a size every so often over a number of connections, and the result has the time of each answer:

```yaml,scenario,run
# file: api.yaml
version: 0
name: api
seed: 9
duration: 5s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: 30ms, loss: { model: random, p: 0.5% } }
      reverse: { delay: 30ms }
workloads:
  - id: api
    kind: request-response
    from: client
    to: server
    request: 200b
    response: 4kb
    connections: 4
    interval: 20ms
    timeout: 5s
```

TCP is statistical: a connection's goodput depends on the start-up, so judge a window after it and repeat the run. [A dumbbell](shape-dumbbell.md) shows the classic fairness test.

## Your own program

`kind: command` starts a program in a node ([A program of your own in the network](pytest-programs.md)). It can run next to the generators, so the generators can load the link while your program is tested.

## What a run measures

The section `measure` says what is recorded beyond the numbers: the width of the bins of the series, a **pcap capture** of the interfaces of a link at both ends, and a **record of every packet** (`packets: true`), which the tables of the Python layer read. The [results reference](https://github.com/leonkrix/netscenario/blob/main/docs/RESULTS.md) lists the files.
