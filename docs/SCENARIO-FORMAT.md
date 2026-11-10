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

- One document per file, UTF-8, at most 4 MiB (the expanded form of a scenario with a trace or a table holds their packets or entries), nesting at most 32 levels deep. An empty file is an error.
- Keys are unique inside a mapping.
- **Anchors, aliases, merge keys (`<<`) and tags are refused.** A scenario reads the same from top to bottom. Reuse is done on purpose with parameters, templates and includes ([below](#parameters-templates-and-includes)), and not through YAML features.
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
| `allow-limited` | no | A list of ids of features: the restrictions of a backend that the scenario accepts, see [Limited features](#limited-features). |
| `params` | no | The parameters of the scenario with their defaults, see [Parameters, templates and includes](#parameters-templates-and-includes). In the scenario that is read it holds the values the run was made with, as text. |
| `templates` | no | Named templates of this file, see [Parameters, templates and includes](#parameters-templates-and-includes). |
| `profile-library` | no | Written by the tool, not by hand: the version of the profile library that the `profile` of the links was resolved with (D-095). A scenario that has it is not resolved again, so a recorded scenario keeps the values of its run. |
| `topology` | yes | See below. |
| `workloads` | yes | A list with at least one workload. |
| `timeline` | no | A list of events, see [Timeline](#timeline). |
| `expect` | no | Checks on the metrics of the run or of a set of runs, see [What the scenario must show](#what-the-scenario-must-show-expect). |
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
- **Links.** A link has an `id` (a Name) and `between`: two different nodes. Two nodes may have **several** links (multipath, multi-homing). The first is the "forward" side: `forward` is the direction from the first node to the second, `reverse` the way back. Impairments are set per direction (LNK-11); `both` sets the same values in both directions and **cannot be combined with `forward` or `reverse`**. A direction that is not mentioned has no impairment. `cost` (a whole number, 1 to 1,000,000, 1 by default) is what the link costs the automatic routing. `mtu` (68 to 65,535 bytes, 1500 by default) is the MTU of the interface at both ends of the link: the kernel refuses a frame longer than the MTU of the interface that receives it, so one value holds for both directions (D-081). A packet that does not fit the MTU of the **first** link of its path is cut into fragments by the sender, and every later link carries those whole; a packet that fits the first link but not a later one would be cut by a router, which the run cannot account for, so the scenario is refused with the sizes.
- **Addresses: `ip`.** `topology: { ip: v4 | v6 | dual }`, `v4` by default (D-081). `v4` is the addresses of the earlier releases (`10.100.<n>.0/30` on the links, `10.200.0.<index>` on the loopback of a node). `v6` gives the links `fd00:100:<n>::/126` and the nodes `fd00:200::<index>` (unique-local addresses, no IPv4 anywhere); `dual` gives both. IPv6 has to be asked for because its interfaces send packets of their own when they come up, so a network with it waits a second before the first queue is built; the namespaces send no router advertisements and do no duplicate address detection, and the neighbours are fixed. A router forwards both families, a host neither. The MTU of a link in a network with IPv6 is at least 1280.
- **Routing.** `routing: auto` (the default) computes a route from every node to every node by the shortest path: the smallest sum of the `cost` of the links, passing routers only. Equally short paths: the first link in the order of the file is taken, so the result is the same on every run; `ecmp: true` installs all of them as a multipath route. `routing: manual` switches the automatic routes off, and only the listed routes exist.
- **Routes.** A route is `{ node, to, via }` with an optional `metric` and `weight`. `node` has the route; `to` is a node or `default` (every destination that has no route of its own); `via` is the **link** the packets leave by, and the node at its other end is the next hop (a link and not a neighbour, so that two links between the same nodes are told apart). The smaller `metric` is used first (100 by default), which makes a failover: when the interface of the first goes down, the second takes over. Routes with the same `metric` share the packets by their `weight` (1 by default, up to 256). With `routing: auto` a listed route replaces the automatic routes of the same node and destination and leaves all others alone; with `manual` the listed routes are all there is. A scenario is checked for a path over the routes for every workload that sends packets: a node without a route, a route through a host, and routes that lead in a circle are refused with the node where it goes wrong.
- **Addresses.** The tool assigns every address and records it in the result; they are not part of the file. Every link gets a subnet of its own and every node an address of its own on its loopback, which workloads and programs use (`{server.addr}`), so a node with several links needs no choice of address. IPv4 only in release 0.3 so far (TOP-08). A key `address` is reserved for a later release.
- **Which backends.** The Kernel backend builds networks of several nodes: every node gets its address on the loopback (`10.200.0.<n>`, the placeholders and the workloads use it), routers forward, and the routes are set in each node, each naming the address of the node as the source of what it sends (D-089, docs/COVERAGE.md). The result lists the nodes, the routes and the path of each workload (docs/RESULTS.md).

### Shortcuts

For the common shapes a topology is written as one shortcut instead of `nodes` and `links` (D-078). The shortcut is **expanded when the file is read**: what is run, hashed, replayed and written out again is the long form, and `netscenario expand` prints it. The names of what is made are fixed, so that a route or an event of the timeline can name them.

| Shortcut | Keys | What it makes |
| --- | --- | --- |
| `chain` | `nodes` (at least 2), `link`, `links` | The first and the last node are hosts, the ones between are routers. A link between each pair, named `<first>-<second>`, its forward direction from the first to the second. `link` is the values of every link, `links` is a list with one entry for each link (instead of `link`). |
| `star` | `hub`, `leaves` (at least 2), `link`, `links` | The hub is a router, the leaves are hosts. A link from each leaf to the hub, named `<leaf>-<hub>`, its forward direction towards the hub. `link` or `links` (one for each leaf). |
| `dumbbell` | `left`, `right` (at least 1 each), `access`, `bottleneck` | The routers `router-left` and `router-right`, a link `<host>-router-left` from each host on the left and `<host>-router-right` from each host on the right (forward from the host to its router, values from `access`), and the link `bottleneck` from `router-left` to `router-right` (values from `bottleneck`). |
| `multipath` | `from`, `to`, `paths` (at least 2): each `{ routers, link }` | `from` and `to` are hosts; each path is a chain of its `routers` (at least one) from `from` to `to`, its links named `<a>-<b>` and given the values of `link`. |

The values of a link (`link`, `links`, `access`, `bottleneck`) are `cost`, `mtu`, `forward`, `reverse` and `both`, with the same rules as the values of a link in the long form. `routing`, `ecmp` and `routes` may stand next to a shortcut; `nodes` and `links` may not (write the long form for that). A shortcut that makes a name twice (a host called `router-left` in a dumbbell), a name longer than 32 characters or a count that does not fit is refused, and a fault in the values of a link is shown at the place in the shortcut.

```yaml
topology:
  dumbbell:
    left: [alice, bob]
    right: [carol, dave]
    access:     { both: { delay: 2ms, rate: 100mbit } }
    bottleneck: { both: { delay: 20ms, rate: 10mbit } }
```

The shortcuts are in [scenarios/dumbbell.yaml](../scenarios/dumbbell.yaml) and [scenarios/star-and-chain.yaml](../scenarios/star-and-chain.yaml); a chain of two routers written out is in [scenarios/three-hops.yaml](../scenarios/three-hops.yaml), a failover with listed routes in [scenarios/two-paths.yaml](../scenarios/two-paths.yaml) and a computed network with one route changed by hand in [scenarios/manual-routes.yaml](../scenarios/manual-routes.yaml).

### Profiles (SCN-06, D-095)

A link can take its values from the **profile library** with `profile: <id>` (`netscenario profiles` lists the ids, [PROFILES.md](PROFILES.md) has the numbers and their sources):

```yaml
links:
  - id: access
    between: [phone, server]       # the first node is the terminal: forward is up, reverse is down
    profile: lte-good
    forward: { loss: { model: random, p: 1% } }   # written next to the profile, it wins
```

A profile sets only what a source states, so most profiles set a delay and few set more; what they do not set, the file writes. The keys written next to `profile` win over it key by key (a `loss` of the file replaces the `loss` of the profile as a whole; `both` wins in both directions). The profile is replaced by its values when the file is read, so `netscenario expand` shows them; the link keeps `profile` as a record, and the scenario gets `profile-library` with the version of the library. A profile is a key of a link, not of a direction, because up and down are only known for a link. An unknown id is an error with the closest one. A step of the timeline and a model of the link work on the values the profile gave.

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
| `jitter` | `amount`: Duration, `distribution`: `uniform` (default), `normal`, `pareto`, `paretonormal` or `table`, and `table` for the last | `netem delay D J`, with `distribution NAME` for all but the first (LNK-02, D-084). **Needs `delay`.** A delay below zero cannot exist, and `netem` would silently clamp it to zero, so the format refuses a jitter that reaches below zero: `uniform` needs `amount` not greater than `delay`, `normal` needs `delay` at least four times `amount` (the tables of `netem` are cut at about four sigma, which is also why a normal jitter never reaches its far tail, see COVERAGE), and for the other tables the delay must be at least as many amounts as the table reaches below its mean (`pareto` 0.67, `paretonormal` 1.50, `table` as the samples have it, known after the file is read). See "The distributions of the jitter" below. |
| `loss` | `model` and its parameters, see below | `netem loss` (LNK-03, LNK-04). `trace` is planned for 0.3. |
| `reorder` | `share`: Probability, `gap`: whole number, at least 1 (optional, default 1) | `netem reorder P gap G` (LNK-05). **Needs `delay`** (`tc` refuses it without). `share` is the share of the packets that are sent on at once; the others wait for the delay and arrive after later ones. With `gap` the share sent on at once is `1 / (gap - 1 + 1 / share)`: 25 % with `gap: 5` is 12.5 %. Measured in D-048. |
| `duplicate` | Probability | `netem duplicate` (LNK-06). The copy is impaired on its own. |
| `corrupt` | Probability | `netem corrupt` (LNK-07). One bit of the frame is flipped, anywhere in it, the Ethernet header included; a receiver with the UDP checksum on drops the packet (D-040), so the program sees loss, and the result tells the two apart (`directions`, [RESULTS.md](RESULTS.md)) and names most of them by sequence number (`corrupted_seq`). The share of packets that are dropped is a little less than `P`: a bit in the source address of the Ethernet header harms nothing. |
| `rate` | Rate | `netem rate` (LNK-08). |

**The distributions of the jitter (D-084).** `netem` draws the variation of the delay from a table of 4,096 entries: the delay is `delay + amount x entry`, with an entry (a number around 0, mostly within a few units) chosen at random.

| `distribution` | The shape | `amount` is |
| --- | --- | --- |
| `uniform` | Every delay between `delay - amount` and `delay + amount` is equally likely (no table). | The half of the range. |
| `normal` | A bell curve, cut at 3.5 deviations above and 4 below. | The standard deviation. |
| `pareto` | A heavy tail towards longer delays (the table `pareto` of iproute2): mean -0.03, deviation 0.82, from -0.67 to +4 amounts. | The scale of the table, not the deviation (0.82 amounts). |
| `paretonormal` | A bell curve with a heavy tail towards longer delays: mean -0.03, deviation 0.83, from -1.50 to +3.9 amounts. | The scale of the table (0.83 amounts). |
| `table` | The shape of a file of samples (below). | The standard deviation of the delays, whatever the tail. |

`pareto`, `paretonormal` and `normal` need the tables of iproute2 on the machine (`netscenario doctor` says whether `tc` finds them).

```yaml
forward:
  delay: 100ms
  jitter:
    amount: 8ms
    distribution: table
    table: { file: round-trips.txt }
```

`table` takes the shape of a **file of samples** (D-084): numbers, one to a line or several separated by spaces or commas, lines that start with `#` are comments; at least 100 and at most a million; any unit, because only the shape is used (take a measurement of the delay of a real link and write the file as it comes). The tool turns the samples into the table of `netem`: the 4,096 evenly spaced quantiles, moved to the mean 0 and scaled to the deviation 1, so `delay` is the mean of the delays and `amount` their standard deviation. A tail that reaches further than the 16 bits of a table hold (4 deviations) is not cut: the table is scaled down and the amount goes to `netem` scaled up by the same factor (the `scale` in the table of the scenario), so the whole tail is there; events rarer than 1 in 8,192 are not in the table.

**The files a scenario refers to (D-083).** The `file` is a path relative to the directory of the scenario file, and it must lie inside that directory (links and `..` are resolved first; an absolute path is refused; `--allow-outside-files` or `allow_outside_files=True` lifts this). It is read when the scenario is loaded, at most 16 MiB. What is made of it is written into the scenario: `netscenario expand file.yaml` shows

```yaml
table:
  file: round-trips.txt
  sha256: 3b1f...            # the fingerprint of the file
  samples: 1200
  scale: 2024                # thousandths; left out when the samples fit
  values: "f9c4..."          # the 4,096 entries, 4 hexadecimal digits each
```

so the canonical form, its hash and the `scenario.json` of a run hold the table, a replay needs no file, and another file is another scenario. A `sha256` written by hand is checked: the file must have that content. A `table` with `values` is complete and is not read from a file again. In a step of the timeline write `distribution: table` and the new `amount` and leave the `table` out: the table is the one of the link, and a step cannot bring another (`netem` keeps the table that was loaded first).

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


### Queue

`queue` is the queue of a direction: the packets that wait for its `rate` (LNK-09, D-082). It needs a `rate`, because without one nothing waits; the delay of the link comes after the queue, so a deep queue adds its waiting time to the `delay`, as in a router.

| Key | Values | Meaning |
| --- | --- | --- |
| `discipline` | `fifo` (default), `codel`, `fq-codel`, `pie` | How the queue drops: `fifo` drops what does not fit (tail drop), `codel` (RFC 8289) by the time a packet has waited, `fq-codel` (RFC 8290) with a queue for each flow, `pie` (RFC 8033) by a probability that follows the delay. |
| `limit` | `100p` or `64kb` | The size: packets, or data (a `fifo` only; the other disciplines count packets). Default: the one of the kernel for the discipline (1000 packets, 10240 for `fq-codel`). |
| `target` | a duration | `codel`, `fq-codel`, `pie`: the delay the queue accepts (5 ms for `codel`, 15 ms for `pie`). |
| `interval` | a duration | `codel`, `fq-codel`: the time over which a delay above the target is looked for (100 ms). |
| `t-update` | a duration | `pie`: the time between two updates of the drop probability (15 ms). |
| `ecn` | `true`, `false` | `codel`, `fq-codel`, `pie`: mark the packets of a flow that asked for it instead of dropping them. |
| `flows` | 1 to 65536 | `fq-codel`: the number of queues (1024). |
| `quantum` | a size | `fq-codel`: the bytes a queue may send in its turn (the MTU). |

```yaml
forward:
  rate: 5mbit
  delay: 20ms
  queue: { discipline: codel, limit: 200p, target: 5ms, interval: 100ms }
```

A parameter that belongs to another discipline is refused with the discipline it belongs to, and so is a limit in data on a discipline that counts packets. A step of the timeline cannot set `queue` (the queue is built with the link and kept for the whole run, D-059); the rate can be changed. A direction without a `queue` keeps the queue that the backend builds for it (the default of the kernel for the packets in flight, see "Queue of the Kernel backend" in [COVERAGE.md](COVERAGE.md)). The result lists every queue that was built, with its counters (`queues`, see [RESULTS.md](RESULTS.md)). A link with a `queue` is built through the fabric of the run (D-079); the example is [`scenarios/queues.yaml`](../scenarios/queues.yaml).

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
| `every` | Duration | The event repeats every so often, the first time at `at` (TML-03). Until `until`, or `count` times, or with neither until the end of the measured phase. |
| `until` | Duration | The moment before which the event repeats. Needs `every`; not with `count`. |
| `count` | whole number | How many times the event happens in all, the first included. Needs `every`; not with `until`. |

**An event that repeats** is its copies, one for each moment, and each copy is checked as an event of its own: `outage: 4s, every: 3s` is refused because the outages overlap, and an event that would repeat more than 10,000 times is refused with the bound. `netscenario expand --seed n` writes the copies.

**`set` is a merge patch** (the rule of RFC 7386): a key that is not written stays as it is, a value replaces the old one as a whole (a `loss` replaces the loss, it does not merge into it), and `null` takes the impairment away. `both` changes the two directions. After every step the direction must be a valid set of impairments, and the reader checks it: `delay: null` while a `jitter` is set is refused, and the message says that it is this step.

**What the reader refuses besides the types:** an event that does none or several of the four things; a link that does not exist (with a hint); a moment at or after the end; an outage that is not longer than zero, that ends after the end of the measured phase, or that overlaps another outage of the same direction (the end of the first would be unclear); a link that goes down twice or comes up while it is up; a `direction` on anything but an outage; a step that writes no key.

**What the Kernel backend cannot follow.** The jitter of a direction keeps one distribution for the whole run: `netem` cannot go back from one table (or from none) to another without building the queue again (D-059). The amount can change.

**When it happens.** An event is applied **before the first packet that its workload sends at or after its moment** (D-038, D-062): the sender of the workload that sends into the direction of the step knows its packet clock, asks the run right after the packet before, and holds the packet at its slot until the run has done the step. For that workload this is the same as a step at the exact moment, whatever the machine does, so the same seed gives the same lost packets; the packet that waits leaves late by the time of the change (`max_lateness_ns` shows it) and the ones after it catch up in order. The result records, for each step, the number of the first packet that met it and how long that packet waited (`events`, [RESULTS.md](RESULTS.md)). A step that no workload can wait for is done by the clock at its moment: no workload sends into its direction, or the moment lies after the last packet. A timeline that changes the loss, the duplication, the corruption or the reordering of a direction that no workload sends into makes the result say that the lost packets are not promised to repeat, with the reason. So does a direction whose queue draws random numbers and that two workloads send into, with or without a timeline: the order of their packets at the queue is not fixed (D-065). A timeline that changes only a rate or a delay is not affected. A `down` and an `up` are bound to a workload on the link in the same way. An `outage` is the exception: it is a window in the time of the receiver, so it is always done by the clock, and the packet at each of its edges can be lost in one run and delivered in another; the result says so with a reason ([COVERAGE.md](COVERAGE.md)).

## Models over time

```yaml
links:
  - id: hop
    between: [terminal, gateway]
    both: { delay: 35ms, rate: 20mbit }          # what the link is outside the states
    model:
      kind: periodic
      states:
        - { id: in-view, for: 40s }                # a state that writes nothing is the link as it is written
        - { id: handover, for: 6s, outage: true }
        - { id: weak, for: 14s, set: { both: { rate: 4mbit, loss: { model: random, p: 1% } } } }
```

A **model** (LMD-04, LMD-05, D-090) makes a link change over time without a `timeline` of the file. When a run begins the model is replaced by the events it makes (`set`, `outage`, `down`, `up`), drawn from the seed of the run, and everything after that is a run with a timeline: the steps are done before the first packet that a workload sends at or after their moment (D-038), the same seed gives the same lost packets, and `events` of the result lists the steps with `from_model: true`. `netscenario expand file --seed n` writes the events that a seed makes. A backend that can follow the steps of a timeline follows a model, and is asked for the features of the states (`rate-limit`, `outages` and so on) with the path of the state.

A **state** is a complete overlay on the link as the file writes it: `set` names the keys that the link has while the state lasts (a merge patch on `forward`, `reverse` or `both`, as in a step of the timeline), `outage: true` loses everything that leaves the link while it lasts (D-064, `direction` limits it), and `down: true` takes the interface down for the stay (D-039). Every other key is what the link says. So a state means the same whatever came before it, and the step into it sets the keys that differ from the state before. Each state is checked once when the file is read, as a link with its overlay: a `delay: null` while the link has a `jitter` is refused, and the message says which state.

| Key of the model | Kind | Meaning |
| --- | --- | --- |
| `kind` | both | `periodic` or `markov`. Required. |
| `states` | both | The states. A Markov chain needs at least two. Required. |
| `begin` | both | When the model begins, from the start of the measured phase; before that the link is what the file says. Default `0s`. |
| `repeat` | periodic | `true` (the default): the list begins again when it is over. `false`: the link is what the file says after the last state. |
| `start` | markov | The state the chain begins in. Required; it is written and not drawn. |

| Key of a state | Kind | Meaning |
| --- | --- | --- |
| `id` | both | The name of the state. Required in a Markov chain. |
| `for` | periodic | How long the state lasts. Required. |
| `stay` | markov | How long the state lasts, or the mean. Required. |
| `distribution` | markov | `exponential` (the default: a Markov chain) or `fixed`. |
| `to` | markov | The chances of the next state, `{ name: 30%, other: 70% }`. They add up to 100 %; none leads to the state itself. Required. |
| `set`, `outage`, `direction`, `down` | both | What the link does in the state. |

**Periodic** is exact: no number is drawn, the moments are sums of the lengths. **Markov** is a chain whose stays are drawn (a semi-Markov chain when they are fixed): the share of time in a state is `pi_i * m_i / sum(pi_j * m_j)`, with `pi` the stationary distribution of the rows `to` and `m` the mean stays. The numbers come from a generator of their own for each link, from the seed and the **name** of the link, so a model on another link, or a link before it, does not change the states of this one. A model that does not name a seed makes the run draw one, and the result says so.

**What the reader refuses besides the types:** the keys of one kind in the other (`stay` in a periodic model), a state of no length, a chain with chances that do not add up, lead to the state itself or to a state that does not exist (with a hint), a start that does not exist, a state that is an `outage` and `down`, a `jitter` or `reorder` that the state leaves without a `delay`, and a model that would make more than 10,000 events (50,000 in all models and repeating events together; a Markov model is checked when the run begins, because its stays are drawn). **On a link with a model** the `timeline` of the file may not `set` a key that a state writes (a delay, a jitter and a reorder count as one), nor `outage` where the states have outages, nor `down` and `up` where they have `down`: the model would take it back at its next step, and the file would not say what happens. A model goes with a link that is written out in `links`; the links that a shortcut makes take their changes from the `timeline`.

Examples: [leo-pass.yaml](../scenarios/leo-pass.yaml) (a periodic pattern with a handover), [signal-states.yaml](../scenarios/signal-states.yaml) (a chain of three signal states), [flapping-link.yaml](../scenarios/flapping-link.yaml) (events that repeat).

### Traces

```yaml
links:
  - id: cell
    between: [phone, server]
    both: { delay: 30ms }                     # what the link is outside the trace
    model:
      kind: trace
      forward: { file: uplink.csv }           # time,rate,delay,loss
      reverse: { file: downlink.mahimahi, format: mahimahi, window: 100ms }
```

A **trace** (LMD-03, D-091) is a model whose rows come from a file: the rate, the delay and the loss of a link over time, recorded on a real network or made by hand. It is read like any model (D-090): when a run begins the rows become the `set` events of the timeline, applied before the first packet that a workload sends at or after their moment, and `events` of the result lists them with `from_model: true`. The files follow D-083: a path relative to the directory of the scenario file, inside it, at most 16 MiB, read once when the scenario is loaded; what is made of them and the SHA-256 of the file are written into the scenario, so a result passed on and a replay need no file.

| Key of the model | Meaning |
| --- | --- |
| `kind` | `trace`. |
| `forward`, `reverse`, `both` | The trace of that direction. `both` is the same trace for both and is not written together with the others. At least one. Each direction has its own trace and its own length. |
| `begin` | When the trace begins, from the start of the measured phase. Before that the link is what the file says. Default `0s`. |
| `repeat` | `true` (the default): the trace begins again when it is over. `false`: the link is what the file says after it. |

| Key of a direction | Meaning |
| --- | --- |
| `file` | The file. Required. |
| `format` | `csv` or `mahimahi`. A name that ends in `.csv` is a CSV file; a Mahimahi trace has no usual extension, so it is written. |
| `window` | Mahimahi only: how much of the trace makes one rate. A whole number of milliseconds, default `100ms`. |
| `period` | CSV only: how long one pass of the trace is. Default: the last row lasts as long as the row before it. |
| `sha256` | The SHA-256 of the file. The loader writes it; if you write it, the file must have exactly that content. |

**CSV** is a header and a row for each change, every value with its unit; lines that start with `#` and empty lines are skipped:

```
time,rate,delay,loss
0s,18mbit,38ms,0%
5s,14mbit,40ms,0%
10s,9mbit,46ms,0.1%
```

The header names `time` and at least one of `rate`, `delay`, `loss`, in any order. From the time of a row until the next, the link has the values of the row; a column that the file does not have is not written by the trace, and the link keeps what the scenario says for it. The times rise. A `rate` of `0` is a row in which nothing is delivered, see below. At most 10,000 rows. Example: [cell-drive.yaml](../scenarios/cell-drive.yaml).

**Mahimahi** is the format of the trace-driven link of that tool: a line for each opportunity to deliver a packet, the time in whole milliseconds, not falling; the trace lasts until its last line, and the opportunities in each `window` give the rate of that window: their number times 1,514 bytes (an IP packet of 1,500 bytes and the Ethernet header, which is what the shapers count) over the width of the window. A window of 100 ms with 100 opportunities is 12.112 mbit. A trace of 100 minutes needs a window of at least 600 ms to stay within 10,000 rows; the message says the window that fits. The delivery is by the byte and not by the packet. Example: [mahimahi-walk.yaml](../scenarios/mahimahi-walk.yaml).

**A trace that changes the rate faster than the backend follows** is a restriction, see [Limited features](#limited-features): the Kernel backend follows a rate to within 0.4 % at steps of 100 ms, within 2 % at 20 ms, and not below that. So a trace with steps of 100 ms and more needs nothing, one with steps closer than 100 ms and not closer than 20 ms is run when the scenario lists `fast-rate-steps` in `allow-limited`, and one with steps closer than 20 ms is refused (a Mahimahi trace takes a wider `window`). A row without a delivery (a Mahimahi window without an opportunity, or a CSV `rate` of 0) needs `stalled-link`: a shaper cannot stop, so the row is the rate at which one frame is delivered in it.

**What the reader refuses besides the types:** a trace without a direction, `both` together with `forward` or `reverse`, `states`, `start` or `stay` in a trace, `window` in a CSV and `period` in a Mahimahi trace, a name whose format is not known, a file that is no trace (the message has the line), rows that do not rise, more than 10,000 rows, a SHA-256 that does not match, and a trace that would make more events than a link may have (10,000, D-090). On a link with a trace the `timeline` of the file may not `set` a key that the trace writes. A `queue` may wait for a rate that a trace (or a state, or a step) brings.

### Limited features

```yaml
allow-limited: [fast-rate-steps, stalled-link]
```

A backend that reproduces a feature only in part refuses it, with the line to write, unless the scenario lists its id in `allow-limited` (D-092). The result then has `accepted_restrictions`, the id and the restriction as the backend states it with the numbers that were measured, and the summary and the reports print them. The list is checked against the scenario as the file writes it, so a model is judged by its states and rows (a Markov chain by its mean stays) and the seed cannot decide whether a run is allowed. An id that names no feature is an error with a suggestion; a feature that the backend cannot reproduce at all is refused whatever the list says. The ids are the `id` of the features in the output of `netscenario doctor` and the rows of [COVERAGE.md](COVERAGE.md): `fast-rate-steps` (the rate changes at intervals closer than 100 ms), `stalled-link` (a trace row in which nothing is delivered).

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

A workload has a `kind`. `udp-cbr` (WRK-01) is UDP at a constant bit rate, sent by the agent in `from`, received by the agent in `to`. `udp-poisson`, `udp-onoff` and `udp-bursty` (WRK-02) send UDP packets in the same way at moments that are drawn from the seed or follow a pattern, see [Traffic patterns](#traffic-patterns). `trace-replay` (WRK-03) sends the packets of a recorded trace at its moments and with its sizes, see [Replaying a trace](#replaying-a-trace). `command` (WRK-06, release 0.2) is a program of your own that runs in a node, see [A program of your own](#a-program-of-your-own). The keys of one kind are refused in the other (`rate` in a `command`), each at its own line.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | A Name, unique among workloads. Metrics are named after it (`media.delay.p99`). |
| `kind` | yes | `udp-cbr`, `udp-poisson`, `udp-onoff`, `udp-bursty`, `trace-replay` or `command`. |
| `from`, `to` | yes | Two different nodes from `nodes`. |
| `rate` | yes (not for `udp-bursty`) | A Rate. **The rate of the payload**: the number of payload bits per second. The packets per second follow from it and `packet-size`. For `udp-poisson` it is the mean rate, for `udp-onoff` the rate while on. |
| `packet-size` | yes | A Size: the payload of one packet. From the size of the header of the agent (16 bytes, 20 with `udp-checksum: false`) to 1472 bytes, the largest payload that fits the MTU of 1500 without fragmentation. If a link on the path says its `mtu`, a packet up to 65,507 bytes is allowed, and one that does not fit the MTU of the first link is cut into fragments (D-081); `frames_per_packet` in the result says how many. |
| `start` | no | A Duration, default `0s`. Relative to the start of the measured phase. |
| `stop` | no | A Duration, default `duration`. Must be greater than `start` and not greater than `duration`. |
| `udp-checksum` | no | A boolean, default `true`. `false` sends the packets without the checksum of UDP, with a CRC-32 in the payload in its place, so that a packet which a link with `corrupt` damages arrives and is counted as `corrupted` instead of being dropped by the receiving kernel (D-040, D-056). Only `true` and `false` are values, and not `on` and `off`: YAML 1.1 reads those as booleans and YAML 1.2 as text, so a file would mean two things. IPv4 only, and the Kernel backend needs `CAP_NET_RAW` in the node ([DOCTOR.md](DOCTOR.md)). |
| `ip` | no | `v4` or `v6`: the family of the addresses of the flow (D-081). Default: the only family of the network, `v4` in a `dual` one. A family that the network does not have is refused. `v6` cannot be combined with `udp-checksum: false` (IPv6 requires the checksum). |
| `dont-fragment` | no | A boolean, default `false`. `true` makes the sender refuse a packet that does not fit the MTU of the link instead of cutting it into fragments (`IP_MTU_DISCOVER` set to `DO`, and the bit "do not fragment" on the packets that fit): the run counts the refused packets as `send_errors` with the reason "Message too long", and they are not lost. Next to a packet that fits the first link, a later link that is smaller is refused as for a packet that may be cut. |
| `echo` | no | A boolean, default `false`. `true` has the far node send every packet it recorded back, unchanged, through the reverse direction, and a receiver in the sending node collect them: the result has `echo` with the packets that came back and their **round trip** (PRB-12, D-087). The delay, the jitter and the loss of the reverse direction count. Not with `udp-checksum: false`, because the echo is an ordinary datagram. A run with an echo is not accounted for packet by packet (`directions` and the check of the queues are left out). |

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

## Replaying a trace

`trace-replay` (WRK-03, D-085) sends the packets of a recorded flow: packet `n` leaves at the `n`th moment of the trace, from the start of the workload, and carries the `n`th payload. A call, a game, a video stream: the shape of real traffic, which no rate describes. It takes the keys `id`, `from`, `to`, `start`, `stop`, `udp-checksum`, `ip` and `dont-fragment` like the other senders, and `trace` instead of `rate` and `packet-size`.

```yaml
workloads:
  - { id: call, kind: trace-replay, from: client, to: server, trace: { file: call.csv } }
```

| Key of `trace` | Meaning |
| --- | --- |
| `file` | The file, a path relative to the directory of the scenario file, read when the scenario is loaded (D-083: only inside that directory, at most 16 MiB). |
| `format` | `csv` or `pcap`. Left out it is taken from the name: `.csv`, `.txt` are CSV; `.pcap`, `.pcapng`, `.cap` are captures. |
| `select` | For a capture with more than one flow: `src`, `dst`, `src-port`, `dst-port`, `protocol` (`udp` or `tcp`). The flow is the packets that go from one endpoint to the other. Not for CSV. |
| `sha256` | The fingerprint of the file. Written by the loader; if it is written, the file must have that content. |

**CSV.** One packet on each line: the time since the start of the workload and the payload of the UDP packet, each with its unit (a number without one is refused, like everywhere in the format), separated by a comma: `12.5ms,1200b`. A first line `time,size`, lines that start with `#` and empty lines are skipped. The times must not fall (packets at the same moment are sent one after the other), a size is from 16 bytes (20 for `udp-checksum: false`), the packet carries the header of the agent, and a smaller one is refused with its line. At most 100,000 packets.

**Capture.** A `pcap` (microseconds or nanoseconds, any byte order) or `pcapng` file of Ethernet (with VLAN tags), raw IP, Linux cooked or BSD loopback. The trace is the packets of **one flow**: the UDP or TCP packets from one source to one destination, with the **transport payload** as the size (a TCP segment without data has none) and the time of the capture, since the first packet of the flow, as the moment. A capture with one flow needs no `select`; with several it is refused with the list of the flows (the packets of each) and the keys that pick one. Packets that are not UDP or TCP over IP, and fragments after the first, are no flow. A packet below the smallest payload (an acknowledgement has no data) is **raised** to it, and the result counts how many (`padded`): the trace is sent as UDP, and the packet has to carry the header of the agent. A capture of the run of netscenario itself (`measure: { capture: ... }`) is a trace of the next one.

The packets are read when the scenario is loaded and kept in the scenario (D-083), as the table of a jitter is: `netscenario expand` shows the trace as `packets`, the packed moments and sizes, with the `sha256` of the file and `padded`, so a recorded scenario replays without the file and another file is another scenario. The window works as for any sender: with `stop` the packets from that moment on are not sent, and the moments count from `start`.

The packets of a trace differ in size, so the result counts **bytes**: `received_bytes` is the payload that arrived, the throughput of a bin the bytes of the packets that arrived in it, `bits` in the series the payload sent and arrived in each bin. `packet_size_bytes` is the largest payload and `rate_bits_per_second` the mean over the span of the trace. A packet larger than the MTU of the first link is cut like any other (D-081), and the frames of a trace are counted for each packet. The largest packet must fit the path as for any sender (`mtu`), and a trace is refused with the place of its largest packet.

## What the scenario must show: `expect`

`expect` (ASR-07, [D-094](decisions/D-094.md)) is a list of named checks on the metrics of a run or of a set of runs. The core judges them when the run is over, `netscenario run` prints each one with its value and its margin and exits with 1 when one does not hold, and the verdict is part of `result.json` or `runset.json`, so the Markdown, JUnit and HTML reports and the pytest plugin read the same judgment.

```yaml
expect:
  checks:
    - name: the stream arrives at its rate
      metric: media.throughput.mean
      value: ">= 1.9mbit"
    - { metric: media.delay.p99, value: "< {rtt / 2 + 10ms}" }
```

| Key | Meaning |
| --- | --- |
| `expect.repetitions` | Optional, 5 to 1,000. Without it the file is judged by **one run** and its checks use `value`. With it the file is judged by a **set of runs**: `netscenario run` makes that many runs (`--repetitions` may ask for more and never for fewer) and the checks use the statistics of a set. A file that mixes the two is refused when it is read. |
| `expect.checks` | At least one check, at most 200. |
| `name` | Optional. What the check is called in the reports. Without it: the metric, the statistic and the bound. With several bounds, the name gets the statistic added. The names of the bounds of a file are different. |
| `metric` | A name of [STATISTICS.md](STATISTICS.md#the-metrics): `<workload>.<what>`. It has to exist for the scenario: a typo is refused with a hint, an `echo` metric needs `echo: true`, a TCP metric a TCP workload, and the metrics of a program of your own are `<program>.<name>.last`, `.min` or `.max`. |
| `value` | A run: the value of the metric. |
| `mean`, `median`, `min`, `max` | A set: the statistic of the values of the runs. |
| `ci95-low`, `ci95-high` | A set: the ends of the 95 % interval around the mean ([STATISTICS.md](STATISTICS.md)). A set that was asked for another statistic or level is refused when a check uses them. |

A **bound** is a string: an operator (`<`, `<=`, `>`, `>=`) and a value in the unit of the metric: a duration for a delay (`"< 150ms"`), a rate for a throughput (`">= 8mbit"`), a percentage or a number from 0 to 1 for a ratio (`"< 1%"`), a plain number for a count (`"> 0"`). A value of the wrong kind is refused with what the metric is. A bound may use parameters and expressions ([above](#parameters-templates-and-includes)). A check can have several bounds.

- **A claim that cannot be checked fails.** A check on a metric that the run does not have (no packet arrived, so there is no delay) fails with the reason; in a set the same holds for a metric that only some runs have. A stream that lost every packet does not pass `delay.p99 < 150ms`.
- **A run that was interrupted is not judged**, and a set that was interrupted has no verdict (it has no set).
- **The margin** is how far the value is on the good side of the bound, negative by how much it is missed.
- Compare the end of the interval that is on the dangerous side with a threshold that has room ([STATISTICS.md](STATISTICS.md#what-stable-means)).

Examples: [`scenarios/expect.yaml`](../scenarios/expect.yaml) (one run) and [`scenarios/expect-set.yaml`](../scenarios/expect-set.yaml) (a set).

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
| `recovery.basis` | `send` or `arrival`: the view of the packets the level of a bin is made from. It does not apply to a TCP workload, whose level is its goodput (D-088). |
| `recovery.baseline-bins`, `recovery.window-bins` | Integers of at least 1. |
| `recovery.fraction` | A percentage above 0 % and at most 100 %. |
| `capture` | A list of link ids, each at most once; an id that is not a link is refused with a hint. Both interfaces of each link are captured, in the node at each end: `capture-<link>-<node>.pcap`. The tap sits after the queue of the interface, so a packet that the impairment dropped is in no file, see [RESULTS.md](RESULTS.md#captures-prb-09-d-047-d-073). Backends that cannot capture refuse the key by name. |
| `packets` | A boolean, default `false`. |

## TCP: data and requests (WRK-04, WRK-05, D-086)

```yaml
workloads:
  - id: download
    kind: tcp-bulk
    from: server
    to: client
    bytes: 5mb          # each connection sends this much and ends; without it, from start to stop
    connections: 2      # default 1, at most 1024
    cc: reno            # the congestion control, default cubic
  - id: api
    kind: request-response
    from: client
    to: server
    request: 200b
    response: 4kb
    connections: 4
    interval: 20ms      # an open loop: a request is due every 20 ms; without it each connection waits for its response
    timeout: 5s         # a request that waits longer failed; default 30s
```

Both use the **TCP of the machine's kernel**, with ordinary sockets, so what they measure is that kernel's TCP over the network of the scenario (the result names the kernel). They are driven by feedback and are not packet-synchronous, so a run with one is **not exact** the way a seeded UDP run is: the result says `statistical`, and a claim about them is made over repetitions. On the Kernel backend the network is built so that the segments of the stack are no larger than a frame (D-086), otherwise `netem` would handle super-packets as one packet.

| Key | `tcp-bulk` | `request-response` | Meaning |
| --- | --- | --- | --- |
| `id`, `from`, `to` | yes | yes | As for the other workloads. `from` opens the connections and sends the data (or the requests); `to` listens. Two different nodes, with a route between them. |
| `bytes` | no | not a key | A Size: what each connection sends before it ends. A transfer is complete when the far end has read the last byte. |
| `request`, `response` | not keys | yes | Sizes from 1 byte to 1 GiB. |
| `connections` | no | no | From 1 to 1024. Default 1. All of them start at once; the listener of the agent holds up to 4096 waiting connections (the kernel keeps the smaller of that and `net.core.somaxconn`). |
| `interval` | not a key | no | A Duration of at least 1 ms: the time between two requests of the whole workload. The latency of a request counts from the moment it was due. Without it the loop is closed. |
| `timeout` | not a key | no | A Duration above 0. Default 30 s. |
| `cc` | no | no | The congestion control of the sockets: `cubic` (the default whatever the host's default is, so that a file means one thing on two machines), `reno`, `bbr` or another that the machine has. A machine that lacks it refuses the scenario. |
| `start`, `stop` | no | no | As for the other workloads. A workload without `bytes` sends until `stop`; the data in flight at `stop` is still delivered. |
| `ip` | no | no | `v4` or `v6`, as for the other workloads. |

**What the Kernel backend does with it:** it starts a server in `to` and the clients in `from` (the agents of [AGENT.md](AGENT.md)), waits for the transfers to be done, and puts a `tcp` entry for each workload in the result ([RESULTS.md](RESULTS.md#tcp-wrk-04-wrk-05-prb-12-d-086)). The packets of a TCP workload and its acknowledgements enter the same directions as everything else, so a scenario with one is not accounted for packet by packet (`directions` and the check of the queues are left out); the counters of the queues (`queues`) are still read.

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

## Parameters, templates and includes

A file that is used again and again with other numbers, or that has the same link twice, does not have to be written out (SCN-05, D-093). Three things, all done when the file is read, before anything is checked; the scenario that results is a plain scenario, and it is the one that is hashed, recorded and replayed.

### Parameters

```yaml
params:
  rtt: 40ms          # a default
  rate: 10mbit
  share: 0.8
  flows:             # no default: it has to be given (--param flows=4)
topology:
  links:
    - id: access
      both: { delay: "{rtt / 2}", rate: "{rate}" }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: "{rate * share}", packet-size: 1200b }
```

- **Declared once**, in `params:` at the top: a name (lower case letters, digits and `_`, starting with a letter) and its default, or no value for a parameter that must be given. A default may use the parameters above it (`half: "{rtt / 2}"`).
- **Given** on the command line (`--param rtt=100ms`, as often as there are parameters), in Python (`params={"rtt": "100ms"}` in `Scenario.load`, `Scenario.from_yaml`, `Scenario.with_params` and `run`), or by a template (see below). A name that is not declared is refused with the declared ones; a value has to be of the kind of the default (a duration for a duration, a number for a number, `true` or `false`; a default that is text takes anything). `netscenario show` and `expand` take `--param` too, and print the result.
- **The result records them.** The scenario that is read has `params:` with the values as text, `result.json` has `scenario.params`, and they are part of the hash: a run with `rtt=100ms` is another scenario than one with `rtt=40ms`, also where a value changes nothing.

### Expressions

A scalar that is `{...}` as a whole (in quotes, because YAML reads an unquoted `{` as a mapping) is replaced by the value of the expression; the reader says so when the quotes are missing. A `{...}` inside a longer text is replaced by the text of its value (`"link-{n}"`, `"--rate={rate}"`).

- **What an expression has:** numbers, quantities with the units of the format (`20ms`, `10mbit`, `1500b`, `2%`), parameter names, `+ - * /`, a minus in front, and parentheses. No functions, no text, no comparisons, no loops: a file never computes more than a conversion of units does.
- **Exact.** The arithmetic is done with fractions of 128-bit integers; `0.1 + 0.2` is `0.3`. The result of a quantity is written in the largest unit that holds it exactly (`40ms * 3` is `120ms`, `10mbit / 4` is `2500kbit`), and one that is not a whole number of the smallest unit (`40ms / 3` is not a whole number of nanoseconds) is an error that says so, as the same value written out would be. A number without a unit is written with at most 12 digits after the point.
- **Kinds.** `+` and `-` take two of the same kind; `*` takes a quantity and a number; `/` takes a quantity and a number, or two quantities of the same kind (the result is a number). A duration times a rate has no unit in the format and is refused. A parameter that is text or `true`/`false` can be put in as it is and is refused in arithmetic. `{rate}` alone is the parameter as it was written.
- **What is not an expression:** `{server.addr}` (a name, a dot, a word: the address of a node in the `run` of a program, see [A program of your own](#a-program-of-your-own)), `{{...}}` (a brace written twice), and anything between braces with characters that an expression has not (`{"a": 1}`) stay as they are. A parameter can be a whole argument of a command (`"{port}"`) or be in one (`"{server.addr}:{port}"`).

### Templates and includes

A template is `params:` and `content:`. A place of use is replaced by the content, with the parameters bound to the values of its `with:`:

```yaml
templates:
  access:
    params: { id: , node: , rate: 10mbit, delay: 5ms }
    content:
      id: "{id}"
      between: ["{node}", hub]
      both: { delay: "{delay}", rate: "{rate}" }
topology:
  links:
    - use: access                      # a template of this file
      with: { id: near-access, node: near, delay: "{rtt / 8}" }
    - include: parts/access.yaml       # a file (the same with `params:` and `content:` at its root)
      with: { id: far-access, node: far }
```

- **A place of use** is a mapping with `use: name` (a template under `templates:` of the same file) or `include: path` (a file), and `with:`, and nothing else. It stands where a value, an item of a list, or a whole section can stand. If the content is a list and the place is an item of a list, the items are spliced in (a file of workloads). The arguments of `with:` are expressions in the scope of the place of use; each is a single value.
- **A template sees its own parameters and nothing else**, so it reads the same wherever it is used. A parameter without a default must be given; one that is not declared, or of another kind than the default, is refused.
- **An included file** has `params:` and `content:` at its root, and may have `templates:` for its own helpers (they are not seen from outside). It is not a scenario: no `version:`. Its path is relative to the file that includes it, below the directory of the scenario (symbolic links resolved) unless `--allow-outside-files` (D-083); it is read once, at most 4 MiB. Templates and includes may be used inside each other up to 8 levels deep; one that uses itself, directly or through others, is refused with the chain.
- **Limits:** at most 10,000 places of use and 1,000,000 values in the expanded scenario, and the expanded text may not be longer than a file (4 MiB).
- **Not here:** a list of values for a parameter, loops and conditions (a sweep is the matrix of release 0.5), a file that changes keys of another (a base scenario), and `expect` (see [What the scenario must show](#what-the-scenario-must-show-expect)).

### Errors

An error is shown where the value was written, and then each place of use that led to it, from the inside out:

```
error: cannot add a duration and a rate
  --> parts/access.yaml:10:18
   |
 9 |   between: ["{from}", "{to}"]
10 |   both: { delay: "{delay + rate}", rate: "{rate}" }
   |                  ^^^^^^^^^^^^^^^^
   = used from scenario.yaml:7:7 (file 'parts/access.yaml')
   = help: both sides of + and - need the same kind of value
```

A refusal of the backend (a feature the Kernel backend cannot reproduce) is shown the same way on the command line, at the line of the template. In Python, `ScenarioProblem.file` and `.chain` carry the same for the problems of reading the scenario; the refusal of a run is shown at the nearest line of the text the scenario was read from. The JSON Schema (`docs/schema/scenario-v0.json`) describes the file as it is written: where a value goes it also accepts a string with an expression, and where a mapping or a list goes, a place of use; so an editor does not mark such a file. The reader, not the schema, checks the scenario that results.

## Keys that are reserved

These keys are not part of version 0 in release 0.2. A scenario that uses one is refused with the message "`<key>` is planned for release X (see FEATURES.md)", not with "unknown key".

| Key | Planned in | Meaning |
| --- | --- | --- |
| `matrix` | 0.5 | Sweeps. |
| `mobility` | 0.6 | Positions over time. |
| `address` (node, link) | later | Fixed addresses. |
| `kind` other than `udp-cbr`, `udp-poisson`, `udp-onoff`, `udp-bursty`, `trace-replay`, `tcp-bulk`, `request-response` and `command` | 0.5 and later | `container` and more. |

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

error: a `queue` holds the packets that wait for the rate, and this direction has no `rate`
  --> access.yaml:10:16
   |
 9 |       forward:
10 |         queue: { limit: 100p }
   |                ^
   = path: topology.links[0].forward.queue
   = help: add a `rate` next to `queue`; without one nothing waits and the queue stays empty
```

The messages for syntax, types, unknown and duplicate keys come from the YAML library and are shown in the words of this format; the others come from the checks of the model (D-020). The scenarios in `scenarios/invalid/` are faulty on purpose, one fault per file (and one with several), and their messages are held in snapshots, so a change in a message is a change in a test (QA-01).

A scenario that is built in code goes through the same checks ([`Scenario::validate`](../crates/netscenario-model/src/validate.rs)); its problems have the path but no line.

The JSON Schema of the format is [`docs/schema/scenario-v0.json`](schema/scenario-v0.json), generated from the types; a test fails when it is out of date and `just schema` writes it. A schema can say what a shape is (keys, types, the pattern of a value) and not what the contents mean, so the checks above are not in it.

## Part 2: planned, not binding

The following sketches show where the format is heading. They are **not** part of version 0 in release 0.1 and may change. The release in brackets is the plan of `FEATURES.md`.

The keys `corrupt`, `reorder`, `duplicate`, `udp-checksum` and the loss models of release 0.2 are in Part 1.

**Link models (0.6).** Geometry-based models, with mobility (0.6). (Periodic, Markov and trace models are in Part 1.) A model replaces the static values of its link.

**Parameters and sweeps (0.3, 0.5).** Values such as `{rtt / 2}` with units and `+ - * /` only; the file stays data. `matrix` describes the grid and the tool runs and judges it.

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
