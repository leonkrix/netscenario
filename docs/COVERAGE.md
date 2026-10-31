# Coverage matrix

What each backend reproduces **faithfully**, what it reproduces **only in part**, and what it **refuses**. This page is the contract behind the first principle of netscenario: a scenario that asks for something a backend cannot do correctly fails with an error. It is never quietly approximated.

## How to read it

Each cell is a mark and a release.

| Mark    | Meaning                                                      |
| ------- | ------------------------------------------------------------ |
| `✔ 0.1` | Faithful, planned for release 0.1                            |
| `~ 0.4` | Limited, planned for release 0.4                             |
| `✘`     | Refused with an error                                        |
| `?`     | Not known until an [experiment](../spikes/README.md) is done |
| `·`     | Does not apply                                               |

A faithful mark is a plan until its cell links to the test that proves it. A limited mark works with a restriction that is written next to the table; the scenario must accept the restriction explicitly (`allow-limited`), otherwise it is refused. A refusal names the reason and a backend that can do it.

The links point to the tests that prove the mark: the fidelity tests of the Kernel backend ([FIDELITY.md](FIDELITY.md)) measure the impairments against their configuration, the others run a scenario in real namespaces. A test of the Kernel backend (`tests/coverage.rs`) fails when a faithful mark of the Kernel column has no link, or the link leads nowhere. The backends:

- **Kernel**: Linux network namespaces, virtual Ethernet pairs and `netem`/`tc`. The reference. Needs root or an unprivileged user namespace.
- **Engine**: a packet-level emulator written for this project, running in userspace on virtual devices inside a user namespace. Linux, no root.
- **Proxy**: a portable mode in which the program under test talks to a process that sits in between. No root, any operating system, but not a faithful network.

Docker is not a column: it is a way to start programs and to reach the Kernel backend from a container, so it has the Kernel column's marks (and needs `NET_ADMIN`, `SYS_ADMIN` and a seccomp profile that allows `unshare`, see "Where it can run").

## Topology

| Feature                      | Kernel                                                            | Engine | Proxy |
| ---------------------------- | ----------------------------------------------------------------- | ------ | ----- |
| Point to point               | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/run.rs)) | ✔ 0.4  | ✔ 0.5 |
| Star                         | ✔ 0.3                                                             | ✔ 0.4  | ~ 0.5 |
| Chain with routing           | ✔ 0.3                                                             | ✔ 0.4  | ✘     |
| Dumbbell (shared bottleneck) | ✔ 0.3                                                             | ✔ 0.4  | ✘     |
| Multipath, multi-homing      | ✔ 0.3                                                             | ✔ 0.4  | ✘     |
| Arbitrary graph              | ✔ 0.3                                                             | ✔ 0.4  | ✘     |
| Links up and down            | ✔ 0.2                                                             | ✔ 0.4  | ✔ 0.5 |
| Nodes joining and leaving    | ✔ 0.6                                                             | ✔ 0.6  | ✘     |
| IPv4                         | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/run.rs)) | ✔ 0.4  | ✔ 0.5 |
| IPv6                         | ✔ 0.3                                                             | ✔ 0.4  | ~ 0.5 |
| NAT, firewall                | ?                                                                 | ?      | ✘     |
| Broadcast, multicast         | ?                                                                 | ?      | ✘     |

## Impairments

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Fixed delay | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs): within 0.02 ms) | ✔ 0.4 | ✔ 0.5 |
| Jitter (uniform, normal) | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs); reorders packets, see below) | ✔ 0.4 | ✔ 0.5 |
| Jitter (pareto, tables) | ✔ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Independent loss | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs)) | ✔ 0.4 | ~ 0.5 (UDP only) |
| Burst loss (Gilbert-Elliott, four-state) | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Reordering | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Duplication | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (UDP only) |
| Corruption | ✔ 0.2 | ✔ 0.4 | ✘ |
| Rate limit | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs); counts Ethernet frames, see below) | ✔ 0.4 | ✔ 0.5 |
| Queue limit | ✔ 0.3 (until then derived, see below) | ✔ 0.4 | ~ 0.5 |
| Queue disciplines (CoDel, FQ-CoDel, PIE) | ✔ 0.3 | ✔ 0.4 | ✘ |
| MTU, fragmentation | ✔ 0.3 | ✔ 0.4 | ✘ |
| Different values per direction | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/provision.rs)) | ✔ 0.4 | ✔ 0.5 |
| Outages | ✔ 0.2 | ✔ 0.4 | ✔ 0.5 |
| ECN marking | ? | ✔ 0.4 | ✘ |

