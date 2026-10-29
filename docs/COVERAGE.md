# Coverage matrix

What each backend reproduces **faithfully**, what it reproduces **only in part**, and what it **refuses**. This page is the contract behind the first principle of netscenario: a scenario that asks for something a backend cannot do correctly fails with an error. It is never quietly approximated.

## How to read it

Each cell is a mark and a release.

| Mark | Meaning |
| --- | --- |
| `✔ 0.1` | Faithful, planned for that release. When it ships, the cell gets a link to the test that proves it. |
| `~ 0.4` | Limited: it works with a restriction that is written next to the table. The scenario must accept the restriction explicitly (`allow-limited`), otherwise it is refused. |
| `✘` | Refused with an error that says why and which backend can do it. |
| `?` | Not known until an [experiment](../spikes/README.md) is done. |
| `·` | Does not apply. |

Nothing is implemented yet. Every mark below is a plan, not a measurement. The backends:

- **Kernel**: Linux network namespaces, virtual Ethernet pairs and `netem`/`tc`. The reference. Needs root or an unprivileged user namespace.
- **Engine**: a packet-level emulator written for this project, running in userspace on virtual devices inside a user namespace. Linux, no root.
- **Proxy**: a portable mode in which the program under test talks to a process that sits in between. No root, any operating system, but not a faithful network.

Docker is not a column: it is a way to start programs and to reach the Kernel backend from a container, so it has the Kernel column's marks (and needs `NET_ADMIN`).

## Topology

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Point to point | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Star | ✔ 0.3 | ✔ 0.4 | ~ 0.5 |
| Chain with routing | ✔ 0.3 | ✔ 0.4 | ✘ |
| Dumbbell (shared bottleneck) | ✔ 0.3 | ✔ 0.4 | ✘ |
| Multipath, multi-homing | ✔ 0.3 | ✔ 0.4 | ✘ |
| Arbitrary graph | ✔ 0.3 | ✔ 0.4 | ✘ |
| Links up and down | ✔ 0.2 | ✔ 0.4 | ✔ 0.5 |
| Nodes joining and leaving | ✔ 0.6 | ✔ 0.6 | ✘ |
| IPv4 | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| IPv6 | ✔ 0.3 | ✔ 0.4 | ~ 0.5 |
| NAT, firewall | ? | ? | ✘ |
| Broadcast, multicast | ? | ? | ✘ |

## Impairments

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Fixed delay | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Jitter (uniform, normal) | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Jitter (pareto, tables) | ✔ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Independent loss | ✔ 0.1 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Burst loss (Gilbert-Elliott, four-state) | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Reordering | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Duplication | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Corruption | ✔ 0.2 | ✔ 0.4 | ✘ |
| Rate limit | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Queue limit | ✔ 0.3 | ✔ 0.4 | ~ 0.5 |
| Queue disciplines (CoDel, FQ-CoDel, PIE) | ✔ 0.3 | ✔ 0.4 | ✘ |
| MTU, fragmentation | ✔ 0.3 | ✔ 0.4 | ✘ |
| Different values per direction | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Outages | ✔ 0.2 | ✔ 0.4 | ✔ 0.5 |
| ECN marking | ? | ✔ 0.4 | ✘ |

**Restriction "UDP only" for the Proxy**: the proxy sits between the program and the network. For UDP it handles every datagram, so loss, reordering and duplication are exact. For TCP it ends the connection itself, so the TCP stack of the program never sees a lost packet. Delay, rate and outages act on the byte stream, but loss, reordering and duplication do not exist for TCP in this backend, and a scenario that asks for them with TCP traffic is refused.

## Change over time

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Scheduled steps | ✔ 0.2 | ✔ 0.4 | ✔ 0.5 |
| Trace-driven rate, delay, loss | ~ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Markov models | ~ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Periodic patterns | ✔ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Geometry-based links | ~ 0.6 | ✔ 0.6 | ~ 0.6 |

**Restriction "trace-driven" for the Kernel**: `netem` has no clock of its own, so the tool changes its parameters at the steps of the trace. The resolution is limited by how fast the kernel accepts changes (to be measured by the experiment S5), and the delivery is not per packet.

## Mobility

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Static positions | ✔ 0.6 | ✔ 0.6 | ✔ 0.6 |
| Waypoint paths, random models | ✔ 0.6 | ✔ 0.6 | ✔ 0.6 |
| Imported mobility traces | ✔ 0.6 | ✔ 0.6 | ✔ 0.6 |
| Orbital motion | ✔ 0.6 | ✔ 0.6 | ✔ 0.6 |
| Handover between access links | ✔ 0.6 | ✔ 0.6 | ~ 0.6 |

Mobility only changes the parameters of links over time (through the geometry-based models), so it inherits the restrictions of "Change over time".

## Workloads and measurement

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| UDP traffic generators | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| TCP traffic generators | ✔ 0.3 | ✔ 0.4 | ~ 0.5 |
| Request and response | ✔ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Your program as a node | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (addresses are the proxy's) |
| A container as a node | ✔ 0.5 | ? | ✘ |
| Per-packet records | ✔ 0.1 | ✔ 0.4 | ~ 0.5 (what passes the proxy) |
| One-way delay | ✔ 0.1 | ✔ 0.4 | ~ 0.5 |
| Packet capture | ✔ 0.2 | ✔ 0.4 | ✘ |
| Kernel queue statistics | ✔ 0.3 | · | · |

One-way delay is exact because all nodes share one machine and one clock. For the Proxy the delay includes the time spent in the proxy itself, which the result states.

## Protocol behaviour

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| UDP and everything on it (QUIC, RTP, custom protocols) | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| TCP behaviour under loss and delay (congestion control) | ✔ 0.3 | ✔ 0.4 | ✘ |
| ICMP | ✔ 0.1 | ✔ 0.4 | ✘ |

## Reproducibility

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Same seed, same loss pattern | ? (depends on the `netem` seed of the kernel) | ✔ 0.4 | ✔ 0.5 |
| Same seed, same timing | ~ (scheduling noise) | ✔ 0.4 (optional virtual time) | ~ |
| Provenance in every result | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |

## Where it can run

| Place | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Linux, as root | ✔ 0.1 | ✔ 0.4 | ✔ 0.5 |
| Linux, unprivileged user namespace | ? (0.1 if the experiments allow) | ✔ 0.4 | ✔ 0.5 |
| Container with `NET_ADMIN` | ✔ 0.5 | ✔ 0.5 | ✔ 0.5 |
| Container without extra rights | ✘ | ? | ✔ 0.5 |
| GitHub-hosted runner | ? | ? | ✔ 0.5 |
| Windows, macOS | ✘ | ✘ | ✔ 0.5 |

## Rules for changing this page

1. A cell moves from a plan to a check mark with a link to the test that proves it. No test, no check mark.
2. A restriction is written next to the table, with the reason.
3. A backend that cannot do a feature faithfully says `✘`. It is not turned into `~` to look better.
4. The page is the source of the chapter on fidelity in the user guide (DOC-06).
