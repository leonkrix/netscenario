# The result of a run

A run writes a directory (`--out`, or the `out_dir` of `RunOptions`) with these files:

| File | What it is |
| --- | --- |
| `result.json` | The result: provenance, seed, and the statistics of each workload. This page describes it. |
| `flow-<n>.records` | The raw per-packet records of workload `n` (the order of `workloads` in the scenario, from 0), in the binary format of [AGENT.md](AGENT.md). One record for each packet that arrived. |
| `flow-<n>.wire` | Only for a workload whose direction has `corrupt`: the packets that the receiving node saw on its interface, in the same binary format as `flow-<n>.records` (D-057). The damaged packets are the sequences in it that are not in the records. |
| `packets-<id>.jsonl` | Only if asked for (`export_packets`): one JSON line for each packet of workload `<id>`, described below. |

The result is JSON, indented, with a line feed at the end. `format` is always `netscenario-result` and `version` is the version of this format (RPT-01, ENV-06): **0** until 1.0, which means that a field may change its meaning in a minor release and the changelog says so. A new field does not change the version. A program that reads results should refuse a `version` it does not know; `netscenario_core::RunResult::from_json` does.

## An example

A run of a scenario like `scenarios/lte-like.yaml` with a duration of 3 seconds, on the development machine (the JSON of a real run, with the per-bin and per-percentile values as they came out):

```json
{
  "format": "netscenario-result",
  "version": 0,
  "scenario": {
    "name": "lte-like-link",
    "hash": "sha256:6771b8f9c6e4a4503da551740c3f9c375a478d5c67908b7b0f28e73da9dca1a9"
  },
  "provenance": {
    "netscenario": "0.0.0",
    "started_at": "2026-10-30T14:41:06Z",
    "backend": "kernel",
    "backend_details": {
      "rights": "user namespace",
      "tc": "iproute2 6.15.0"
    },
    "kernel": "6.12.113+deb13-amd64",
    "cpu_model": "AMD Ryzen 7 5800X 8-Core Processor",
    "cpus": 16
  },
  "seed": {
    "value": 42,
    "drawn": false,
    "loss_reproducible": true,
    "queue_checks": [
      { "link": "access", "direction": "forward", "expected": 6250, "counted": 6250 }
    ]
  },
  "accepted_restrictions": [],
  "addresses": [
    {
      "node": "client",
      "link": "access",
      "address": "10.100.0.1"
    },
    {
      "node": "server",
      "link": "access",
      "address": "10.100.0.2"
    }
  ],
  "timing": {
    "duration_ns": 3000000000,
    "settle_ns": 1120000000
  },
  "workloads": [
    {
      "id": "media",
      "kind": "udp-cbr",
      "from": "client",
      "to": "server",
      "from_address": "10.100.0.1",
      "to_address": "10.100.0.2:5000",
      "rate_bits_per_second": 2000000,
      "packet_size_bytes": 1200,
      "start_ns": 0,
      "duration_ns": 3000000000,
      "sent": 625,
      "received": 619,
      "lost": 6,
      "loss_ratio": 0.0096,
      "duplicates": 0,
      "reordered": 170,
      "unexpected": 0,
      "delay": {
        "min_ns": 28755827,
        "mean_ns": 41232904,
        "p50_ns": 41380790,
        "p90_ns": 47206574,
        "p95_ns": 49154956,
        "p99_ns": 52603430,
        "p999_ns": 56542754,
        "max_ns": 56542754
      },
      "throughput": {
        "bin_ns": 1000000000,
        "bits_per_second": [
          1920000.0,
          1968000.0,
          1977600.0,
          76800.0
        ]
      },
      "sender": {
        "max_lateness_ns": 143,
        "send_errors": 0,
        "first_error": null
      },
      "foreign_datagrams": 0,
      "corrupted": 0,
      "corrupted_named": null,
      "corrupted_seq": [],
      "corrupted_seq_unavailable": null,
      "records_file": "flow-0.records",
      "packets_file": "packets-media.jsonl"
    }
  ],
  "directions": [
    {
      "link": "access",
      "direction": "forward",
      "sent": 625,
      "reached_node": 622,
      "delivered": 619,
      "lost_in_queue": 3,
      "dropped_corrupted": 3,
      "receive_buffer_drops": 0
    }
  ]
}
```

