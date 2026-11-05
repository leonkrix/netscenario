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

`send` takes `--dont-fragment`: the socket is set to `IP_MTU_DISCOVER` `DO` (`IPV6_MTU_DISCOVER` over IPv6), so the system refuses a packet that does not fit the MTU of the link (`Message too long`), and the sender counts it in `errors` and in the refused sequence numbers. The target of `send` and the bind address of `recv` may be IPv6 (`[fd00:200::2]:5000`); a stream without the checksum is IPv4 only. The receiver reads datagrams up to 64 KiB, so a packet that was cut into fragments arrives whole.

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
netscenario-agent send --to <addr> --size <bytes> --duration-ns <ns> [--rate <bits/s>]
                       [--pattern cbr|poisson|onoff|bursty|trace] [--trace <file>] [--seed <n>] [--distribution fixed|exponential]
                       [--on-ns <ns> --off-ns <ns>] [--burst-packets <n> --gap-ns <ns> --pause-ns <ns>]
                       [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>]
netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
                       [--udp-checksum <on|off>] [--wire-out <file>] [--echo-to <addr>]
netscenario-agent tcp-serve --bind <addr> --out <file> [--flow <n>] [--congestion <name>] [--progress-interval-ns <ns>]
                            [--max-time-ns <ns>] [--ignore-stdin]
netscenario-agent tcp-bulk --to <addr> --out <file> (--bytes <n> | --duration-ns <ns> | both) [--flow <n>] [--connections <n>]
                           [--congestion <name>] [--chunk-bytes <n>] [--start-at-ns <monotonic ns>]
                           [--info-interval-ns <ns>] [--finish-timeout-ns <ns>] [--ignore-stdin]
netscenario-agent tcp-request --to <addr> --out <file> --request-bytes <n> --response-bytes <n> --duration-ns <ns>
                              [--interval-ns <ns>] [--flow <n>] [--connections <n>] [--congestion <name>]
                              [--start-at-ns <monotonic ns>] [--info-interval-ns <ns>] [--timeout-ns <ns>] [--ignore-stdin]