**What the Kernel backend does that a reader may not expect** (measured, [FIDELITY.md](FIDELITY.md)):

- **The rate counts Ethernet frames.** `netem rate 2mbit` limits the frames of the link, and a frame is the payload plus 42 bytes (8 UDP, 20 IP, 14 Ethernet). With payloads of 1,000 bytes the payload rate that gets through is 2 mbit × 1,000 / 1,042 = 1.919 mbit, and the measured one was 1.920. A scenario that wants 2 mbit of payload has to ask for more; the rate of a workload is the rate of the payload (`docs/RESULTS.md`).
- **Jitter reorders packets.** `netem` sends every packet at its own moment, so when the jitter is larger than the gap between two packets, a later packet can overtake an earlier one. With 5 ms of jitter and a packet every millisecond, 58 % of the packets (uniform) or 72 % (normal) arrived after a packet with a higher number. The results count them (`reordered`). The delay distribution itself is the configured one (mean, standard deviation and the ends were within the bounds of the tests).
- **A rate and a jitter in one direction are two qdiscs, in the order of the scenario.** A direction has the stages loss, rate, delay with jitter, in this order: a packet may be dropped when it enters, the packets that are kept are sent one after the other at the rate, and each then travels for its own delay. One `netem` qdisc cannot do that: with a `rate` it sends a packet at the later of its own moment and the moment of the packet in front of it, so the jitter is cut off from below, the order is kept and the mean delay is too high (a configured jitter of 5 ms came out as 4.1 ms and the mean as 42.7 ms instead of 41.0, measured and modelled, [FIDELITY.md](FIDELITY.md)). The Kernel backend therefore builds a direction with both from two qdiscs, the rate in the root and the delay with its jitter in its child, and measures what the scenario says: 41.06 ms, 5.01 ms, 28.6 % reordered (the model gives 41.01, 4.99 and 28.8). Every other direction has one qdisc. Decision D-030.
- **The delay is exact to the microsecond, and so is the smallest unit.** A time that is not a whole number of microseconds is refused, not rounded (see "Queue of the Kernel backend").

**What release 0.2 will do, as the spikes S11 to S14 measured it** (none of this is in 0.1; the decisions are [D-038](DECISIONS.md), D-040, D-047 and D-048):

- **Events happen between packets.** A change of the timeline is applied before the first packet of its workload at or after its moment, so the same seed gives the same lost packets (one workload in the direction). A timer would put the change one to eight packets late and, with a loss of 0 in between, move the whole pattern.
- **Corruption looks like loss to the program, and the result says which.** A bit flipped by `netem corrupt` (anywhere in the frame, the Ethernet header included) makes the receiver drop the packet, with the UDP checksum on (the normal case). The result separates the packets lost in the queue from the corrupted ones, exactly for the node, and by sequence number for most of them (94 to 99 percent in the spike). A workload that switches the checksum off sees the corrupted payload arrive.
- **A change keeps what it does not mention.** For `corrupt`, `reorder`, `rate` and `slot` `netem` keeps the old value, so the backend writes the whole state in every change, with explicit zeros.
- **`reorder P` sends a share P on at once and holds the rest.** The packets that arrive after a higher number are the held ones, about 1 - P of the stream when the delay is much larger than the gap between packets.
- **No correlation argument.** In `netem` the correlation of `loss random`, `duplicate` and `reorder` does not keep the probability (10 % loss with a correlation of 50 % lost 0.1 %), so the format does not offer it; bursts are `gemodel` and the four-state model.
- **A capture shows what the network delivered.** The tap sits after the queue of the interface, so a packet that the impairment dropped is in no capture; the capture is checked against the counter of the interface and says when it is incomplete.

**Queue of the Kernel backend before 0.3**: the queue of a link is not yet a key of the scenario (release 0.3). `netem` drops a packet when its queue is full and its default is 1,000 packets, which also counts the packets that are only travelling for their delay; with 20 ms of delay and 100,000 packets per second (the most a workload may send) 2,000 are in flight, so the default would lose packets that nobody asked to lose. The backend therefore sets `limit` to the packets in flight at the largest rate, 100,000 per second of delay plus jitter, plus the 1,000 of the default for queueing, at most 1,000,000 (a queue this long at the largest packet size takes 1.4 GB). Where a direction has a rate **and** a jitter, the two qdiscs share the work: the rate stage is the queue of the link and keeps the default of 1,000 packets, and the delay stage gets the packets in flight. A `tc qdisc show` of such a direction lists both.