## The fields

### Top level

| Field | Meaning |
| --- | --- |
| `format`, `version` | See above. |
| `scenario.name` | The `name` of the scenario. |
| `scenario.hash` | `sha256:` and the SHA-256 of the scenario in canonical JSON (the keys in the order of the format, keys with default values left out). A change of comments or of the layout of the file does not change it; a change of any value does. Two results with the same hash come from the same scenario. |
| `provenance` | Where, when and with what the result was made (ENV-02), below. |
| `seed` | The seed, below. |
| `accepted_restrictions` | The limits of the backend that the scenario accepted with `allow-limited` (release 0.3): `feature` and `restriction`. Empty in release 0.1, where a limit is refused. |
| `addresses` | The IPv4 address of each end of each link: `node`, `link`, `address`. They are assigned by the backend (`10.100.n.1` and `.2` for link `n` in the Kernel backend) and are not part of the scenario. |
| `timing.duration_ns` | The `duration` of the scenario. |
| `timing.settle_ns` | How long the run waited after the last packet was sent for the packets still in the network. A packet that needs longer counts as lost. Without a setting this is one second plus twice the longest hold of the scenario (the largest delay plus four times the largest jitter of any direction). |
| `workloads` | One entry for each workload, in the order of the scenario. |
| `directions` | Where the packets of each direction went, below. |
| `events` | What the run did to the links while it ran (the timeline), below. Empty for a scenario without one. |
| `notes` | What the backend says about how it did the run, one sentence for each fact (D-063): for example that the changes of a link were made by a `tc` process each because the `tc` that lives for the run stopped working. Not errors. Empty if the run went as usual. |

### `provenance`

| Field | Meaning |
| --- | --- |
| `netscenario` | The version of netscenario. |
| `started_at` | When the run started, UTC. |
| `backend`, `backend_details` | The backend and what it says about itself. The Kernel backend gives `tc` (the iproute2 version) and `rights` (`user namespace` or `own rights`). |
| `kernel` | The kernel release (`/proc/sys/kernel/osrelease`), or `null`. |
| `cpu_model`, `cpus` | The model of the first processor and the number of processors the run could use, or `null`. |

The host name, the user name and file paths are **not** recorded: a result may be shared.

### `seed` (ENV-01)

| Field | Meaning |
| --- | --- |
| `value` | The seed from which the seeds of all random parts of the run were derived (the loss of each direction), or `null` if the backend cannot apply a seed. |
| `drawn` | `true` if the scenario had no `seed` and the run drew one. Put `value` into the scenario as `seed` to repeat the run. |
| `loss_reproducible` | Whether the same seed gives the same lost packets. `false` for a scenario with loss, duplication, corruption or reordering on a backend that cannot apply a seed (the Kernel backend with iproute2 older than 6.6.0), `false` if a queue counted packets that the workloads did not send (`queue_checks`), and `false` for a scenario with **corruption**: the seed fixes which packets are chosen for it, but `netem` takes the place of the flipped bit from the generator of the kernel, and a bit in the source address of the Ethernet header (6 bytes of the frame) harms nothing, so now and then a corrupted packet arrives in one run and is dropped in another. The packets lost in the queue are the same (`lost_in_queue` of `directions` repeats exactly); the packets lost in all differ by a few (measured: one packet in about every 8 runs, with 41 corrupted packets of 1,200 bytes). **What `true` promises:** the same seed gives the same lost packets *if the packets enter the queue in the same order*. With one workload in a direction that order is its own schedule. With two workloads in one direction their packets can be microseconds apart, and which one reaches the queue first changes from run to run on a real kernel; the number of packets still agrees, so this release says `true` there, which is more than it can promise (release 0.2 says `false` and why; the Engine backend of release 0.4 has no such limit): such a run is a sample, not a repeatable experiment, and this field says so. |
| `not_reproducible_because` | One sentence for each reason `loss_reproducible` is `false`; empty otherwise. The command line prints them as warnings. |
| `queue_checks` | For each direction with random loss (any model), duplication, corruption or reordering, **except one that duplicates**, one entry: the `link`, the `direction` (`forward` is from the first node of `between` to the second), the packets that the workloads `expected` to send into it (those the kernel took) and the packets the queue `counted`. The pattern of lost packets is a function of the seed **and of the number of packets that enter the queue**, so a run is repeatable only if the two agree (ENV-07, D-036). Empty if the backend cannot count, or the run has no seed. A direction with `duplicate` has no entry: the queue counts the copies that it makes as well, and the count cannot tell them from a packet that nobody asked for, so the number cannot be compared (D-054); `loss_reproducible` stays `true` there, on the same condition as for a single workload. |

