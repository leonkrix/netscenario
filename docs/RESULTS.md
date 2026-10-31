# The result of a run

A run writes a directory (`--out`, or the `out_dir` of `RunOptions`) with these files:

| File | What it is |
| --- | --- |
| `result.json` | The result: provenance, seed, and the statistics of each workload. This page describes it. |
| `flow-<n>.records` | The raw per-packet records of workload `n` (the order of `workloads` in the scenario, from 0), in the binary format of [AGENT.md](AGENT.md). One record for each packet that arrived. |
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
      "records_file": "flow-0.records",
      "packets_file": "packets-media.jsonl"
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
| `loss_reproducible` | Whether the same seed gives the same lost packets. `false` for a scenario with loss on a backend that cannot apply a seed (the Kernel backend with iproute2 older than 6.6.0), and `false` if a queue counted packets that the workloads did not send (`queue_checks`). **What `true` promises:** the same seed gives the same lost packets *if the packets enter the queue in the same order*. With one workload in a direction that order is its own schedule. With two workloads in one direction their packets can be microseconds apart, and which one reaches the queue first changes from run to run on a real kernel; the number of packets still agrees, so this release says `true` there, which is more than it can promise (release 0.2 says `false` and why; the Engine backend of release 0.4 has no such limit): such a run is a sample, not a repeatable experiment, and this field says so. |
| `queue_checks` | For each direction with random loss, one entry: the `link`, the `direction` (`forward` is from the first node of `between` to the second), the packets that the workloads `expected` to send into it (those the kernel took) and the packets the queue `counted`. The pattern of lost packets is a function of the seed **and of the number of packets that enter the queue**, so a run is repeatable only if the two agree (ENV-07, D-036). Empty if the backend cannot count, or the run has no seed. |

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
| `unexpected` | Records with a sequence number the sender never used. Always 0 with a correct sender and receiver; a value above 0 is a bug to report. |
| `delay` | The one-way delay of the packets that arrived (the first arrival of each), in nanoseconds: `min_ns`, `mean_ns` (rounded down), `p50_ns`, `p90_ns`, `p95_ns`, `p99_ns`, `p999_ns`, `max_ns`; `null` if no packet arrived. |
| `throughput` | `bin_ns` (one second) and `bits_per_second`: the payload bits that arrived in each second, counted from the moment the first packet was sent. See below. |
| `sender.max_lateness_ns` | The most that a packet left after its moment. A value that is large against the interval between packets (4.8 ms in the example) means that the machine, not the network, shaped the stream. |
| `sender.send_errors`, `sender.first_error` | Packets the kernel refused to take, and the first reason. |
| `foreign_datagrams` | Datagrams that reached the receiver and were not packets of the agent. |
| `records_file`, `packets_file` | The files of this workload, relative to `result.json`. `packets_file` is `null` unless the export was asked for. |

**Percentiles are the nearest rank.** For `n` delays in ascending order, percentile `p` is the delay at rank `ceil(p * n / 100)` (the first delay is rank 1). It is always a delay that was measured, never an interpolation. With few packets the high percentiles are the largest delays (`p999_ns` equals `max_ns` for up to 1000 packets).

**The throughput series** counts a packet in the second in which it *arrived*, with the seconds counted from the first packet that was sent. A bin is not scaled: the last bin of a stream is usually partly filled, and the packets still in flight when the sender stops fall into a bin of their own (the bin of 76,800 bit/s in the example). Read the series as "what came through each second", not as a rate that must equal the offered rate in every bin. The offered rate is `rate_bits_per_second`. For UDP the throughput and the goodput are the same number, because the payload is counted.

## The per-packet export (`packets-<id>.jsonl`)

One JSON object on each line, one line for each packet of the workload, in the order of the sequence numbers:

```jsonl
{"workload":"media","sequence":0,"send_ns":0,"receive_ns":43634952,"delay_ns":43634952}
{"workload":"media","sequence":1,"send_ns":4799944,"receive_ns":41036749,"delay_ns":36236805}
{"workload":"media","sequence":2,"send_ns":9599959,"receive_ns":43161561,"delay_ns":33561602}
{"workload":"media","sequence":17,"lost":true}
{"workload":"media","sequence":18,"duplicate":true,"send_ns":86400000,"receive_ns":131000000,"delay_ns":44600000}
```

(The first three lines are from the real run of the example; the last two show the other kinds of line.) `sequence` is the number of the packet. The times are **nanoseconds since the first packet of the workload was sent**, so that they mean something outside the machine. A packet that did not arrive has only `lost: true`. A second arrival of a packet follows the line of its first arrival with `duplicate: true`. A record with a sequence number the sender never used has `unexpected: true`. The export is the way to look at *which* packets were lost, for bursts and for the pattern of a seed.

## What a result does not say

- **Round trip time** (PRB-12) is not measured in release 0.1; the one-way delay is. The round trip needs a workload that is answered (WRK-05, release 0.3).
- **The extent of reordering** (PRB-04: how far a packet is out of place, and the RFC 3550 jitter) is not measured; `reordered` counts the packets, and the export shows which ones (a sequence number that arrives after a higher one).
- A packet that needed longer than `settle_ns` counts as lost. With a rate limit that builds a long queue, set the `settle` (the option exists in the library; the command line follows).