```

Numbers are plain integers: bits per second, bytes, nanoseconds. The units of a scenario file are resolved by the program that starts the agent. Exit codes: 0 done, 1 failed (the reason is on standard error), 2 the command line is wrong.

The agents print one JSON object per line on standard output; the types are `netscenario_agent::events::Event`, used by both ends:

```json
{"event":"ready","flow":1,"address":"10.100.0.2:40000"}
{"event":"ready","flow":1,"address":"10.100.0.2:40000","wire_error":"cannot read the frames of the interface: Operation not permitted (os error 1)"}
{"event":"received","flow":1,"packets":100,"foreign":0,"corrupted":0,"wire_packets":100,"first_receive_ns":123,"last_receive_ns":456}
{"event":"sent","flow":1,"packets":100,"first_send_ns":120,"last_send_ns":450,"max_lateness_ns":9000,"errors":0,"first_error":null,"refused":[]}
```

- `recv` prints `ready` once it listens (the sender starts after it), runs until it reads a line `stop` or the end of its standard input (so a receiver whose supervisor died does not run for ever), or until `--max-time-ns`. Packets that are already queued when the stop comes are still recorded, for at most 100 ms. `--ignore-stdin` is for use by hand.
- `send` starts at `--start-at-ns` (a reading of `CLOCK_MONOTONIC`, so that the supervisor can start several workloads at one moment) or at once.
- The sender reads its standard input also when it has no barriers. A line `stop` ends the stream before its next packet (also while it waits for a barrier or for the moment of the next packet of a slow stream, within 20 ms): the summary has `"stopped":true` and the packets sent so far; the packets that were never sent are not lost (D-066). The end of the input does not stop it.
- A packet that the kernel refuses to take (the interface is down, D-064) uses its sequence number all the same, so a packet keeps its place in time. `errors` counts them and `refused` lists them as runs `[first, last]`; `packets` counts only the ones that were handed to the kernel. A sender keeps at most 10,000 separate runs and counts the rest in `refused_unrecorded`, and the run fails on that: it could not say which packets were sent.

## The tap: naming the packets that the node drops (`--wire-out`)

A frame that the node drops because a bit in it was flipped never reaches the socket of the receiver, but a **packet socket** (`AF_PACKET`) sees it before the stack judges it. With `--wire-out <file>` the receiver opens one in its node (`netscenario_agent::tap`, `rustix`, no `unsafe`, not bound to an interface: the destination address and port select the stream) and writes a record, in the format above, for every frame that is a packet of its stream, delivered or not. The run takes the sequence numbers in that file that are not in the records of the receiver: those are the damaged packets (D-057).

A frame counts only if everything that says whose it is is intact: the type of Ethernet (IPv4), the version and header length of IP (`0x45`), its protocol (UDP) and destination address, the destination port of UDP, and the magic, the version, the flags and the sequence number of the agent: 18 bytes. A bit in any of them leaves the frame unnamed. That is stricter than the sequence number alone needs, on purpose: a frame with a damaged destination port cannot be given to a stream with certainty, and a wrong name is worse than none. The CRC-32 of a stream without the checksum is not checked here; a frame that fails it is the kind this is for.

The tap needs `CAP_NET_RAW` in the node. It is a help and not a condition: where it cannot be opened (or its file cannot be made) the receiver says why in the `wire_error` of its `ready` line and goes on recording, and the run reports the damaged packets as counted and not named. `received` has `wire_packets`, the number of packets the tap wrote, when it ran. The start of a stream waits for `ready`, so the tap is open before the first packet leaves.

## Barriers: waiting for the run at a packet (`--barriers`, D-038, D-062)

`send --barriers 12:0,40:1` makes the sender stop at two points of its stream: before packet 12 for the thing the run calls 0, and before packet 40 for 1. Right after it has sent the packet before (for packet 0 at the start) it prints a line `{"event":"barrier","flow":1,"id":0,"packet":12}`, and at the slot of packet 12, when it would send it, it goes on only if it has read the line `go 0` on its standard input; if the line is not there yet it waits for it. The run does its thing (a change of the link) between the two lines. The packets after the late one keep their absolute schedule: those that are due are sent at once, in the right order.

The sender never waits for ever. The end of its standard input answers every barrier (a run that died does not leave a sender behind), and an answer that does not come in 10 seconds is given up: the sender goes on, counts it in `barrier_timeouts` of its summary, and the run fails on it, because the guarantee does not hold for that barrier. The summary lists the barriers (`barriers`: `id`, `packet`, `asked_ns`, `held_ns`), so the run can say how long each packet waited. A barrier at a packet beyond the end of the stream is never asked.

## How the stream is timed

Packet `n` leaves at `start + n × size × 8 / rate`, computed in integers from `n` and not by adding an interval, so an interval that is not a whole number of nanoseconds (3 mbit with 1000 byte packets: 2.666… ms) does not drift. The sender sleeps until shortly before the moment and spins for the last 150 microseconds, because a sleep wakes up tens of microseconds late. **A sending agent therefore uses one core fully while it runs.** `max_lateness_ns` in the summary is the largest amount by which a packet left after its moment: it tells how much of a gap in a result comes from the machine and not from the network.

## The schedules of Poisson, on/off and bursty traffic (WRK-02, D-071)

`--pattern` picks the moments at which the packets leave; `cbr` is the default and needs `--rate`. The others need what the table says; the numbers are nanoseconds and bits per second.

| `--pattern` | Needs | The moments |
| --- | --- | --- |
| `cbr` | `--rate` | Packet `n` at `n × size × 8 / rate`. |
| `poisson` | `--rate`, `--seed` | Packet `n` at the floor of `S_n`, where `S_n` is the sum of `n + 1` exponential gaps with the mean `size × 8 × 10^9 / rate` ns, each drawn from the generator. |
| `onoff` | `--rate`, `--on-ns`, `--off-ns`, `--seed` (if drawn) | Phase `k` of the on phases starts at the floor of `P_k`, where `P_0 = 0` and `P_{k+1} = P_k + on_k + off_k`. In it packet `i` leaves at `floor(P_k) + i × size × 8 × 10^9 / rate` (integer division) while that is less than `on_k`. |
| `bursty` | `--burst-packets`, `--gap-ns`, `--pause-ns`, `--seed` (if drawn) | Burst `k` starts at the floor of `B_k`, where `B_0 = 0` and `B_{k+1} = B_k + (n - 1) × gap + pause_k`. Packet `i` of it leaves at `floor(B_k) + i × gap`. |

`--pattern trace` (WRK-03, D-085) needs `--trace <file>` and no `--rate` or `--size`: packet `n` leaves at the `n`th moment of the file and has the `n`th size. The file is text: a first line `# netscenario trace v1: offset in nanoseconds, payload in bytes`, then one line `<offset> <size>` for each packet (at most 100,000, the offsets not falling, a size from the smallest payload to the largest datagram). The run writes it as `flow-<n>.trace` in the directory of the result, from the packets in the scenario. A size below 16 bytes (20 without the checksum) or above the largest UDP payload is refused before a packet leaves.