The seed of one direction is derived from the seed of the run, the index of the link and the direction (`netscenario_core::derive_seed`, SplitMix64). **The derivation is fixed**: a test pins its values, because a change would give old seeds new loss patterns. The timing of the machine is not seeded and not reproducible; `max_lateness_ns` says how much it mattered.

### Each entry of `workloads`

| Field | Meaning |
| --- | --- |
| `id`, `kind` | As in the scenario (`udp-cbr`). |
| `from`, `to` | The sending and the receiving node. |
| `from_address` | The address of the sender. |
| `to_address` | The address and the port the packets went to (port 5000 for the first workload, 5001 for the next). |
| `rate_bits_per_second`, `packet_size_bytes` | From the scenario: the rate of the **payload** and the size of the payload of one packet. |
| `start_ns`, `duration_ns` | When the workload started (from the start of the measured phase) and how long it sent. |
| `sent` | The packets the sender handed to the kernel. |
| `received` | The different packets that arrived. |
| `lost` | `sent - received`. |
| `loss_ratio` | `lost / sent` (0 if nothing was sent), a number between 0 and 1. |
| `duplicates` | Arrivals of a packet that had arrived before. They are not counted in `received` and not in the delays. |
| `reordered` | Packets that arrived after a packet with a higher sequence number (RFC 4737, type-P-reordered). A packet that is overtaken by several counts once; duplicates are not counted. `netem` reorders packets when the jitter is larger than the gap between them ([FIDELITY.md](FIDELITY.md)). |
| `reordering` | How far the reordered packets were out of place (RFC 4737), or `null` if none was: `extent` (`max`, `mean`, `p50`, `p95`, `p99`, in packets) and `late_time` (`max_ns`, `mean_ns`, `p50_ns`, `p95_ns`, `p99_ns`). See "The pattern of the arrivals" below. |
| `jitter` | The interarrival jitter of RFC 3550, or `null` for fewer than two arrivals: `last_ns`, `mean_ns` and `max_ns` of the running estimate. See below. |
| `loss_runs` | The runs of lost packets (RFC 3357): `runs`, `longest`, `mean` (0 if nothing was lost) and `histogram`, a list of `{length, runs}` by ascending length. See below. |
| `unexpected` | Records with a sequence number the sender never used. Always 0 with a correct sender and receiver; a value above 0 is a bug to report. |
| `delay` | The one-way delay of the packets that arrived (the first arrival of each), in nanoseconds: `min_ns`, `mean_ns` (rounded down), `p50_ns`, `p90_ns`, `p95_ns`, `p99_ns`, `p999_ns`, `max_ns`; `null` if no packet arrived. |
| `throughput` | `bin_ns` (one second) and `bits_per_second`: the payload bits that arrived in each second, counted from the moment the first packet was sent. See below. |
| `sender.max_lateness_ns` | The most that a packet left after its moment. A value that is large against the interval between packets (4.8 ms in the example) means that the machine, not the network, shaped the stream. |
| `sender.send_errors`, `sender.first_error` | Packets the kernel refused to take, and the first reason. |
| `foreign_datagrams` | Datagrams that reached the receiver and were not packets of the agent. |
| `corrupted` | Datagrams that reached the receiver damaged, in a workload with `udp-checksum: false`: the CRC-32 of the packet did not match, or its header was not one. They are not in `received` and not in the delays, and they **are in `lost`**, because a damaged packet is not an intact one; `lost - corrupted` is what never arrived. With the checksum on the kernel drops a damaged packet before the receiver sees it, so `corrupted` is 0 there, and such packets are told apart only per node, in `directions` (`dropped_corrupted`). Which packets are damaged is not named (the sequence number may be the damaged part). |
| `corrupted_named` | In a workload whose direction has `corrupt`: how many damaged packets the receiving node saw on the wire and did not deliver intact, with the sequence number readable (D-057). `null` where the direction does not corrupt, or where they could not be named (`corrupted_seq_unavailable`). It is read from frames, so it is **not** the count of damaged packets: a frame whose bit hit the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, and the magic, version, flags and sequence number of the agent) cannot be named, which is 82 % of the dropped frames named at 64 byte payloads, 92 % at 200 and 99 % at 1,200 ([FIDELITY.md](FIDELITY.md)). The exact count for the node is `dropped_corrupted` of `directions`; for a direction with one workload `dropped_corrupted + corrupted - corrupted_named` is the number that could not be named. A bit in the sequence number can make another number, which is named if it is in range and was not delivered (on a link that only corrupts that is a packet that was damaged too; with a loss in the direction it can be one that the queue dropped). |
| `corrupted_seq` | The first 1,000 of those sequence numbers, in ascending order; `--packets` marks all of them. |
| `corrupted_seq_unavailable` | Why a direction that corrupts could not be named, if it could not: the receiver needs `CAP_NET_RAW` in the node to read the interface (the run goes on without it), and the names are withdrawn where the node dropped datagrams for a full receive buffer, because a packet that is missing then may not be damaged. |
| `records_file`, `packets_file` | The files of this workload, relative to `result.json`. `packets_file` is `null` unless the export was asked for. |