**Restriction "UDP only" for the Proxy**: the proxy sits between the program and the network. For UDP it handles every datagram, so loss, reordering and duplication are exact. For TCP it ends the connection itself, so the TCP stack of the program never sees a lost packet. Delay, rate and outages act on the byte stream, but loss, reordering and duplication do not exist for TCP in this backend, and a scenario that asks for them with TCP traffic is refused.

## Change over time

| Feature                        | Kernel | Engine | Proxy |
| ------------------------------ | ------ | ------ | ----- |
| Scheduled steps                | ✔ 0.2  | ✔ 0.4  | ✔ 0.5 |
| Trace-driven rate, delay, loss | ~ 0.3  | ✔ 0.4  | ✔ 0.5 |
| Markov models                  | ~ 0.3  | ✔ 0.4  | ✔ 0.5 |
| Periodic patterns              | ✔ 0.3  | ✔ 0.4  | ✔ 0.5 |
| Geometry-based links           | ~ 0.6  | ✔ 0.6  | ~ 0.6 |

**Restriction "trace-driven" for the Kernel**: `netem` has no clock of its own, so the tool changes its parameters at the steps of the trace. The resolution is limited by how fast the kernel accepts changes (to be measured by the experiment S5), and the delivery is not per packet.

## Mobility

| Feature                       | Kernel | Engine | Proxy |
| ----------------------------- | ------ | ------ | ----- |
| Static positions              | ✔ 0.6  | ✔ 0.6  | ✔ 0.6 |
| Waypoint paths, random models | ✔ 0.6  | ✔ 0.6  | ✔ 0.6 |
| Imported mobility traces      | ✔ 0.6  | ✔ 0.6  | ✔ 0.6 |
| Orbital motion                | ✔ 0.6  | ✔ 0.6  | ✔ 0.6 |
| Handover between access links | ✔ 0.6  | ✔ 0.6  | ~ 0.6 |

Mobility only changes the parameters of links over time (through the geometry-based models), so it inherits the restrictions of "Change over time".

## Workloads and measurement

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| UDP traffic generators | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/agent.rs)) | ✔ 0.4 | ✔ 0.5 |
| TCP traffic generators | ✔ 0.3 | ✔ 0.4 | ~ 0.5 |
| Request and response | ✔ 0.3 | ✔ 0.4 | ✔ 0.5 |
| Your program as a node | ✔ 0.2 | ✔ 0.4 | ~ 0.5 (addresses are the proxy's) |
| A container as a node | ✔ 0.5 | ? | ✘ |
| Per-packet records | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/run.rs)) | ✔ 0.4 | ~ 0.5 (what passes the proxy) |
| One-way delay | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs)) | ✔ 0.4 | ~ 0.5 |
| Packet capture | ✔ 0.2 | ✔ 0.4 | ✘ |
| Kernel queue statistics | ✔ 0.3 | · | · |

One-way delay is exact because all nodes share one machine and one clock. For the Proxy the delay includes the time spent in the proxy itself, which the result states.

## Protocol behaviour

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| UDP and everything on it (QUIC, RTP, custom protocols) | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/fidelity.rs): UDP streams of any packet size) | ✔ 0.4 | ✔ 0.5 |
| TCP behaviour under loss and delay (congestion control) | ✔ 0.3 | ✔ 0.4 | ✘ |
| ICMP | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/provision.rs): `a_ping_takes_the_round_trip_of_both_directions`) | ✔ 0.4 | ✘ |

## Reproducibility

| Feature | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Same seed, same loss pattern | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/run.rs); needs iproute2 6.6.0 or newer) | ✔ 0.4 | ✔ 0.5 |
| Same seed, same timing | ~ (scheduling noise) | ✔ 0.4 (optional virtual time) | ~ |
| Provenance in every result | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/run.rs): `a_run_measures_the_link_and_writes_a_result`) | ✔ 0.4 | ✔ 0.5 |

## Where it can run

