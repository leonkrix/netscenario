# The agent

The agent is a small program, `netscenario-agent`, that runs inside a node and does the measuring (D-008, WRK-01, PRB-01). A scenario never needs `iperf` or anything else in the node. The Kernel backend starts the agent by path in the network namespace of a node; the same program serves later backends.

Two agents cooperate: a **sender** puts a sequence number and the send time into every packet of a constant bit rate stream, a **receiver** writes one record per arriving packet. Both read `CLOCK_MONOTONIC`, which every node of a machine shares, so the **one-way delay is the difference of two readings of the same clock**, not an estimate. The send time is taken right before the packet is handed to the kernel, the receive time right after the kernel hands it to the receiver.

## The packet (wire format, version 1)

The payload of each UDP packet starts with 16 bytes in network byte order; the rest is zeros up to the `packet-size` of the workload (the **payload**, 16 to 1472 bytes; the rate of a workload counts payload bits).

| Bytes | Field | Meaning |
| --- | --- | --- |
| 0 to 1 | magic | `0x4E53` (`NS`). A datagram without it is not a packet of the agent: the receiver counts it as `foreign` and does not record it. |
| 2 | version | `1`. Another version is not read. |
| 3 | flags | `0`. Reserved; a packet with a flag set is not read. |
| 4 to 7 | sequence | From 0, one more for each packet. |
| 8 to 15 | send time | `CLOCK_MONOTONIC` in nanoseconds. |

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
                       [--flow <n>] [--start-at-ns <monotonic ns>]
netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
```

Numbers are plain integers: bits per second, bytes, nanoseconds. The units of a scenario file are resolved by the program that starts the agent. Exit codes: 0 done, 1 failed (the reason is on standard error), 2 the command line is wrong.

The agents print one JSON object per line on standard output; the types are `netscenario_agent::events::Event`, used by both ends:

```json
{"event":"ready","flow":1,"address":"10.100.0.2:40000"}
{"event":"received","flow":1,"packets":100,"foreign":0,"first_receive_ns":123,"last_receive_ns":456}
{"event":"sent","flow":1,"packets":100,"first_send_ns":120,"last_send_ns":450,"max_lateness_ns":9000,"errors":0,"first_error":null}
```

- `recv` prints `ready` once it listens (the sender starts after it), runs until it reads a line `stop` or the end of its standard input (so a receiver whose supervisor died does not run for ever), or until `--max-time-ns`. Packets that are already queued when the stop comes are still recorded, for at most 100 ms. `--ignore-stdin` is for use by hand.
- `send` starts at `--start-at-ns` (a reading of `CLOCK_MONOTONIC`, so that the supervisor can start several workloads at one moment) or at once.

## How the stream is timed

Packet `n` leaves at `start + n × size × 8 / rate`, computed in integers from `n` and not by adding an interval, so an interval that is not a whole number of nanoseconds (3 mbit with 1000 byte packets: 2.666… ms) does not drift. The sender sleeps until shortly before the moment and spins for the last 150 microseconds, because a sleep wakes up tens of microseconds late. **A sending agent therefore uses one core fully while it runs.** `max_lateness_ns` in the summary is the largest amount by which a packet left after its moment: it tells how much of a gap in a result comes from the machine and not from the network.

## Not in this version

- **Static binary.** D-008 spoke of a static (musl) binary. In the Kernel backend the nodes share the file system of the host and need none; the static build belongs to the release (REL-01) and to backends where the node has its own file system.
- Hardware or kernel timestamps (`SO_TIMESTAMPNS`): the receive time is taken in user space after the read. At the measured rates the error is a few microseconds (S8).
- Poisson, on/off and bursty traffic (WRK-02), TCP (WRK-04), request and response (WRK-05).

## How a run starts the agent

The Kernel backend starts the agent as any program of a node (`Backend::start_program`): in the network namespace of the node, with its standard output, standard error and standard input connected to the supervisor. The caller then asks what the program did (`poll_program`: new lines, and how it ended) and writes to it (`send_line`, for `stop`). The order of a stream, as the test `tests/agent.rs` in the Kernel backend runs it: start the receiver, wait for its `ready` line, start the sender, wait until it ends, wait for the packets in flight, send `stop` to the receiver, wait until it ends, read the record file. The files are written by the agent into a directory of the run, which the nodes share with the host.

**Memory.** A program can be started with a limit on its address space (`Program::memory_limit`, set with `prlimit --as` before the program starts). A program that needs more fails to allocate and ends; it does not take the machine down (SAF-02). A test starts a program that needs 150 MB with a limit of 64 MiB and shows that it does not get it.