### The pattern of the arrivals (PRB-04, PRB-06)

The ratio of lost packets and the delays do not say how the packets are spread. Three numbers do, each from the **first arrivals** in the order they happened (a packet that arrives twice counts once, at its first arrival):

- **Reordering.** A packet is reordered if it arrives after a packet with a higher number (`reordered`). Its **extent** is the number of arrivals between it and the earliest-arrived packet with a higher number (RFC 4737, section 4.2): a packet that comes right after the one that overtook it has extent 1; the largest extent is about the room a buffer needs to put the packets back in order. Its **late time** is the time between those two arrivals (section 4.3): how long that buffer would have waited. Both are over the reordered packets only. The code is checked against all the worked examples of the RFC, including the late times, and against the definition on every order of six packets.
- **Jitter.** The estimate of RFC 3550 (appendix A.8): for two packets that arrive one after the other, `D = (R_j - R_i) - (S_j - S_i)` is the difference of their transit times, and the estimate moves a sixteenth of the way to `|D|` with every packet. The pairs are **in the order of arrival**, as the RFC says, so a packet that is reordered makes a large `D`; for a link with a jitter this is more than the spread of the delays divided by three (a uniform delay of 10 ms gives 3.5 ms, measured and modelled, not 10 / 3 = 3.3). Send and receive times are readings of the one clock of the machine, so `D` is exact. The RFC defines the running estimate; a test wants one number for a run, so the result has `last_ns` (the estimate after the last packet, what a report at the end would carry), `mean_ns` (its mean over the run, which does not depend on the last sixteen packets: **use this one in a test**) and `max_ns`.
- **Loss runs.** A run is a stretch of consecutive sequence numbers none of which arrived (the "loss period" of RFC 3357). Independent loss gives mostly runs of length 1, bursts give longer ones: a link that loses 5 % in bursts of 10 and a link that loses 5 % one packet at a time are told apart here and nowhere else. A packet that arrived damaged (`corrupted`) did not arrive and is in a run, as it is in `lost`.

