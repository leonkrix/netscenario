# Scenario format, version 0 (release 0.1)

**Status: the contract for release 0.1, with the impairments, the timeline and the program of release 0.2 added** (the format reads them; the Kernel backend applies the loss models, the reordering, the duplication and the corruption, the whole timeline (`set`, `outage`, `down`, `up`) and the statistics of them, and refuses what it does not apply yet - the program - and says so). Part 1 describes exactly what a scenario file may contain. Anything not in Part 1 is not part of the format and is refused with an error that says in which release it is planned. Part 2 sketches what comes later and is not binding. A JSON Schema is generated from the Rust types (`netscenario schema`, SCN-01, committed as `docs/schema/scenario-v0.json`); that schema and the tests are the final authority, and this page is checked against them.

The format is versioned with the line `version: 0`. **Version 0 is the series before 1.0: it may change in any minor release, and the changelog says how.** From 1.0 on the version is 1, and a change that breaks files raises it (SCN-08 adds `netscenario migrate`).

## Principles

1. **Strict.** An unknown key, a duplicate key, a value of the wrong type, a number without a unit, a unit that does not exist: each is an error. A file means the same thing everywhere.
2. **One way to say a thing.** There are no aliases and no two spellings of one unit.
3. **Data, not code.** A scenario cannot compute or run anything. (A workload that runs your program says so, from release 0.2.)
4. **Refuse, do not approximate.** A value that a backend cannot apply exactly is an error, not a rounded value (D-005).
5. **Errors point at the file.** See "Errors" below.

## YAML rules

The format is YAML 1.2; JSON is accepted because it is a subset. In addition:

- One document per file, UTF-8, at most 1 MiB, nesting at most 32 levels deep. An empty file is an error.
- Keys are unique inside a mapping.
- **Anchors, aliases, merge keys (`<<`) and tags are refused.** A scenario reads the same from top to bottom. Reuse is designed on purpose in release 0.3 (parameters, includes, templates), and not through YAML features.
- Comments are free.

## The file at a glance

```yaml
version: 0              # required: the format version
name: lte-like-link     # required: a Name, used in reports and file names
seed: 42                # optional: one seed, the seeds of the parts are derived from it
duration: 30s           # required: the length of the measured phase
backend: auto           # optional: auto (default) or kernel
topology: { ... }       # required: the nodes, the links and the routes
workloads: [ ... ]      # required: what sends traffic, at least one
timeline: [ ... ]       # optional: changes over time (release 0.2)
measure: { ... }        # optional: what is measured besides the statistics (release 0.2)
```

A complete scenario of release 0.1:

```yaml
version: 0
name: lte-like-link
seed: 42
duration: 30s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:                     # client -> server
        delay: 40ms
        jitter: { amount: 5ms, distribution: normal }
        loss: { model: random, p: 1% }
        rate: 10mbit
      reverse:                     # server -> client
        delay: 40ms
        rate: 20mbit
workloads:
  - id: media
    kind: udp-cbr
    from: client
    to: server
    rate: 2mbit
    packet-size: 1200b
```

### Top level

