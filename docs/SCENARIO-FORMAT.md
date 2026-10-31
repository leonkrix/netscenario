# Scenario format, version 0 (release 0.1)

**Status: the contract for release 0.1.** Part 1 describes exactly what a scenario file may contain in release 0.1. Anything not in Part 1 is not part of the format and is refused with an error that says in which release it is planned. Part 2 sketches what comes later and is not binding. A JSON Schema is generated from the Rust types (`netscenario schema`, SCN-01, committed as `docs/schema/scenario-v0.json`); that schema and the tests are the final authority, and this page is checked against them.

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
topology: { ... }       # required: the nodes and the link
workloads: [ ... ]      # required: what sends traffic, at least one
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
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward:  { ... }
      reverse:  { ... }
```

- `nodes` is a list of exactly two Names in release 0.1 (point to point, TOP-01). The longer form `{ id: router, role: router }` and more nodes arrive with release 0.3.
- `links` is a list of exactly one link in release 0.1.
- A link has an `id` (Name) and `between`: two different nodes from `nodes`. The first is the "forward" side: `forward` is the direction from the first node to the second, `reverse` the way back.
- Impairments are set per direction (LNK-11). `both` sets the same values in both directions; **`both` cannot be combined with `forward` or `reverse`** (use one form). A direction that is not mentioned has no impairment.
- The addresses are assigned by the tool from `10.0.0.0/8` (IPv4 only, TOP-08) and recorded in the result. They are not part of the file. A key `address` is reserved for a later release.

### Impairments of a direction

The stages of a direction act in this order, whatever order the keys have in the file: **`loss`** (a packet may be dropped when it enters the direction and then uses no capacity), **`rate`** (the packets that are kept are sent one after the other at the rate, in a queue, in the order they came), **`delay` with `jitter`** (each packet then travels for its own delay, drawn independently). So a jitter does not slow the stream down, and it can make a packet overtake the one before it when the jitter is larger than the gap between them: the result counts such packets (`reordered`, [RESULTS.md](RESULTS.md)). A backend that cannot build exactly this refuses the scenario; the Kernel backend builds it ([COVERAGE.md](COVERAGE.md), [FIDELITY.md](FIDELITY.md)).

```yaml
forward:
  delay: 40ms
  jitter: { amount: 5ms, distribution: normal }
  loss: { model: random, p: 1% }
  rate: 10mbit
```

| Key | Value | Meaning in the Kernel backend |
| --- | --- | --- |
| `delay` | Duration | `netem delay` (LNK-01). |
| `jitter` | `amount`: Duration, `distribution`: `uniform` (default) or `normal` | `netem delay D J`, with `distribution normal` for the second one (LNK-02). **Needs `delay`.** A delay below zero cannot exist, and `netem` would silently clamp it to zero, so the format refuses a jitter that reaches below zero: `uniform` needs `amount` not greater than `delay`, `normal` needs `delay` at least four times `amount` (the tables of `netem` are cut at about four sigma, which is also why a normal jitter never reaches its far tail, see COVERAGE). |
| `loss` | `model: random`, `p`: Probability | `netem loss` (LNK-03). Other models (`gilbert-elliott`, `markov4`, `trace`) are planned for 0.2 and 0.3. |
| `rate` | Rate | `netem rate` (LNK-08). |

The queue of the link is the default of the kernel (`limit 1000` packets) and is recorded in the result. A `queue` key (limit and discipline, LNK-09) arrives with release 0.3.

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

`udp-cbr` is the only kind in release 0.1 (WRK-01): UDP at a constant bit rate, sent by the agent in `from`, received by the agent in `to`.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | A Name, unique among workloads. Metrics are named after it (`media.delay.p99`). |
| `kind` | yes | `udp-cbr`. |
| `from`, `to` | yes | Two different nodes from `nodes`. |
| `rate` | yes | A Rate. **The rate of the payload**: the number of payload bits per second. The packets per second follow from it and `packet-size`. |
| `packet-size` | yes | A Size: the payload of one packet. From the size of the header of the agent (16 bytes) to 1472 bytes, the largest payload that fits the MTU of 1500 without fragmentation (fragmentation arrives with release 0.3). |
| `start` | no | A Duration, default `0s`. Relative to the start of the measured phase. |
| `stop` | no | A Duration, default `duration`. Must be greater than `start` and not greater than `duration`. |

Packets per second are at most **100,000** per workload. A higher rate is an error that says so: the agent was measured up to that rate (spike S8) and the format does not promise more than has been measured.

Every `udp-cbr` workload produces a record per packet (sequence number, send time, receive time, PRB-01) and the metrics of PRB-02, PRB-03 and PRB-05: packets sent, received and lost, delivered ratio, one-way delay (mean, percentiles, maximum) and throughput and goodput over time. The metric names and the result format are defined in `docs/RESULTS.md`, which arrives with the result format (RPT-01). The assertions of release 0.1 are written in test code (ASR-01), not in the file.

## Keys that are reserved

These keys are not part of version 0 in release 0.1. A scenario that uses one is refused with the message "`<key>` is planned for release X (see FEATURES.md)", not with "unknown key".

| Key | Planned in | Meaning |
| --- | --- | --- |
| `timeline`, `repetitions`, `probes` | 0.2 | Events over time, repeated runs, extra measurements. |
| `allow-limited`, `expect`, `params`, `include` | 0.3 | Accepted restrictions, assertions in the file, parameters and includes. |
| `matrix` | 0.5 | Sweeps. |
| `mobility` | 0.6 | Positions over time. |
| `model` (in a link) | 0.2 and 0.3 | Link models: steps, trace, Markov, periodic. |
| `queue` (in a direction) | 0.3 | Queue limit and discipline. |
| `address` (node, link) | later | Fixed addresses. |
| `kind` other than `udp-cbr` | 0.2 and 0.3 | `command` (your program), `tcp-bulk`, `request-response` and more. |

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

error: `timeline` is planned for release 0.2 (see FEATURES.md)
  --> access.yaml:18:1
   |
17 |     packet-size: 1200b
18 | timeline:
   | ^^^^^^^^
   = path: timeline
```

