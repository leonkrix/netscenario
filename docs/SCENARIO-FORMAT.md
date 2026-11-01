# Scenario format, version 0 (release 0.1)

**Status: the contract for release 0.1, with the impairments, the timeline and the program of release 0.2 added** (the format reads them; the Kernel backend applies the loss models, the reordering, the duplication and the corruption since steps 4.2a and 4.2b, and refuses what it does not apply yet - the timeline, the program - and says so). Part 1 describes exactly what a scenario file may contain. Anything not in Part 1 is not part of the format and is refused with an error that says in which release it is planned. Part 2 sketches what comes later and is not binding. A JSON Schema is generated from the Rust types (`netscenario schema`, SCN-01, committed as `docs/schema/scenario-v0.json`); that schema and the tests are the final authority, and this page is checked against them.

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
timeline: [ ... ]       # optional: changes over time (release 0.2)
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
| `corrupt` | Probability | `netem corrupt` (LNK-07). One bit of the frame is flipped, anywhere in it, the Ethernet header included; a receiver with the UDP checksum on drops the packet (D-040), so the program sees loss, and the result tells the two apart (`directions`, [RESULTS.md](RESULTS.md)). The share of packets that are dropped is a little less than `P`: a bit in the source address of the Ethernet header harms nothing. |
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
| `outage` | Duration | Nothing is delivered for this long (a loss of 100 %), and the sender notices nothing (D-039). Afterwards the link has the values it had. |
| `direction` | `forward`, `reverse` or `both` (the default) | For an `outage` only. |
| `down` | `true` | The interface of the link goes down: the sender gets errors, and the agent counts them as `send_errors` (D-039). |
| `up` | `true` | The interface comes up again. |

**`set` is a merge patch** (the rule of RFC 7386): a key that is not written stays as it is, a value replaces the old one as a whole (a `loss` replaces the loss, it does not merge into it), and `null` takes the impairment away. `both` changes the two directions. After every step the direction must be a valid set of impairments, and the reader checks it: `delay: null` while a `jitter` is set is refused, and the message says that it is this step.

**What the reader refuses besides the types:** an event that does none or several of the four things; a link that does not exist (with a hint); a moment at or after the end; an outage that is not longer than zero, that ends after the end of the measured phase, or that overlaps another outage of the same direction (the end of the first would be unclear); a `loss` that is set inside an outage of its direction (an outage is a loss of 100 %); a link that goes down twice or comes up while it is up; a `direction` on anything but an outage; a step that writes no key.

**When it happens.** An event is applied before the first packet that its workload sends at or after its moment (D-038), so the same seed gives the same lost packets; the result records when each event was really applied and how many packets had entered the queue. The Kernel backend applies the timeline from step 4.3 of release 0.2; until then it refuses a scenario with one and names the feature ([COVERAGE.md](COVERAGE.md)).

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

A workload has a `kind`. `udp-cbr` (WRK-01) is UDP at a constant bit rate, sent by the agent in `from`, received by the agent in `to`. `command` (WRK-06, release 0.2) is a program of your own that runs in a node, see [A program of your own](#a-program-of-your-own). The keys of one kind are refused in the other (`rate` in a `command`), each at its own line.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | A Name, unique among workloads. Metrics are named after it (`media.delay.p99`). |
| `kind` | yes | `udp-cbr` or `command`. |
| `from`, `to` | yes | Two different nodes from `nodes`. |
| `rate` | yes | A Rate. **The rate of the payload**: the number of payload bits per second. The packets per second follow from it and `packet-size`. |
| `packet-size` | yes | A Size: the payload of one packet. From the size of the header of the agent (16 bytes) to 1472 bytes, the largest payload that fits the MTU of 1500 without fragmentation (fragmentation arrives with release 0.3). |
| `start` | no | A Duration, default `0s`. Relative to the start of the measured phase. |
| `stop` | no | A Duration, default `duration`. Must be greater than `start` and not greater than `duration`. |

Packets per second are at most **100,000** per workload. A higher rate is an error that says so: the agent was measured up to that rate (spike S8) and the format does not promise more than has been measured.

Every `udp-cbr` workload produces a record per packet (sequence number, send time, receive time, PRB-01) and the metrics of PRB-02, PRB-03 and PRB-05: packets sent, received and lost, delivered ratio, one-way delay (mean, percentiles, maximum) and throughput and goodput over time. The metric names and the result format are defined in `docs/RESULTS.md`, which arrives with the result format (RPT-01). The assertions of release 0.1 are written in test code (ASR-01), not in the file.

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

**What the Kernel backend does with it.** It starts programs from step 4.6 of release 0.2; until then it refuses a scenario with a `command` and names the feature ([COVERAGE.md](COVERAGE.md)). Running a scenario file that you did not write is running its programs: read the `command` entries first ([SECURITY.md](../SECURITY.md)).

## Keys that are reserved

These keys are not part of version 0 in release 0.1. A scenario that uses one is refused with the message "`<key>` is planned for release X (see FEATURES.md)", not with "unknown key".

| Key | Planned in | Meaning |
| --- | --- | --- |
| `repetitions`, `probes` | 0.2 | Repeated runs, extra measurements. |
| `allow-limited`, `expect`, `params`, `include` | 0.3 | Accepted restrictions, assertions in the file, parameters and includes. |
| `matrix` | 0.5 | Sweeps. |
| `mobility` | 0.6 | Positions over time. |
| `model` (in a link) | 0.2 and 0.3 | Link models: steps, trace, Markov, periodic. |
| `queue` (in a direction) | 0.3 | Queue limit and discipline. |
| `address` (node, link) | later | Fixed addresses. |
| `kind` other than `udp-cbr` and `command` | 0.3 and later | `tcp-bulk`, `request-response`, `udp-poisson` and more. |

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

**Corruption that arrives (0.2).** `udp-checksum: off` on a `udp-cbr` workload lets the corrupted payload arrive, and the result then counts `corrupted` (D-040). The keys `corrupt`, `reorder`, `duplicate` and the loss models are in Part 1.

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