Real numbers of `scenarios/lte-like.yaml` (30 s, a normal jitter of 5 ms, 0.75 % loss):

```json
"reordering": { "extent": { "max": 5, "mean": 1.46, "p50": 1, "p95": 3, "p99": 4 },
                "late_time": { "max_ns": 20743100, "mean_ns": 4234990, "p50_ns": 3402984, "p95_ns": 11335475, "p99_ns": 15068642 } },
"jitter": { "last_ns": 5267369, "mean_ns": 5961500, "max_ns": 9546054 },
"loss_runs": { "runs": 47, "longest": 1, "mean": 1.0, "histogram": [ { "length": 1, "runs": 47 } ] }
```

### `events` (TML-01, D-060)

One entry for each **step** of the timeline, in the order the run did them. An event of the file can be more than one step: a `set` of `both` is one step for each direction.

| Field | Meaning |
| --- | --- |
| `event` | The place of the event in `timeline` of the scenario, from 0. |
| `kind` | `set`. (`outage-start`, `outage-end`, `down` and `up` come with the steps that do them.) |
| `link`, `direction` | The link, and `forward` or `reverse`; `null` for a step of the whole link. |
| `scheduled_ns` | When the file says it happens, in nanoseconds from the start of the measured phase, the moment at which the workloads start. |
| `started_ns` | When the run asked the backend to do it. |
| `applied_ns` | When the backend had done it. For a step by the clock `applied_ns - scheduled_ns` is how late the step was: the wait for the clock, the change itself (0.2 to 0.4 ms through the `tc` that lives for the run, about 2 ms with a `tc` process each, D-063) and what the machine added; it is never done before its moment. A step at a barrier is done when the sender has sent the packet before its moment. |
| `packet` | For a step at a barrier: the number of the first packet that met the new state (the number of packets that met the old one). `null` for a step by the clock. |
| `held_ns` | For a step at a barrier: how long that packet waited at its slot for the step to be done; 0 or a few microseconds if it was done by then. `null` for a step by the clock. |

**Two ways to do a step** (D-038, D-062). A step of a direction that a workload sends into is done **at a barrier**: the sender knows which of its packets is the first one at or after the moment, asks the run right after the packet before, and holds that packet at its slot until the run has done the step. `packet` is the number of that packet (the packets before it met the old state), and `held_ns` how long it waited; a change that is done before its slot costs the packet nothing (`held_ns` is a few microseconds). Because every packet of that workload meets the same state whatever the machine does, the same seed gives the same lost packets. The step is done when the sender has sent the packet before, so it can be done up to one gap before `scheduled_ns`. Any other step is done **by the clock**: `packet` is `null`, and a packet that leaves between `scheduled_ns` and `applied_ns` meets the old state. That is a few packets at the edge of a change of the rate or the delay and no more. For a step that changes what a queue draws random numbers for (a loss, an outage, a duplication, a corruption, a reordering) it also decides which packets the seed takes from there on, and the run says so: `seed.loss_reproducible` is `false` and `seed.not_reproducible_because` has the reason, for a step by the clock, and for a direction that two workloads send into, because the order in which their packets reach the queue is not fixed.

### `directions` (D-040)

A packet that never reaches the program can have been lost in the queue of the link or dropped by the node because a bit in it was flipped (`corrupt`). A program sees both as loss, and a test that is about corruption needs to know which. For each direction of a link that carried a workload there is one entry:

| Field | Meaning |
| --- | --- |
| `link`, `direction` | The link, and `forward` (from the first node of `between` to the second) or `reverse`. |
| `sent` | The packets the workloads put into the direction (those the kernel took). |
| `reached_node` | The frames that arrived at the interface of the receiving node, counted by the interface (`/proc/net/dev`, read before and after the workloads). |
| `delivered` | The datagrams that reached the sockets of the receivers: the packets of the agent, the others (`foreign_datagrams`) and the damaged ones of a stream without the checksum (`corrupted`). |
| `lost_in_queue` | `sent - reached_node`: the packets that the queue of the link dropped (the loss, a queue that was full). `null` in a direction with `duplicate`, where the frames include the copies, and when more frames arrived than were sent. |
| `dropped_corrupted` | `reached_node - delivered - receive_buffer_drops`: the frames that arrived and that the node dropped, because the bit that `netem` flipped made the checksum of IP or UDP wrong, or the address not the node's. |
| `receive_buffer_drops` | The datagrams the node dropped because the receive buffer of a socket was full (`RcvbufErrors` of UDP). This is a fault of the receiver and not of the network, so it is kept apart from `dropped_corrupted`. |

For a direction without `duplicate` and without a full buffer, `lost_in_queue + dropped_corrupted + delivered = sent`, to the packet; the fidelity cases check it ([FIDELITY.md](FIDELITY.md)). The counts are those of the interface and of the node, so they hold for all the workloads in a direction together. They are exact for the node and say nothing about which packet it was; `corrupted_named` and `corrupted_seq` of the workload name most of them (D-057). The command line lists a direction only where the node dropped something. The array is empty if the backend cannot count, and for a scenario with a program of your own (`kind: command`), whose packets the agent does not count.

**Percentiles are the nearest rank.** For `n` delays in ascending order, percentile `p` is the delay at rank `ceil(p * n / 100)` (the first delay is rank 1). It is always a delay that was measured, never an interpolation. With few packets the high percentiles are the largest delays (`p999_ns` equals `max_ns` for up to 1000 packets).

**The throughput series** counts a packet in the second in which it *arrived*, with the seconds counted from the first packet that was sent. A bin is not scaled: the last bin of a stream is usually partly filled, and the packets still in flight when the sender stops fall into a bin of their own (the bin of 76,800 bit/s in the example). Read the series as "what came through each second", not as a rate that must equal the offered rate in every bin. The offered rate is `rate_bits_per_second`. For UDP the throughput and the goodput are the same number, because the payload is counted.

## The per-packet export (`packets-<id>.jsonl`)

One JSON object on each line, one line for each packet of the workload, in the order of the sequence numbers:

```jsonl
{"workload":"media","sequence":0,"send_ns":0,"receive_ns":43634952,"delay_ns":43634952}
{"workload":"media","sequence":1,"send_ns":4799944,"receive_ns":41036749,"delay_ns":36236805}
{"workload":"media","sequence":2,"send_ns":9599959,"receive_ns":43161561,"delay_ns":33561602}
{"workload":"media","sequence":17,"lost":true}
{"workload":"media","sequence":19,"lost":true,"corrupted":true}
{"workload":"media","sequence":18,"duplicate":true,"send_ns":86400000,"receive_ns":131000000,"delay_ns":44600000}
```

(The first three lines are from the real run of the example; the last three show the other kinds of line. `corrupted` marks a lost packet that the receiving node saw on the wire and dropped as damaged: the `corrupted_seq` of the workload, all of them.) `sequence` is the number of the packet. The times are **nanoseconds since the first packet of the workload was sent**, so that they mean something outside the machine. A packet that did not arrive has only `lost: true`. A second arrival of a packet follows the line of its first arrival with `duplicate: true`. A record with a sequence number the sender never used has `unexpected: true`. The export is the way to look at *which* packets were lost, for bursts and for the pattern of a seed.

## What a result does not say

- **Round trip time** (PRB-12) is not measured in release 0.1; the one-way delay is. The round trip needs a workload that is answered (WRK-05, release 0.3).
- **The other metrics of RFC 4737** (the byte offset and the gap between reordered packets) and the loss distance of RFC 3357 are not computed. The export of every packet has what they need.
- A packet that needed longer than `settle_ns` counts as lost. With a rate limit that builds a long queue, set the `settle` (the option exists in the library; the command line follows).
