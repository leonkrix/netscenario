# The agent

The agent is a small program, `netscenario-agent`, that runs inside a node and does the measuring (D-008, WRK-01, PRB-01). A scenario never needs `iperf` or anything else in the node. The Kernel backend starts the agent by path in the network namespace of a node; the same program serves later backends.

Two agents cooperate: a **sender** puts a sequence number and the send time into every packet of a constant bit rate stream, a **receiver** writes one record per arriving packet. Both read `CLOCK_MONOTONIC`, which every node of a machine shares, so the **one-way delay is the difference of two readings of the same clock**, not an estimate. The send time is taken right before the packet is handed to the kernel, the receive time right after the kernel hands it to the receiver.

## The packet (wire format, versions 1 and 2)

The payload of each UDP packet starts with 16 bytes (20 in version 2) in network byte order; the rest is zeros up to the `packet-size` of the workload (the **payload**, 16 to 1472 bytes, 20 to 1472 in version 2; the rate of a workload counts payload bits).

| Bytes | Field | Meaning |
| --- | --- | --- |
| 0 to 1 | magic | `0x4E53` (`NS`). A datagram without it is not a packet of the agent: the receiver counts it as `foreign` and does not record it. |
| 2 | version | `1`, or `2` for a stream without the checksum of UDP. Another version is not read. |
| 3 | flags | `0`. Reserved; a packet with a flag set is not read. |
| 4 to 7 | sequence | From 0, one more for each packet. |
| 8 to 15 | send time | `CLOCK_MONOTONIC` in nanoseconds. |
| 16 to 19 | CRC-32 | Version 2 only: the CRC-32 of every other byte of the payload. |

**Version 2 is for `udp-checksum: false` (D-040, D-056).** The sender writes `0` into the checksum field of UDP, which IPv4 allows (the receiving kernel then does not verify it), so a bit that the network flips in the payload is not caught on the way, and the CRC-32 lets the receiver tell a damaged packet from a good one. The CRC-32 is the one of Ethernet, zlib and PNG (polynomial `0x04C11DB7`, reflected, start and end value `0xFFFFFFFF`; the check value of `123456789` is `0xCBF43926`), written by hand in `netscenario_agent::crc` and tested against that value and against a bit-by-bit version for every length. It covers the whole payload except its own four bytes, the zeros at the end included, so a flipped bit anywhere in the payload is found (a CRC-32 finds every single flipped bit, and every burst of up to 32 bits). It does not cover the headers of IP and UDP: a bit in the source port of UDP, for example, arrives unnoticed, and the payload is intact, which is what the stream is about.

A sender without the checksum cannot use an ordinary UDP socket: `SO_NO_CHECK` has no safe wrapper in the libraries that the workspace uses, and the workspace forbids `unsafe` (D-025). It opens a **raw IPv4 socket** for the protocol UDP and writes the 8 bytes of the header of UDP itself, with the checksum field 0; the kernel adds the header of IP and chooses the route and the source address as for any socket. This needs `CAP_NET_RAW` in the node, which a user namespace and root have; `netscenario doctor` tests it (`raw-socket`), and a run refuses a workload with `udp-checksum: false` where a node does not have it, before anything is built. IPv6 is not possible (the checksum is required there).

The receiver of such a stream is told so (`--udp-checksum off`) and reads only version 2. A datagram that is not a good packet counts as `corrupted` and is not recorded: it cannot be told from a stranger, because the bit may have been flipped in the magic or the version. So `foreign` is always 0 there.

The sequence number has 32 bits. The sender refuses a stream with more than 2³² packets (at the highest supported rate of 100,000 packets per second that is about 11.9 hours) instead of letting the number wrap.

## The record file (version 1)

The receiver writes a binary file: a header of 16 bytes, then one record of 24 bytes for each packet in the order of arrival. A packet that did not arrive has no record; the sender's `packets` says how many were sent.

| Header bytes | Field       | Meaning                    |
| ------------ | ----------- | -------------------------- |
| 0 to 7       | magic       | `NSRECORD`                 |
| 8 to 9       | version     | `1`                        |
| 10 to 11     | record size | `24`                       |
| 12 to 15     | flow        | The number of the workload |

| Record bytes | Field        | Meaning                          |
| ------------ | ------------ | -------------------------------- |
| 0 to 7       | sequence     | From the packet                  |
| 8 to 15      | send time    | From the packet                  |
| 16 to 23     | receive time | `CLOCK_MONOTONIC` in nanoseconds |

A file that ends inside a record is an error when it is read, not a shorter file, because a lost record would look like a lost packet. The library `netscenario_agent::records` reads and writes it; a test pins the exact bytes. The run turns the file into statistics and, on request, into JSON Lines (RPT-01). It is binary because 100,000 packets per second for 30 seconds are 72 MB in this form and about three times that as JSON.

## The command line

```text
netscenario-agent send --to <addr> --rate <bits/s> --size <bytes> --duration-ns <ns>
                       [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>]
netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
                       [--udp-checksum <on|off>] [--wire-out <file>]
```

Numbers are plain integers: bits per second, bytes, nanoseconds. The units of a scenario file are resolved by the program that starts the agent. Exit codes: 0 done, 1 failed (the reason is on standard error), 2 the command line is wrong.

The agents print one JSON object per line on standard output; the types are `netscenario_agent::events::Event`, used by both ends:

```json
{"event":"ready","flow":1,"address":"10.100.0.2:40000"}
{"event":"ready","flow":1,"address":"10.100.0.2:40000","wire_error":"cannot read the frames of the interface: Operation not permitted (os error 1)"}
{"event":"received","flow":1,"packets":100,"foreign":0,"corrupted":0,"wire_packets":100,"first_receive_ns":123,"last_receive_ns":456}
{"event":"sent","flow":1,"packets":100,"first_send_ns":120,"last_send_ns":450,"max_lateness_ns":9000,"errors":0,"first_error":null}
```

- `recv` prints `ready` once it listens (the sender starts after it), runs until it reads a line `stop` or the end of its standard input (so a receiver whose supervisor died does not run for ever), or until `--max-time-ns`. Packets that are already queued when the stop comes are still recorded, for at most 100 ms. `--ignore-stdin` is for use by hand.
- `send` starts at `--start-at-ns` (a reading of `CLOCK_MONOTONIC`, so that the supervisor can start several workloads at one moment) or at once.

## The tap: naming the packets that the node drops (`--wire-out`)

A frame that the node drops because a bit in it was flipped never reaches the socket of the receiver, but a **packet socket** (`AF_PACKET`) sees it before the stack judges it. With `--wire-out <file>` the receiver opens one in its node (`netscenario_agent::tap`, `rustix`, no `unsafe`, not bound to an interface: the destination address and port select the stream) and writes a record, in the format above, for every frame that is a packet of its stream, delivered or not. The run takes the sequence numbers in that file that are not in the records of the receiver: those are the damaged packets (D-057).

A frame counts only if everything that says whose it is is intact: the type of Ethernet (IPv4), the version and header length of IP (`0x45`), its protocol (UDP) and destination address, the destination port of UDP, and the magic, the version, the flags and the sequence number of the agent: 18 bytes. A bit in any of them leaves the frame unnamed. That is stricter than the sequence number alone needs, on purpose: a frame with a damaged destination port cannot be given to a stream with certainty, and a wrong name is worse than none. The CRC-32 of a stream without the checksum is not checked here; a frame that fails it is the kind this is for.

The tap needs `CAP_NET_RAW` in the node. It is a help and not a condition: where it cannot be opened (or its file cannot be made) the receiver says why in the `wire_error` of its `ready` line and goes on recording, and the run reports the damaged packets as counted and not named. `received` has `wire_packets`, the number of packets the tap wrote, when it ran. The start of a stream waits for `ready`, so the tap is open before the first packet leaves.

## Barriers: waiting for the run at a packet (`--barriers`, D-038, D-062)

`send --barriers 12:0,40:1` makes the sender stop at two points of its stream: before packet 12 for the thing the run calls 0, and before packet 40 for 1. Right after it has sent the packet before (for packet 0 at the start) it prints a line `{"event":"barrier","flow":1,"id":0,"packet":12}`, and at the slot of packet 12, when it would send it, it goes on only if it has read the line `go 0` on its standard input; if the line is not there yet it waits for it. The run does its thing (a change of the link) between the two lines. The packets after the late one keep their absolute schedule: those that are due are sent at once, in the right order.

The sender never waits for ever. The end of its standard input answers every barrier (a run that died does not leave a sender behind), and an answer that does not come in 10 seconds is given up: the sender goes on, counts it in `barrier_timeouts` of its summary, and the run fails on it, because the guarantee does not hold for that barrier. The summary lists the barriers (`barriers`: `id`, `packet`, `asked_ns`, `held_ns`), so the run can say how long each packet waited. A barrier at a packet beyond the end of the stream is never asked.

## How the stream is timed

Packet `n` leaves at `start + n × size × 8 / rate`, computed in integers from `n` and not by adding an interval, so an interval that is not a whole number of nanoseconds (3 mbit with 1000 byte packets: 2.666… ms) does not drift. The sender sleeps until shortly before the moment and spins for the last 150 microseconds, because a sleep wakes up tens of microseconds late. **A sending agent therefore uses one core fully while it runs.** `max_lateness_ns` in the summary is the largest amount by which a packet left after its moment: it tells how much of a gap in a result comes from the machine and not from the network.

## Not in this version

- **Static binary.** D-008 spoke of a static (musl) binary. In the Kernel backend the nodes share the file system of the host and need none; the static build belongs to the release (REL-01) and to backends where the node has its own file system.
- Hardware or kernel timestamps (`SO_TIMESTAMPNS`): the receive time is taken in user space after the read. At the measured rates the error is a few microseconds (S8).
- Poisson, on/off and bursty traffic (WRK-02), TCP (WRK-04), request and response (WRK-05).

## How a run starts the agent

The Kernel backend starts the agent as any program of a node (`Backend::start_program`): in the network namespace of the node, with its standard output, standard error and standard input connected to the supervisor. The caller then asks what the program did (`poll_program`: new lines, and how it ended) and writes to it (`send_line`, for `stop`). The order of a stream, as the test `tests/agent.rs` in the Kernel backend runs it: start the receiver, wait for its `ready` line, start the sender, wait until it ends, wait for the packets in flight, send `stop` to the receiver, wait until it ends, read the record file. The files are written by the agent into a directory of the run, which the nodes share with the host.

**Memory.** A program can be started with a limit on its address space (`Program::memory_limit`, set with `prlimit --as` before the program starts). A program that needs more fails to allocate and ends; it does not take the machine down (SAF-02). A test starts a program that needs 150 MB with a limit of 64 MiB and shows that it does not get it.