| Place | Kernel | Engine | Proxy |
| --- | --- | --- | --- |
| Linux, as root | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/provision.rs); the CI runs all of them as root) | ✔ 0.4 | ✔ 0.5 |
| Linux, unprivileged user namespace | ✔ 0.1 ([test](../crates/netscenario-backend-kernel/tests/provision.rs)) | ✔ 0.4 | ✔ 0.5 |
| Container with `NET_ADMIN` | ✘ alone, ✔ 0.5 with `SYS_ADMIN` and a seccomp profile that allows `unshare` | ✘ alone, ✔ 0.5 with `SYS_ADMIN` and a seccomp profile that allows `unshare` | ✔ 0.5 |
| Container without extra rights | ✘ | ✘ | ✔ 0.5 |
| GitHub-hosted runner | ✔ 0.1 ([the CI](../.github/workflows/ci.yml): `ubuntu-22.04` and `ubuntu-24.04`) | ✔ 0.4 | ✔ 0.5 |
| Windows, macOS | ✘ | ✘ | ✔ 0.5 |

**Restriction "same seed" for the Kernel**: A scenario that writes a `seed` and has loss is refused when the `tc` is older than that. Without a `seed` in the file it gets a drawn one, which the result records, if the `tc` can apply it; if not, the scenario runs and the result says that its loss is not reproducible (`Provisioned::loss_reproducible`). `netem` accepts a `seed` (the `tc` of iproute2 6.6.0 or newer; older versions, such as those of Ubuntu 22.04 and 24.04, do not know the word), which according to its manual guides the random loss and corruption events. Jitter, reordering and duplication are not promised to be reproducible. The pattern is a function of the seed **and of the number of packets that enter the queue, in the order in which they enter**, so any other packet in the namespace (IPv6 neighbour discovery, ARP) shifts it, and so does a different order of the packets of two workloads that send into the same direction (one workload per direction keeps its own order; the check of the counter does not see an exchange of two packets, and release 0.2 reports a second workload in a direction with loss as not reproducible). The tool therefore creates quiet namespaces (no IPv6, fixed neighbour entries) and, after the workloads have ended, compares the packet counter of the queue with the packets the workloads sent; if they differ, the result says that the run is not reproducible and names the queue (`queue_checks`, [RESULTS.md](RESULTS.md)). Measured so far (spike S2): the complete 200-packet patterns of two seeds are identical on kernels 6.8, 6.12 and 7.0, with `tc` 6.15 and 7.3.0, as root and as user, on three different machines. The kernel does not promise this across versions, so the guarantee is "same seed, same kernel and `tc` version", and the provenance of every result records both. The engine backend has no such condition.

**Notes on "Where it can run"** (spikes S1 and S6): unprivileged user namespaces work by default on Debian 13 and Ubuntu 22.04. Ubuntu 24.04 refuses them until `kernel.apparmor_restrict_unprivileged_userns` is 0 or an AppArmor profile allows the binary; `doctor` says so and [PERMISSIONS.md](PERMISSIONS.md) shows the ways out. In a container, `NET_ADMIN` alone is not enough: the default seccomp profile of Docker blocks `unshare`, so no namespace can be created. It works with `SYS_ADMIN` and `seccomp=unconfined` (a narrower profile that only allows `unshare` and `setns` is not tested yet). The rootless case inside a container is not tested yet. The engine has the same limits as the Kernel backend here: its nodes are network namespaces, and a virtual device (veth or TAP) needs `NET_ADMIN` in the namespace that owns it. This is derived from spike S6, not measured with the engine itself. In a container without extra rights only the Proxy backend works, because it needs nothing but ordinary sockets; if a container can reach a Docker daemon, the Docker backend (BKD-09) could start a helper container with the rights, which is a different case and a decision for later. On the GitHub images `ubuntu-22.04` (kernel 6.8) and `ubuntu-24.04` (kernel 7.0), with 4 virtual CPUs, the Kernel experiments S1 to S5 and the engine prototype S8 ran as the runner user (on 24.04 after the AppArmor setting was relaxed) and gave the same picture as on a development machine. The `tc` of both images (iproute2 5.15 and 6.1.0) is too old for `seed`, so the CI builds or installs a newer one (this is not needed when the tool sets the seed through netlink, which worked with the old `tc` in spike S7).

## Rules for changing this page

1. A cell moves from a plan to a check mark with a link to the test that proves it. No test, no check mark.
2. A restriction is written next to the table, with the reason.
3. A backend that cannot do a feature faithfully says `✘`. It is not turned into `~` to look better.
4. The page is the source of the chapter on fidelity in the user guide (DOC-06).