The messages for syntax, types, unknown and duplicate keys come from the YAML library and are shown in the words of this format; the others come from the checks of the model (D-020). The scenarios in `scenarios/invalid/` are faulty on purpose, one fault per file (and one with several), and their messages are held in snapshots, so a change in a message is a change in a test (QA-01).

A scenario that is built in code goes through the same checks ([`Scenario::validate`](../crates/netscenario-model/src/validate.rs)); its problems have the path but no line.

The JSON Schema of the format is [`docs/schema/scenario-v0.json`](schema/scenario-v0.json), generated from the types; a test fails when it is out of date and `just schema` writes it. A schema can say what a shape is (keys, types, the pattern of a value) and not what the contents mean, so the checks above are not in it.

## Part 2: planned, not binding

The following sketches show where the format is heading. They are **not** part of version 0 in release 0.1 and may change. The release in brackets is the plan of `FEATURES.md`.

**Timeline (0.2).** Events over time: `at`, a target (`link`, optionally a direction), and an action (`set`, `outage`, `down`, `up`). Times are relative to the start of the measured phase. An event is applied before the first packet that its workload sends at or after its moment (D-038); the result records when each event was really applied and how many packets had entered the queue.

```yaml
timeline:
  - { at: 10s, link: access, set: { forward.rate: 2mbit } }
  - { at: 20s, link: access, outage: 2s }
```

**More loss models (0.2).** The parameters follow the `netem` documentation so that the mapping can be checked; there are no aliases. In `gilbert-elliott` `loss-bad` and `loss-good` are the loss probabilities in the two states (the third and fourth argument of `netem loss gemodel`, measured in S12); in `markov4` the five numbers are the transitions in the order of the names. There is no `correlation` key for any loss, duplication or reordering (D-048).

```yaml
loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }
loss: { model: markov4, p13: 2%, p31: 30%, p32: 10%, p23: 20%, p14: 0.5% }
```

**Corruption and reordering (0.2).** `corrupt: 1%` flips a bit in the frame; a receiver with the UDP checksum on drops it (D-040), so the program sees loss, and the result tells it from a loss in the queue. `udp-checksum: off` on a `udp-cbr` workload lets the corrupted payload arrive, and the result then counts `corrupted`. `reorder: { share: 25%, gap: 5 }` needs a `delay`: the share is the share of packets sent on at once (D-048).

**Your program as a node (0.2).**

```yaml
workloads:
  - id: app
    kind: command
    node: server
    run: ["./target/release/my-server", "--listen", "{server.addr}:4433"]
    ready: { udp-port: 4433, within: 5s }
    stop: { signal: TERM, within: 3s }
```

A `command` runs in the namespace of its node, as the invoking user, with the working directory of the scenario file. It is the only way for a scenario to run code, and it always says so in the file.

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