`--distribution fixed` (the default) uses `--on-ns`, `--off-ns` and `--pause-ns` as they are; with `exponential` each length is drawn from an exponential distribution with that mean. The order of the draws is part of the definition: for `onoff`, `on_0`, then `off_0`, `on_1`, `off_1` and so on; for `bursty`, `pause_0`, `pause_1` and so on; for `poisson`, one gap per packet. A moment is the floor of a sum that is kept as a double, so that rounding does not pile up. A moment at or after `--duration-ns` ends the stream.

**The generator** is written out in `netscenario_agent::rng` and is not a dependency whose output could change with a version: SplitMix64 (Steele, Lea and Flood) turns the 64 bit seed into the four words of the state of xoshiro256++ 1.0 (Blackman and Vigna); a uniform number in `(0, 1]` is `((x >> 11) + 1) × 2^-53`; an exponential number with the mean `m` is `-ln(u) × m`. The logarithm is computed in the program from the exponent and the mantissa of the double with a series of additions, multiplications and divisions (`ln` of the standard library may differ in the last bit between systems, and these operations may not), so every machine draws the same numbers. The seed of workload `n` of a run with the seed `s` is the first output of a SplitMix64 started at `first ^ (n × 0xD1B54A32D192ED03)`, where `first` is the first output of a SplitMix64 started at `s` (`n` counts the workloads that send UDP, from 0; `rng::derive_seed`).

The first outputs are pinned, from an implementation in Python that shares nothing with the Rust code: SplitMix64 from 0 gives `0xE220A8397B1DCDAF`, `0x6E789E6AA1B965F4`, `0x06C45D188009454F`, `0xF88BB8A8724C81EC`; xoshiro256++ from the state `1, 2, 3, 4` gives 41943041, 58720359, 3588806011781223, 3591011842654386 (the reference outputs of its authors); from the seed 42 it gives 15021278609987233951, 5881210131331364753, 18149643915985481100; and `derive_seed(1, 0)` is 6791897765849424158. A Poisson sender of 1 mbit with 125 byte packets started with `--seed 42` sends its first packets at 205420, 1348545, 1364782, 1719836, 1951132 and 2481993 ns. Tests hold all of these, and a change that moves one of them changes every result that was made from a seed, so it needs a decision of its own.

A sender walks its schedule once, in order, and keeps no table of it. The run walks it again after the fact (the series, the packet that a step of the timeline waits for), in one pass for all the steps; it gets the same moments because the walk is a function of the schedule and the seed. The packets are numbered in the order of their moments, and a schedule that cannot make a packet (an on phase of zero, a burst of no packets, a burst with no time to the next) makes none and is refused by the sender.

## UDP echo: the round trip of a datagram stream (`--echo-to`, D-086)