| Key | Required | Meaning |
| --- | --- | --- |
| `version` | yes | The integer `0`. Any other number is an error that says which versions this tool reads. |
| `name` | yes | A Name. Appears in the result and names its files. |
| `seed` | no | An unsigned 64-bit integer. Without it the tool draws one and records it in the result, so every run can be repeated (ENV-01), provided the backend can apply a seed to its loss (the Kernel backend with iproute2 6.6.0 or newer). Where it cannot, a scenario with loss still runs, and the result says that the loss is not reproducible. |
| `duration` | yes | A Duration greater than zero. The measured phase runs from the start of the workloads to this time. The setup and the teardown are not part of it. |
| `backend` | no | `auto` (the default) or `kernel`. `engine` and `proxy` are refused until releases 0.4 and 0.5. |
| `topology` | yes | See below. |
| `workloads` | yes | A list with at least one workload. |
| `timeline` | no | A list of events, see [Timeline](#timeline). |
| `measure` | no | The width of the bins of the series, how the time to recover is worked out, the links to capture and the export of every packet, see [What is measured](#what-is-measured). |

## Values

All values are strings or numbers in the file; this table says what is accepted. A number is decimal digits with an optional fraction (`40`, `2.5`): no sign, no exponent, no digit separators. **A unit follows the number directly, without a space, in lower case.** A value that is not a whole number of the smallest unit of its kind is an error (`0.5ns`), because it cannot be applied exactly.

| Kind | Units | Examples | Rules |
| --- | --- | --- | --- |
| Duration | `ns`, `us`, `ms`, `s`, `m`, `h` | `40ms`, `2.5s`, `1m` | A whole number of nanoseconds. Zero is allowed where a delay is meant. |
| Rate | `bit`, `kbit`, `mbit`, `gbit`, `tbit` | `10mbit`, `500kbit` | Bits per second, decimal (`kbit` is 1,000). A whole number of bits per second, greater than zero. There is no `bps` and no `Mbit`. |
| Size | `b`, `kb`, `mb`, `gb`, `kib`, `mib`, `gib` | `1200b`, `64kb`, `1mib` | Bytes. `kb` is 1,000 bytes, `kib` is 1,024. A whole number of bytes. |
| Probability | `%` | `2%`, `0.1%`, `100%` | From `0%` to `100%`, at most six digits after the point. **Only with the percent sign:** `0.02` or `1` is an error, because `1` could mean 1 % or 100 %. |
| Name | none | `client`, `access-1` | 1 to 32 characters: lower case letters, digits and `-`; starts with a letter, does not end with `-`. Unique among nodes, among links and among workloads. |

`netem` stores probabilities with a resolution of about 2·10⁻¹⁰, finer than the six digits that the format allows, so every accepted probability is applied without rounding that matters.

## Topology

```yaml
topology:
  nodes:
    - client
    - { id: edge, role: router }
    - server
  links:
    - id: access
      between: [client, edge]
      forward:  { ... }
      reverse:  { ... }
    - { id: delivery, between: [edge, server] }
  routing: auto          # or manual
  routes: [ ... ]        # optional
```

A topology is one graph of nodes and links (D-078). Release 0.3 reads any number of nodes and links; a scenario of release 0.1 or 0.2 (two nodes and one link) is read as before and keeps its text and its hash.

- **Nodes.** A node is written as its name (a host) or as `{ id, role }`. A **host** sends and receives and does not forward; a **router** forwards between its links. A node that lies on a path between two others has to be a router, and the error says which. At least two nodes, at most 256; every node has at least one link; the name `default` is taken (it is the word for every destination in a route).
- **Links.** A link has an `id` (a Name) and `between`: two different nodes. Two nodes may have **several** links (multipath, multi-homing). The first is the "forward" side: `forward` is the direction from the first node to the second, `reverse` the way back. Impairments are set per direction (LNK-11); `both` sets the same values in both directions and **cannot be combined with `forward` or `reverse`**. A direction that is not mentioned has no impairment. `cost` (a whole number, 1 to 1,000,000, 1 by default) is what the link costs the automatic routing.
- **Routing.** `routing: auto` (the default) computes a route from every node to every node by the shortest path: the smallest sum of the `cost` of the links, passing routers only. Equally short paths: the first link in the order of the file is taken, so the result is the same on every run; `ecmp: true` installs all of them as a multipath route. `routing: manual` switches the automatic routes off, and only the listed routes exist.
- **Routes.** A route is `{ node, to, via }` with an optional `metric` and `weight`. `node` has the route; `to` is a node or `default` (every destination that has no route of its own); `via` is the **link** the packets leave by, and the node at its other end is the next hop (a link and not a neighbour, so that two links between the same nodes are told apart). The smaller `metric` is used first (100 by default), which makes a failover: when the interface of the first goes down, the second takes over. Routes with the same `metric` share the packets by their `weight` (1 by default, up to 256). With `routing: auto` a listed route replaces the automatic routes of the same node and destination and leaves all others alone; with `manual` the listed routes are all there is. A scenario is checked for a path over the routes for every workload that sends packets: a node without a route, a route through a host, and routes that lead in a circle are refused with the node where it goes wrong.
- **Addresses.** The tool assigns every address and records it in the result; they are not part of the file. Every link gets a subnet of its own and every node an address of its own on its loopback, which workloads and programs use (`{server.addr}`), so a node with several links needs no choice of address. IPv4 only in release 0.3 so far (TOP-08). A key `address` is reserved for a later release.
- **Which backends.** The Kernel backend builds networks of several nodes: every node gets its address on the loopback (`10.200.0.<n>`, the placeholders and the workloads use it), routers forward, and the routes are set in each node (docs/COVERAGE.md). The result lists the nodes, the routes and the path of each workload (docs/RESULTS.md).

### Shortcuts

For the common shapes a topology is written as one shortcut instead of `nodes` and `links` (D-078). The shortcut is **expanded when the file is read**: what is run, hashed, replayed and written out again is the long form, and `netscenario expand` prints it. The names of what is made are fixed, so that a route or an event of the timeline can name them.

| Shortcut | Keys | What it makes |
| --- | --- | --- |
| `chain` | `nodes` (at least 2), `link`, `links` | The first and the last node are hosts, the ones between are routers. A link between each pair, named `<first>-<second>`, its forward direction from the first to the second. `link` is the values of every link, `links` is a list with one entry for each link (instead of `link`). |
| `star` | `hub`, `leaves` (at least 2), `link`, `links` | The hub is a router, the leaves are hosts. A link from each leaf to the hub, named `<leaf>-<hub>`, its forward direction towards the hub. `link` or `links` (one for each leaf). |
| `dumbbell` | `left`, `right` (at least 1 each), `access`, `bottleneck` | The routers `router-left` and `router-right`, a link `<host>-router-left` from each host on the left and `<host>-router-right` from each host on the right (forward from the host to its router, values from `access`), and the link `bottleneck` from `router-left` to `router-right` (values from `bottleneck`). |
| `multipath` | `from`, `to`, `paths` (at least 2): each `{ routers, link }` | `from` and `to` are hosts; each path is a chain of its `routers` (at least one) from `from` to `to`, its links named `<a>-<b>` and given the values of `link`. |

The values of a link (`link`, `links`, `access`, `bottleneck`) are `cost`, `forward`, `reverse` and `both`, with the same rules as the values of a link in the long form. `routing`, `ecmp` and `routes` may stand next to a shortcut; `nodes` and `links` may not (write the long form for that). A shortcut that makes a name twice (a host called `router-left` in a dumbbell), a name longer than 32 characters or a count that does not fit is refused, and a fault in the values of a link is shown at the place in the shortcut.

```yaml
topology:
  dumbbell:
    left: [alice, bob]
    right: [carol, dave]
    access:     { both: { delay: 2ms, rate: 100mbit } }
    bottleneck: { both: { delay: 20ms, rate: 10mbit } }
```

The shortcuts are in [scenarios/dumbbell.yaml](../scenarios/dumbbell.yaml) and [scenarios/star-and-chain.yaml](../scenarios/star-and-chain.yaml); a chain of two routers written out is in [scenarios/three-hops.yaml](../scenarios/three-hops.yaml), a failover with listed routes in [scenarios/two-paths.yaml](../scenarios/two-paths.yaml) and a computed network with one route changed by hand in [scenarios/manual-routes.yaml](../scenarios/manual-routes.yaml).

### Impairments of a direction

The stages of a direction act in this order, whatever order the keys have in the file: **`duplicate`** (a packet may be sent twice, and the copy goes through everything that follows on its own, so with a loss of 10 % a duplicated packet is lost twice as often as it is lost once, D-048), **`loss`** (a packet may be dropped when it enters the direction and then uses no capacity), **`corrupt`** (a bit of the packet may be flipped), **`rate`** (the packets that are kept are sent one after the other at the rate, in a queue, in the order they came), **`delay` with `jitter`** (each packet then travels for its own delay, drawn independently; with **`reorder`** a share of the packets is not delayed at all). So a jitter does not slow the stream down, and it can make a packet overtake the one before it when the jitter is larger than the gap between them: the result counts such packets (`reordered`, [RESULTS.md](RESULTS.md)). A backend that cannot build exactly this refuses the scenario; the Kernel backend builds it ([COVERAGE.md](COVERAGE.md), [FIDELITY.md](FIDELITY.md)).

```yaml
forward:
  delay: 40ms
  jitter: { amount: 5ms, distribution: normal }
  loss: { model: random, p: 1% }
  rate: 10mbit
```

and with the impairments of release 0.2:

```yaml
forward:
  delay: 40ms
  loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }
  reorder: { share: 5% }
  duplicate: 0.5%
  corrupt: 0.2%
```

| Key | Value | Meaning in the Kernel backend |
| --- | --- | --- |
| `delay` | Duration | `netem delay` (LNK-01). |
| `jitter` | `amount`: Duration, `distribution`: `uniform` (default) or `normal` | `netem delay D J`, with `distribution normal` for the second one (LNK-02). **Needs `delay`.** A delay below zero cannot exist, and `netem` would silently clamp it to zero, so the format refuses a jitter that reaches below zero: `uniform` needs `amount` not greater than `delay`, `normal` needs `delay` at least four times `amount` (the tables of `netem` are cut at about four sigma, which is also why a normal jitter never reaches its far tail, see COVERAGE). |
| `loss` | `model` and its parameters, see below | `netem loss` (LNK-03, LNK-04). `trace` is planned for 0.3. |
| `reorder` | `share`: Probability, `gap`: whole number, at least 1 (optional, default 1) | `netem reorder P gap G` (LNK-05). **Needs `delay`** (`tc` refuses it without). `share` is the share of the packets that are sent on at once; the others wait for the delay and arrive after later ones. With `gap` the share sent on at once is `1 / (gap - 1 + 1 / share)`: 25 % with `gap: 5` is 12.5 %. Measured in D-048. |
| `duplicate` | Probability | `netem duplicate` (LNK-06). The copy is impaired on its own. |
| `corrupt` | Probability | `netem corrupt` (LNK-07). One bit of the frame is flipped, anywhere in it, the Ethernet header included; a receiver with the UDP checksum on drops the packet (D-040), so the program sees loss, and the result tells the two apart (`directions`, [RESULTS.md](RESULTS.md)) and names most of them by sequence number (`corrupted_seq`). The share of packets that are dropped is a little less than `P`: a bit in the source address of the Ethernet header harms nothing. |
| `rate` | Rate | `netem rate` (LNK-08). |

**The loss models.** The parameters are those of `netem` and have its names, without aliases; every parameter of a model is required (a default of `tc` that the file does not show would be a value nobody wrote). There is no `correlation` key for any loss, duplication or reordering, because the correlation argument of `netem` does not keep the probability (D-048).

| `model` | Parameters | `netem` |
| --- | --- | --- |
| `random` | `p`: the chance that a packet is lost | `loss random p` |
| `gilbert-elliott` | `p`: the chance, for each packet, to go from the good to the bad state; `r`: from the bad to the good state; `loss-bad`: the chance that a packet is lost in the bad state; `loss-good`: the same in the good state | `loss gemodel p r loss-bad loss-good` (the third and fourth argument, measured in S12). The mean stay in the bad state is `1 / r` packets. |
| `markov4` | `p13`, `p31`, `p32`, `p23`, `p14`: the transitions between four states (1 receiving in a gap, 2 receiving inside a burst, 3 losing inside a burst, 4 losing a single packet in a gap) | `loss state p13 p31 p32 p23 p14`. From state 1 the chances `p13` and `p14` are two ways out of one draw, and so are `p31` and `p32` from state 3: each pair adds up to at most 100 %, and the reader refuses more. |

```yaml
loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }
loss: { model: markov4, p13: 2%, p31: 30%, p32: 10%, p23: 20%, p14: 0.5% }
```


The queue of the link is the default of the kernel (`limit 1000` packets) and is recorded in the result. A `queue` key (limit and discipline, LNK-09) arrives with release 0.3.

## Timeline

`timeline` is a list of events. An event happens at a moment of the measured phase, to one link, and does exactly one of four things. The moment is before the end of the measured phase; events at one moment are applied in the order of the file, and the list does not have to be in time order.

```yaml
timeline:
  - { at: 10s, link: access, set: { forward: { rate: 2mbit } } }
  - { at: 20s, link: access, outage: 2s }
  - { at: 25s, link: access, set: { forward: { rate: null } } }
  - { at: 28s, link: access, down: true }
  - { at: 29s, link: access, up: true }
```

| Key | Value | Meaning |
| --- | --- | --- |
| `at` | Duration | The moment, from the start of the measured phase. Required. |
| `link` | the `id` of a link | Required. |
| `set` | `forward`, `reverse` and `both`, each with the keys of an impairment | Changes values of the link from this moment on. See below. |
| `outage` | Duration | Nothing is delivered for this long, **not even a packet that was on its way when it began**, and the sender notices nothing (D-039, D-064). The window is in the time of the receiver: it begins at `at` and ends `outage` later, and every packet that leaves the link in it is lost. It changes no value of the link, which act as before and may be changed by a `set` inside the window; afterwards the link is what the file says it is. |
| `direction` | `forward`, `reverse` or `both` (the default) | For an `outage` only. |
| `down` | `true` | The interface of the link goes down at both ends: a sender gets errors, the agent counts them as `send_errors` and lists the packets as `refused`, and the packets in the queues are dropped (D-039, D-064). A packet that is refused is not sent and not lost. |
| `up` | `true` | The interface comes up again. |

**`set` is a merge patch** (the rule of RFC 7386): a key that is not written stays as it is, a value replaces the old one as a whole (a `loss` replaces the loss, it does not merge into it), and `null` takes the impairment away. `both` changes the two directions. After every step the direction must be a valid set of impairments, and the reader checks it: `delay: null` while a `jitter` is set is refused, and the message says that it is this step.

**What the reader refuses besides the types:** an event that does none or several of the four things; a link that does not exist (with a hint); a moment at or after the end; an outage that is not longer than zero, that ends after the end of the measured phase, or that overlaps another outage of the same direction (the end of the first would be unclear); a link that goes down twice or comes up while it is up; a `direction` on anything but an outage; a step that writes no key.

**What the Kernel backend cannot follow.** The jitter of a direction keeps one distribution for the whole run: `netem` cannot go back from normal to uniform without building the queue again (D-059). The amount can change.

**When it happens.** An event is applied **before the first packet that its workload sends at or after its moment** (D-038, D-062): the sender of the workload that sends into the direction of the step knows its packet clock, asks the run right after the packet before, and holds the packet at its slot until the run has done the step. For that workload this is the same as a step at the exact moment, whatever the machine does, so the same seed gives the same lost packets; the packet that waits leaves late by the time of the change (`max_lateness_ns` shows it) and the ones after it catch up in order. The result records, for each step, the number of the first packet that met it and how long that packet waited (`events`, [RESULTS.md](RESULTS.md)). A step that no workload can wait for is done by the clock at its moment: no workload sends into its direction, or the moment lies after the last packet. A timeline that changes the loss, the duplication, the corruption or the reordering of a direction that no workload sends into makes the result say that the lost packets are not promised to repeat, with the reason. So does a direction whose queue draws random numbers and that two workloads send into, with or without a timeline: the order of their packets at the queue is not fixed (D-065). A timeline that changes only a rate or a delay is not affected. A `down` and an `up` are bound to a workload on the link in the same way. An `outage` is the exception: it is a window in the time of the receiver, so it is always done by the clock, and the packet at each of its edges can be lost in one run and delivered in another; the result says so with a reason ([COVERAGE.md](COVERAGE.md)).

## Workloads

```yaml
workloads:
  - id: media
    kind: udp-cbr
    from: client
    to: server
    rate: 2mbit
    packet-size: 1200b
    start: 0s
    stop: 30s
```

A workload has a `kind`. `udp-cbr` (WRK-01) is UDP at a constant bit rate, sent by the agent in `from`, received by the agent in `to`. `udp-poisson`, `udp-onoff` and `udp-bursty` (WRK-02) send UDP packets in the same way at moments that are drawn from the seed or follow a pattern, see [Traffic patterns](#traffic-patterns). `command` (WRK-06, release 0.2) is a program of your own that runs in a node, see [A program of your own](#a-program-of-your-own). The keys of one kind are refused in the other (`rate` in a `command`), each at its own line.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | A Name, unique among workloads. Metrics are named after it (`media.delay.p99`). |
| `kind` | yes | `udp-cbr`, `udp-poisson`, `udp-onoff`, `udp-bursty` or `command`. |
| `from`, `to` | yes | Two different nodes from `nodes`. |
| `rate` | yes (not for `udp-bursty`) | A Rate. **The rate of the payload**: the number of payload bits per second. The packets per second follow from it and `packet-size`. For `udp-poisson` it is the mean rate, for `udp-onoff` the rate while on. |
| `packet-size` | yes | A Size: the payload of one packet. From the size of the header of the agent (16 bytes, 20 with `udp-checksum: false`) to 1472 bytes, the largest payload that fits the MTU of 1500 without fragmentation (fragmentation arrives with release 0.3). |
| `start` | no | A Duration, default `0s`. Relative to the start of the measured phase. |
| `stop` | no | A Duration, default `duration`. Must be greater than `start` and not greater than `duration`. |
| `udp-checksum` | no | A boolean, default `true`. `false` sends the packets without the checksum of UDP, with a CRC-32 in the payload in its place, so that a packet which a link with `corrupt` damages arrives and is counted as `corrupted` instead of being dropped by the receiving kernel (D-040, D-056). Only `true` and `false` are values, and not `on` and `off`: YAML 1.1 reads those as booleans and YAML 1.2 as text, so a file would mean two things. IPv4 only, and the Kernel backend needs `CAP_NET_RAW` in the node ([DOCTOR.md](DOCTOR.md)). |

Packets per second are at most **100,000** per workload. A higher rate is an error that says so: the agent was measured up to that rate (spike S8) and the format does not promise more than has been measured.

Every workload that sends UDP produces a record per packet (sequence number, send time, receive time, PRB-01) and the metrics of PRB-02, PRB-03 and PRB-05: packets sent, received and lost, delivered ratio, one-way delay (mean, percentiles, maximum) and throughput and goodput over time, and PRB-07: the same packets by time bin, sent and arrived side by side (`series-<id>.jsonl`), and the time to recover after each event of the timeline. The metric names and the result format are defined in `docs/RESULTS.md`, which arrives with the result format (RPT-01). The assertions of release 0.1 are written in test code (ASR-01), not in the file.

### Traffic patterns

Three more kinds send UDP packets of one size, from `from` to `to`, like `udp-cbr` (`id`, `from`, `to`, `packet-size`, `start`, `stop` and `udp-checksum` mean the same) and differ in **when the packets leave**. The moments are drawn from a seeded generator whose algorithm is written down ([AGENT.md](AGENT.md)), so **the same `seed` gives the same packets at the same moments** in every run, and a step of the timeline lands before the same packet (D-071). A scenario without a `seed` gets a drawn one, as for the loss, and the result records it. Each workload draws from a sequence of its own.

```yaml
workloads:
  - { id: requests, kind: udp-poisson, from: client, to: server, rate: 800kbit, packet-size: 400b }
  - { id: sensor, kind: udp-onoff, from: client, to: server, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms, distribution: exponential }
  - { id: video, kind: udp-bursty, from: server, to: client, packet-size: 1200b, burst-packets: 10, gap: 200us, pause: 100ms }
```

| Kind | Keys of its own | What it sends |
| --- | --- | --- |
| `udp-poisson` | `rate` | A Poisson process: the gaps between packets are exponentially distributed with the mean `packet-size / rate`, so `rate` is the mean rate. The first packet leaves after the first gap. |
| `udp-onoff` | `rate`, `on`, `off`, `distribution` | An **on phase** in which packets leave at the constant `rate` (a packet at the start of the phase, then one every `packet-size / rate` while they fall inside it), then an **off phase** with none, and so on, starting with an on phase. The mean rate is `rate × on / (on + off)`. |
| `udp-bursty` | `burst-packets`, `gap`, `pause`, `distribution` | Bursts of `burst-packets` packets, `gap` apart, then a `pause` from the last packet of the burst to the first of the next. The first burst starts at the start. The mean rate is `burst-packets × packet-size × 8` over `(burst-packets - 1) × gap + pause`. |

`on`, `off` and `pause` are Durations above zero. `distribution` is `fixed` (the default: every phase lasts exactly the time given, nothing is drawn, and the pattern is one you can work out by hand) or `exponential` (the lengths are drawn, with the time given as their mean: a Markov source). `burst-packets` is from 1 to 100,000. The packets of a burst are at most 100,000 a second like everything else, so a `gap` is at least 10 us (with one packet in a burst it is not used). A Poisson source can send a few packets closer together than that now and then; they leave a little late, and `sender.max_lateness_ns` in the result says by how much.

The result has the **mean** rate of the payload in `rate_bits_per_second` for these kinds (the rate of the scenario for `udp-poisson`), and the command line says `on average`. The series ([RESULTS.md](RESULTS.md)) shows the pattern in time: a bin in an off phase is empty, a burst is a bin that holds ten packets.

## What is measured

```yaml
measure:
  series:
    bin: 500ms              # the width of a bin of the series of each workload (default 1s)
  recovery:                 # how the time to recover after an event is worked out
    basis: arrival          # send (default) or arrival
    baseline-bins: 5        # bins before the disturbance that make the baseline (default 5)
    window-bins: 3          # bins in a row that have to be back at the level (default 3)
    fraction: 90%           # the share of the baseline to reach again (default 90%)
  capture: [access]         # links whose interfaces are captured as pcap files, at both ends
  packets: true             # also write packets-<id>.jsonl, one line for each packet (default false)
```

A scenario is the whole experiment: what runs **and what is measured**. A threshold on the time to recover only means something with the definition of the time to recover next to it, and the hash of the scenario in the result tells whether two results were measured in the same way. So these keys are in the file, not in the options of a run (D-073). The options of a run are only about how this machine runs it: the directory of the result, the seed to repeat a run with, the settle time, the working directory, the form of the output.

Every key is optional, and so is the section. `--bin`, `--recovery-basis`, `--recovery-baseline`, `--recovery-window`, `--recovery-fraction`, `--capture` and `--packets` on the command line, and the arguments of the same names in Python (`bin`, `recovery_basis`, ...), **replace** the key for one run, which is how a test tries another bin without editing the file. The scenario that ran, with the replacements, is the one whose hash is in the result.

| Key | Meaning |
| --- | --- |
| `series.bin` | A Duration, at least `1ms`; the measured phase may have at most a million bins. The width of a bin of `series-<id>.jsonl` and the resolution of the time to recover ([RESULTS.md](RESULTS.md)). |
| `recovery.basis` | `send` or `arrival`: the view the level of a bin is made from. |
| `recovery.baseline-bins`, `recovery.window-bins` | Integers of at least 1. |
| `recovery.fraction` | A percentage above 0 % and at most 100 %. |
| `capture` | A list of link ids, each at most once; an id that is not a link is refused with a hint. Both interfaces of each link are captured, in the node at each end: `capture-<link>-<node>.pcap`. The tap sits after the queue of the interface, so a packet that the impairment dropped is in no file, see [RESULTS.md](RESULTS.md#captures-prb-09-d-047-d-073). Backends that cannot capture refuse the key by name. |
| `packets` | A boolean, default `false`. |

## A program of your own

```yaml
workloads:
  - id: app
    kind: command
    node: server
    run: ["./target/release/my-server", "--listen", "{server.addr}:4433"]
    env:
      RUST_LOG: info
    ready: { udp-port: 4433, within: 5s }
    shutdown: { signal: TERM, within: 3s }
```

A `command` is the only way for a scenario to run code, and it always says so in the file. It runs in the namespace of its node, as the invoking user (inside a user namespace that is uid 0 mapped to the user), in the directory of the scenario file, and **without a shell**: `run` is the program and its arguments, so there is nothing to quote and nothing that a space or a `;` could change (D-041, D-051).

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | A Name, unique among all workloads. |
| `kind` | yes | `command`. |
| `node` | yes | The node it runs in. |
| `run` | yes | A list of texts: the program, then its arguments. At least one, at most 256 entries, 4096 bytes each, no NUL character. |
| `env` | no | A mapping of variables. Names are letters, digits and `_`, not starting with a digit, and not starting with `NETSCENARIO_` (those are set by the tool: the addresses of the nodes, the directory of the run). At most 64 variables. |
| `ready` | no | How the run learns that the program can be used; before that the senders do not start. Without it the program is ready when it has been started. See below. |
| `on-exit` | no | `continue` (default) or `fail`: what the run does when the program ends by itself, before the end of the run, with an exit code other than 0 or by a signal. `continue`: the run goes on and the result reports how it ended. `fail`: the run stops with an error that names the program ([PROGRAMS.md](PROGRAMS.md)). |
| `shutdown` | no | `signal`: `TERM` (default), `INT`, `HUP`, `QUIT`, `USR1` or `USR2`; `within`: a Duration, default `5s`. The signal goes to the process group of the program, so its children get it too, and `KILL` follows when anything is still there after `within`. |

**Placeholders.** In `run` and in the values of `env`, `{node.addr}` is the address of a node (`{server.addr}`). It is checked when the file is read: the node must exist (with a hint), and any other text in braces is refused. A brace that is meant as a brace is written twice: `{{` and `}}`.

**Readiness.** Exactly one of these, and every one except `delay` needs `within`, the time after which a program that is not ready fails the run:

| Key | Ready when |
| --- | --- |
| `udp-port` | A UDP socket is bound to that port in the node (to any address or the address of the node; read from `/proc/net/udp`, S14). |
| `tcp-port` | A TCP socket listens on that port (from `/proc/net/tcp`). |
| `stdout-line` | A line on the standard output of the program contains this text. |
| `file` | This file exists, a path relative to the directory of the scenario file. |
| `delay` | After this long, whatever the program does. The one way that checks nothing, and the only one without `within`. |

A port is read from outside, so the program needs no help and a UDP port (which cannot be probed by connecting) works. Probing by connecting was left out for that reason.

**What the Kernel backend does with it:** [PROGRAMS.md](PROGRAMS.md) says what runs, in which order, as whom, with which variables (`NETSCENARIO_*`), how the output and the metrics (`{"metric": ..., "value": ...}` lines) are read, and what the result has. Running a scenario file that you did not write is running its programs: read the `command` entries first ([SECURITY.md](../SECURITY.md)).

## Keys that are reserved

These keys are not part of version 0 in release 0.2. A scenario that uses one is refused with the message "`<key>` is planned for release X (see FEATURES.md)", not with "unknown key".

| Key | Planned in | Meaning |
| --- | --- | --- |
| `allow-limited`, `expect`, `params`, `include` | 0.3 | Accepted restrictions, assertions in the file, parameters and includes. |
| `matrix` | 0.5 | Sweeps. |
| `mobility` | 0.6 | Positions over time. |
| `model` (in a link) | 0.3 | Link models: trace, Markov, periodic (the steps over time are the `timeline`, 0.2). |
| `queue` (in a direction) | 0.3 | Queue limit and discipline. |
| `address` (node, link) | later | Fixed addresses. |
| `kind` other than `udp-cbr`, `udp-poisson`, `udp-onoff`, `udp-bursty` and `command` | 0.3 and later | `tcp-bulk`, `request-response`, `trace-replay` and more. |

**`repetitions` is not a key of the scenario, and it is not planned as one**: how often a scenario is run is an option of a run (`netscenario run --repetitions n`, `run(..., repetitions=n)`; D-042, D-074), so that the file stays what is run and the hash of the scenario is the same for one run and for a set. A scenario that has the key is refused with a message that says so.

## Errors

Every error from reading a file has **the line and the column, and shows the source line** with a mark under the place. The errors that come from the contents and not from the syntax (a node that does not exist, a `stop` before the start) name in addition the **path** in the file, such as `topology.links[0].between[1]`, and often a hint on how to fix them. The text says what was found and what was expected. All problems of the contents are reported together, in the order of the file; a syntax error is reported alone, because nothing after it can be read reliably. What the tool prints:

```text
error: '40' has no unit (use ns, us, ms, s, m or h)
  --> access.yaml:10:16
   |
 9 |       forward:
10 |         delay: 40
   |                ^^

error: unknown field `dealy`, expected one of delay, jitter, loss, rate
  --> access.yaml:10:9
   |
 9 |       forward:
10 |         dealy: 40ms
   |         ^^^^^
   = help: did you mean 'delay'?

error: unknown unit 'min' in '40min' (did you mean 'm'? use ns, us, ms, s, m or h)
  --> access.yaml:10:16
   |
 9 |       forward:
10 |         delay: 40min
   |                ^^^^^

error: the node 'sever' does not exist
 --> access.yaml:8:25
  |
7 |     - id: access
8 |       between: [client, sever]
  |                         ^^^^^
  = path: topology.links[0].between[1]
  = help: did you mean 'server'? the nodes are: client, server

error: `queue` is planned for release 0.3 (see FEATURES.md)
  --> access.yaml:11:9
   |
10 |         delay: 40ms
11 |         queue: { limit: 100 }
   |         ^^^^^
   = path: topology.links[0].forward.queue
```

The messages for syntax, types, unknown and duplicate keys come from the YAML library and are shown in the words of this format; the others come from the checks of the model (D-020). The scenarios in `scenarios/invalid/` are faulty on purpose, one fault per file (and one with several), and their messages are held in snapshots, so a change in a message is a change in a test (QA-01).

A scenario that is built in code goes through the same checks ([`Scenario::validate`](../crates/netscenario-model/src/validate.rs)); its problems have the path but no line.

The JSON Schema of the format is [`docs/schema/scenario-v0.json`](schema/scenario-v0.json), generated from the types; a test fails when it is out of date and `just schema` writes it. A schema can say what a shape is (keys, types, the pattern of a value) and not what the contents mean, so the checks above are not in it.

## Part 2: planned, not binding

The following sketches show where the format is heading. They are **not** part of version 0 in release 0.1 and may change. The release in brackets is the plan of `FEATURES.md`.

The keys `corrupt`, `reorder`, `duplicate`, `udp-checksum` and the loss models of release 0.2 are in Part 1.

**Link models (0.3 to 0.6).** `model: { kind: trace, file: ..., format: mahimahi }`, Markov, periodic and geometry-based models, with mobility (0.6). A model replaces the static values of its link. Traces are files named by a path; hashes of their content come with 0.3.

**Expectations in the file (0.3).**

```yaml
expect:
  - { metric: media.delivered_ratio, ci95-low: ">= 0.97" }
  - { metric: media.delay.p99, "<": 150ms }
```

**Parameters and sweeps (0.3, 0.5).** Values such as `{rtt / 2}` with units and `+ - * /` only; the file stays data. `matrix` describes the grid and the tool runs and judges it.

**Limited features (0.3).** A backend that can do a feature only in part refuses it unless the scenario lists it in `allow-limited`; the result records every accepted restriction.

## Decisions on the questions of the design

| Question | Decision |
| --- | --- |
| How strict is address assignment? | In 0.1 the tool assigns every address; fixed plans come later (`address` is reserved). |
| One thing for `timeline` and `model: steps`? | Yes. `timeline` is the readable shorthand (0.2). |
| An `environment` section? | No. Kernel, permissions and tools are properties of the run, recorded in the result, not requirements of a scenario. |
| Names of the loss parameters? | The `netem` names, without aliases. |
| How are traces shared? | As files with a path; content hashes with 0.3. |
| Where do sweeps end and a test harness begin? | The format describes only the grid; running and judging it is the job of the tool. |

## Open

1. **Round-trip time (PRB-12).** Decided: `udp-cbr` stays one-way, and the round trip is measured by the request/response workload of release 0.3 (WRK-05), which a receiver answers. The ledger says so; the format of release 0.1 does not change.
2. **Typo hints are a guess.** (SCN-10, release 0.2.) A hint is given for a node, a key, a value of an enumeration (`kernal`) and a unit when exactly one known name is close: one edit for names up to five characters, a third of the length for longer ones, and a swap of two neighbours (`dealy`) counts as one edit. Keys: `Delay`, `dealy`; a required key that is missing while a near name is present (`packet_size`) says so. Units: the same unit in another case (`Mbit`), a name people use (`min`, `mbps`, `bytes`) if the kind of value has the unit, or one edit. When two names are equally close there is no hint. An edit distance cannot tell a slip from another word: a key `speed` would be taken for `seed` (the file `unknown-key.yaml` uses a key that is far from all). The hint is written as a question for that reason.