`recv --echo-to <addr>` sends every packet that it recorded on to `<addr>`, unchanged, from a socket of its own. A second `recv` bound at that address in the sender's node records the packets that come back: the send time in them is the first one, so the **round trip** of a packet is the receive time of the second record minus its send time, and a packet that was lost on the way there or back has no record there. The echo is an ordinary datagram and so only for a stream with the checksum of UDP; `--udp-checksum off` with `--echo-to` is refused. The summary of the receiver says how many packets it echoed (`echoed`) and how many the system refused (`echo_errors`).

## TCP: bulk transfers and requests (WRK-04, WRK-05, D-086)

The kernel TCP of the machine is the subject: the agents use ordinary sockets, so the congestion control, the windows and the timers are the kernel's. `tcp-serve` in one node waits for connections; `tcp-bulk` and `tcp-request` in another make them. Every connection starts with a **hello** of 32 bytes from the client, so the server needs nothing on its command line about what comes:

| Bytes | Field | Meaning |
| --- | --- | --- |
| 0 to 3 | magic | `NSTP` |
| 4 | version | `1` |
| 5 | mode | `1` bulk, `2` requests |
| 6 to 7 | reserved | zero |
| 8 to 11 | connection | The number of the connection in its workload, from 0 |
| 12 to 15 | request size | Bytes of a request (requests only, at least 1) |
| 16 to 19 | response size | Bytes of a response (requests only, at least 1) |
| 20 to 31 | reserved | zero |

A server answers a hello that it does not read by closing the connection, and counts it as `refused`.

**Bulk.** The client sends its payload in chunks (`--chunk-bytes`, 65,536 by default, at most 1 MiB), each with a header of four bytes that holds its length, and ends with a chunk of length 0. The server counts the payload as it reads it, in intervals of the clock (below), and answers the end with one byte; the client reads it, and that moment is the **completion** of the transfer: the last byte has been read by the application at the far end. Then the client closes. (A half-close would tell the server the end as well, but the socket of the client would be in `TIME_WAIT` by the time the client knew, and the kernel gives no `tcp_info` for such a socket; here the server waits for the client to close first, so the socket is whole when the client takes the last sample, after the answer.) `--bytes` is what each connection sends, `--duration-ns` is how long each sends, from `--start-at-ns` or from the start; with both a connection ends at whichever comes first, and one of them is needed. The chunk in progress at the end is finished. A line `stop` on the standard input (or its end) ends a transfer early: its connection is recorded as failed and the summary says `stopped`.

**Requests.** The client sends a request of `--request-bytes` and reads a response of `--response-bytes`, again and again, on each of `--connections` connections, until `--duration-ns` has passed. Without `--interval-ns` the loop is **closed**: the next request follows the response. With it the loop is **open**: request `n` of the whole workload is due at `n × interval` from the start, whichever connection takes it, and a request that is due while its connection is busy waits, so that the latency counted from the moment it was due includes the wait (a slow server cannot hide behind the clients that waited for it). A request that gets no response in `--timeout-ns` (30 s by default) fails and its connection is not used again.

**Congestion control.** `--congestion <name>` is set on every socket of the client (and, on the server, on the sockets that answer) and read back; a name the machine lacks is a failed connection whose reason names it (`the congestion control 'bbr' cannot be used here: ...`). Without it the socket has what the kernel gives, and the summary says which (`congestion`). The scenario layer, not the agent, decides the default (D-086).

**Samples of `TCP_INFO`.** Every `--info-interval-ns` (100 ms by default; 0 takes none) a thread of the client asks the kernel for the `tcp_info` of each connection and writes the round trip time, the congestion window, the retransmitted segments so far and the bytes acknowledged so far; a last sample is taken at the end of the connection. The agent asks through a `sock_diag` netlink socket, because `getsockopt(TCP_INFO)` has no safe wrapper and the workspace has no `unsafe` (D-025). A kernel without `sock_diag` gives no samples, and the summary says why (`info_error`).

### The stream record file (version 1)

The agents write a file of records of 48 bytes after a header of 16, in network byte order, with every time on `CLOCK_MONOTONIC` in nanoseconds, the clock of the records of the UDP agents. `netscenario_agent::streams` reads and writes it (a test pins the exact bytes, and a fuzz target reads arbitrary input).

| Header bytes | Field | Meaning |
| --- | --- | --- |
| 0 to 7 | magic | `NSSTREAM` |
| 8 to 9 | version | `1` |
| 10 to 11 | record size | `48` |
| 12 to 15 | flow | The number of the workload |

| Record bytes | Field | Meaning |
| --- | --- | --- |
| 0 | kind | 1 progress, 2 info, 3 connection, 4 request |
| 1 | status | 0 ok; for info 1 is the last sample; 2 failed |
| 2 to 3 | reserved | zero |
| 4 to 7 | connection | From 0 |
| 8 to 47 | `t`, `a`, `b`, `c`, `d` | Five numbers of eight bytes, by kind: |

| Kind | Written by | `t` | `a` | `b` | `c` | `d` |
| --- | --- | --- | --- | --- | --- | --- |
| progress | the server | start of the interval (a multiple of `--progress-interval-ns`, 10 ms by default) | payload bytes read in it | reads it took | 0 | 0 |
| info | a client | the moment of the sample | round trip time in microseconds | congestion window in segments | retransmitted segments so far | bytes acknowledged so far (the SYN counts as one: use differences) |
| connection | a client | connect started | established | done (the answer of the server for bulk; the close for requests) | payload bytes sent | bytes received (requests) |
| request | a client | due | sent | last byte of the response read | request bytes | response bytes (0 if it failed) |

The records are in the order they were written, not in the order of their times. A file that ends inside a record, holds a kind this version does not have, or is of another version is an error when it is read and not a shorter file: a record that is skipped is a number that is missing from a result. The server writes a progress record only for an interval in which a connection read something; a series by time bin is made by the run from these, with the resolution of the interval.

### What the agents say

```json
{"event":"ready","flow":6,"address":"10.100.0.2:40000"}
{"event":"served","flow":6,"connections":3,"refused":0,"bytes_received":4006000,"requests":30,"first_receive_ns":120,"last_receive_ns":456}
{"event":"tcp","flow":6,"connections":2,"failed":0,"bytes_sent":4000000,"bytes_received":0,"requests":0,"requests_failed":0,"congestion":"cubic","first_error":null,"info_error":null,"first_ns":100,"last_ns":900,"stopped":false}
```

`tcp-serve` says `ready` when it listens (the clients start after it), runs until it reads `stop` or its input ends (or until `--max-time-ns`), and then says `served`. A connection that is still open when the stop comes is given half a second without data to finish. The clients say `tcp` when they are done; a failure of a connection is in the summary and in its record, and the exit code is 0 unless the command line was wrong (2) or the file could not be written (1).

## Not in this version

- **Static binary.** D-008 spoke of a static (musl) binary. In the Kernel backend the nodes share the file system of the host and need none; the static build belongs to the release (REL-01) and to backends where the node has its own file system.
- Hardware or kernel timestamps (`SO_TIMESTAMPNS`): the receive time is taken in user space after the read. At the measured rates the error is a few microseconds (S8).
- The UDP echo (`recv --echo-to`) is an option of the agent; a workload kind that uses it, with the round trip of a datagram stream in its result, is not in the scenario format yet.

## How a run starts the agent

The Kernel backend starts the agent as any program of a node (`Backend::start_program`): in the network namespace of the node, with its standard output, standard error and standard input connected to the supervisor. The caller then asks what the program did (`poll_program`: new lines, and how it ended) and writes to it (`send_line`, for `stop`). The order of a stream, as the test `tests/agent.rs` in the Kernel backend runs it: start the receiver, wait for its `ready` line, start the sender, wait until it ends, wait for the packets in flight, send `stop` to the receiver, wait until it ends, read the record file. The files are written by the agent into a directory of the run, which the nodes share with the host.

**Memory.** A program can be started with a limit on its address space (`Program::memory_limit`, set with `prlimit --as` before the program starts). A program that needs more fails to allocate and ends; it does not take the machine down (SAF-02). A test starts a program that needs 150 MB with a limit of 64 MiB and shows that it does not get it.

