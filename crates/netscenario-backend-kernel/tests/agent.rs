//! The agent in the nodes of a real run: the receiver listens in one namespace, the sender sends from the other
//! through a link with a delay, and the records say what the network did (WRK-01, PRB-01). The sequence of
//! calls is the one a run makes: start the receiver, wait for `ready`, start the sender, wait for it to end,
//! stop the receiver, read the records.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Delays of a few milliseconds are far below the precision of a double
    clippy::cast_precision_loss
)]

mod support;

use std::time::{Duration, Instant};

use netscenario_agent::events::{Event, ReceiveSummary, SendSummary};
use netscenario_agent::records;
use netscenario_backend_kernel::KernelBackend;
use netscenario_core::{Backend, Program, ProgramHandle, provision};
use support::{agent_exe, backend, namespaces_available, scenario, work_dir};

/// Polls until the program has ended and returns the events it printed.
fn events_until_exit(backend: &mut KernelBackend, program: ProgramHandle) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(200))
            .unwrap();
        events.extend(state.lines.iter().map(|l| serde_json::from_str(l).unwrap()));
        if let Some(exit) = state.exit {
            assert!(exit.success(), "{exit}: {}", state.stderr);
            return events;
        }
        assert!(Instant::now() < deadline, "the agent did not end");
    }
}

fn sent(mut events: Vec<Event>) -> SendSummary {
    match events.pop() {
        Some(Event::Sent(summary)) => summary,
        other => panic!("the sender must end with sent, not {other:?}"),
    }
}

fn received(mut events: Vec<Event>) -> ReceiveSummary {
    match events.pop() {
        Some(Event::Received(summary)) => summary,
        other => panic!("the receiver must end with received, not {other:?}"),
    }
}

/// Runs one stream from the client to the server and returns the summaries and the records.
fn stream(
    link: &str,
    rate: &str,
    size: &str,
    duration_ns: &str,
    label: &str,
    extra: &[&str],
    wire: bool,
) -> (
    SendSummary,
    ReceiveSummary,
    records::File,
    Option<records::File>,
) {
    let agent = agent_exe().expect("checked by the caller");
    let dir = work_dir(label);
    let file = dir.join("flow-1.records");
    let wire_file = dir.join("flow-1.wire");
    let mut backend = backend(|_| {});
    let built = provision(&mut backend, &scenario("", link)).unwrap();
    let (client, server) = (built.nodes[0].1, built.nodes[1].1);
    let limit = 1 << 30;
    // `extra` goes to both programs: the checksum of UDP is the same at both ends
    let argv = |args: &[&str]| {
        let mut argv = vec![agent.display().to_string()];
        argv.extend(args.iter().chain(extra).map(|a| (*a).to_owned()));
        Program::new(argv).with_memory_limit(limit)
    };

    let mut receive_arguments = vec![
        "recv",
        "--bind",
        "10.100.0.2:5000",
        "--flow",
        "1",
        "--out",
        file.to_str().unwrap(),
    ];
    if wire {
        receive_arguments.extend(["--wire-out", wire_file.to_str().unwrap()]);
    }
    let receiver = backend
        .start_program(server, &argv(&receive_arguments))
        .unwrap();
    // The sender starts when the receiver is ready
    let ready = backend
        .poll_program(receiver, Duration::from_secs(10))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Event>(&ready.lines[0]).unwrap(),
        Event::Ready {
            flow: 1,
            address: "10.100.0.2:5000".to_owned(),
            wire_error: None
        },
        "{}",
        ready.stderr
    );
    let sender = backend
        .start_program(
            client,
            &argv(&[
                "send",
                "--to",
                "10.100.0.2:5000",
                "--rate",
                rate,
                "--size",
                size,
                "--duration-ns",
                duration_ns,
                "--flow",
                "1",
            ]),
        )
        .unwrap();
    let sent = sent(events_until_exit(&mut backend, sender));
    // The packets still in flight arrive within the delay of the link; then the receiver is told to stop
    std::thread::sleep(Duration::from_millis(300));
    backend.send_line(receiver, "stop").unwrap();
    let heard = received(events_until_exit(&mut backend, receiver));
    backend.teardown().unwrap();
    let records = records::read(std::fs::File::open(&file).unwrap()).unwrap();
    let wire = wire.then(|| records::read(std::fs::File::open(&wire_file).unwrap()).unwrap());
    let _ = std::fs::remove_dir_all(dir);
    (sent, heard, records, wire)
}

#[test]
fn the_agent_measures_the_delay_of_the_link() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    // 2 mbit with 1200 byte payloads is 208.3 packets per second: 209 in one second
    let (sent, received, file, _) = stream(
        "      forward: { delay: 20ms }\n",
        "2000000",
        "1200",
        "1000000000",
        "agent-delay",
        &[],
        false,
    );
    assert_eq!(sent.packets, 209);
    assert_eq!((sent.errors, received.foreign), (0, 0));
    assert_eq!(
        received.packets, 209,
        "nothing is lost on a link without loss"
    );
    assert_eq!(file.flow, 1);
    let sequences: Vec<u64> = file.records.iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, (0..209).collect::<Vec<u64>>());

    let mut delays: Vec<f64> = file
        .records
        .iter()
        .map(|r| r.delay_ns() as f64 / 1e6)
        .collect();
    delays.sort_by(f64::total_cmp);
    let median = delays[delays.len() / 2];
    // 20 ms from `netem` and the path of the packet, which is a few tens of microseconds (spike S4). The
    // upper bound leaves room for a busy shared machine; the exact deviations are for the fidelity tests
    // (QA-06). The lower bound is physics and never moves.
    assert!(
        (20.0..22.0).contains(&median),
        "the median delay is {median} ms"
    );
    assert!(
        delays[0] >= 20.0,
        "a packet was faster than the link allows: {} ms",
        delays[0]
    );
    // One packet may be held up by the machine for some milliseconds, but not by a hundred
    assert!(
        *delays.last().unwrap() < 120.0,
        "the slowest packet took {} ms",
        delays.last().unwrap()
    );
    // The first packets do not wait for ARP because the provisioner sets the neighbours. An ARP request is
    // held back by the link as well, which would give the first four packets 25 to 40 ms; one disturbed
    // packet cannot move the middle of four.
    let mut first_four: Vec<f64> = file
        .records
        .iter()
        .take(4)
        .map(|r| r.delay_ns() as f64 / 1e6)
        .collect();
    first_four.sort_by(f64::total_cmp);
    assert!(
        first_four[2] < 25.0,
        "the first packets took {first_four:?} ms: did they wait for ARP?"
    );
    // The machine kept the schedule of the sender: it says by how much it did not
    assert!(
        sent.max_lateness_ns < 100_000_000,
        "the sender was {} ns late",
        sent.max_lateness_ns
    );
}

/// The direction matters: the delay is only in the direction to the server, and the stream goes that way.
#[test]
fn a_delay_in_the_other_direction_does_not_touch_the_stream() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (_, received, file, _) = stream(
        "      reverse: { delay: 50ms }\n",
        "1000000",
        "500",
        "500000000",
        "agent-reverse",
        &[],
        false,
    );
    assert_eq!(received.packets, 125);
    let mut delays: Vec<u64> = file.records.iter().map(records::Record::delay_ns).collect();
    delays.sort_unstable();
    let median = delays[delays.len() / 2];
    // The way back takes 50 ms; a stream that was held back by it would have a median of 50 ms
    assert!(
        median < 10_000_000,
        "the stream took {median} ns although only the way back is slow"
    );
}

/// The loss that the scenario asks for shows in the records as missing sequence numbers (PRB-02), and the same
/// seed loses the same packets.
#[test]
fn the_loss_of_the_link_shows_as_missing_sequence_numbers() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let lost = |label: &str| -> Vec<u64> {
        let agent = agent_exe().unwrap();
        let dir = work_dir(label);
        let file = dir.join("flow-1.records");
        let mut backend = backend(|_| {});
        let built = provision(
            &mut backend,
            &support::scenario(
                "seed: 99\n",
                "      forward: { loss: { model: random, p: 20% } }\n",
            ),
        )
        .unwrap();
        let run = |args: Vec<String>| {
            let mut argv = vec![agent.display().to_string()];
            argv.extend(args);
            Program::new(argv)
        };
        let receiver = backend
            .start_program(
                built.nodes[1].1,
                &run(vec![
                    "recv".into(),
                    "--bind".into(),
                    "10.100.0.2:5000".into(),
                    "--out".into(),
                    file.display().to_string(),
                ]),
            )
            .unwrap();
        let ready = backend
            .poll_program(receiver, Duration::from_secs(10))
            .unwrap();
        assert_eq!(ready.lines.len(), 1, "{}", ready.stderr);
        let sender = backend
            .start_program(
                built.nodes[0].1,
                &run(
                    "send --to 10.100.0.2:5000 --rate 1000000 --size 500 --duration-ns 1000000000"
                        .split(' ')
                        .map(String::from)
                        .collect(),
                ),
            )
            .unwrap();
        let summary = sent(events_until_exit(&mut backend, sender));
        std::thread::sleep(Duration::from_millis(200));
        backend.send_line(receiver, "stop").unwrap();
        events_until_exit(&mut backend, receiver);
        backend.teardown().unwrap();
        let arrived: Vec<u64> = records::read(std::fs::File::open(&file).unwrap())
            .unwrap()
            .records
            .iter()
            .map(|r| r.sequence)
            .collect();
        let _ = std::fs::remove_dir_all(dir);
        (0..summary.packets)
            .filter(|n| !arrived.contains(n))
            .collect()
    };
    let first = lost("agent-loss-1");
    let second = lost("agent-loss-2");
    // 250 packets at 20 percent: about 50 lost, far from 0 and from all
    assert!(
        (20..90).contains(&first.len()),
        "{} of 250 packets were lost",
        first.len()
    );
    assert_eq!(first, second, "the same seed must lose the same packets");
}

/// A stream without the checksum of UDP is sent through a raw socket, and the receiver checks the CRC-32 that
/// takes the place of the checksum. On a link without impairment every packet arrives whole.
#[test]
fn a_stream_without_the_checksum_arrives_whole_on_a_clean_link() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (sent, received, file, _) = stream(
        "      forward: { delay: 5ms }\n",
        "2000000",
        "1200",
        "1000000000",
        "agent-no-checksum",
        &["--udp-checksum", "off"],
        false,
    );
    assert_eq!(
        (sent.packets, sent.errors),
        (209, 0),
        "{:?}",
        sent.first_error
    );
    assert_eq!(
        (received.packets, received.corrupted, received.foreign),
        (209, 0, 0)
    );
    let sequences: Vec<u64> = file.records.iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, (0..209).collect::<Vec<u64>>());
    // The raw socket sends at the same moments as the socket of UDP: the delay is the one of the link
    let median = {
        let mut delays: Vec<u64> = file.records.iter().map(records::Record::delay_ns).collect();
        delays.sort_unstable();
        delays[delays.len() / 2]
    };
    assert!(
        (5_000_000..8_000_000).contains(&median),
        "the median delay is {median} ns"
    );
}

/// With the link flipping bits, the stream without the checksum is the one that sees damaged packets arrive:
/// the kernel does not drop them, the CRC-32 finds them. The same link with the checksum on is the canary: the
/// kernel drops the damaged packets, so none arrives damaged, and fewer arrive at all.
#[test]
fn a_corrupted_packet_arrives_without_the_checksum_and_is_dropped_with_it() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let link = "      forward: { corrupt: 10% }\n";
    let (sent, off, _, _) = stream(
        link,
        "2000000",
        "200",
        "2000000000",
        "agent-corrupt-off",
        &["--udp-checksum", "off"],
        false,
    );
    let (_, on, _, _) = stream(
        link,
        "2000000",
        "200",
        "2000000000",
        "agent-corrupt-on",
        &[],
        false,
    );
    // 2 mbit with 200 byte payloads is 1,250 packets per second: 2,500 in two seconds
    assert_eq!(sent.packets, 2500);
    assert_eq!((on.corrupted, on.foreign), (0, 0), "the kernel drops them");
    // 10 % are corrupted, 250 of 2,500; with 200 payload bytes in a frame of 242, 83 % of the flips land in the
    // payload and arrive: 206 with a standard deviation of 14, so the bounds are four deviations. The exact
    // figures are for the fidelity cases; this only tells the two streams apart.
    assert!(
        (150..=265).contains(&off.corrupted),
        "{} packets arrived damaged",
        off.corrupted
    );
    assert_eq!(
        off.foreign, 0,
        "no datagram is called foreign without the checksum"
    );
    assert!(
        off.packets + off.corrupted <= sent.packets,
        "{} + {} of {}",
        off.packets,
        off.corrupted,
        sent.packets
    );
    // With the checksum the damaged packets are gone, so fewer arrive than without it, where they are counted
    assert!(
        on.packets < off.packets + off.corrupted,
        "{} arrived with the checksum, {} intact and {} damaged without it",
        on.packets,
        off.packets,
        off.corrupted
    );
}

/// The tap in the receiving node sees the packets that the node then drops as damaged, and with their sequence
/// numbers. The link here only corrupts (10 %), so every packet that was not delivered was damaged, and every
/// sequence that the tap saw and the socket did not get is one of them: the names are a subset of what the
/// sender sent, none was delivered, and most of the damaged packets are named.
#[test]
fn the_tap_names_the_packets_that_the_node_dropped_as_damaged() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (sent, received, file, wire) = stream(
        "      forward: { corrupt: 10% }\n",
        "2000000",
        "200",
        "2000000000",
        "agent-tap",
        &[],
        true,
    );
    let wire = wire.unwrap();
    assert_eq!(sent.packets, 2500);
    assert_eq!(received.wire_packets, Some(wire.records.len() as u64));
    let delivered: std::collections::BTreeSet<u64> =
        file.records.iter().map(|r| r.sequence).collect();
    let seen: std::collections::BTreeSet<u64> = wire.records.iter().map(|r| r.sequence).collect();
    // Everything that was delivered was seen by the tap first (it sees the frame before the stack does)
    assert!(
        delivered.difference(&seen).count() <= 3,
        "{} delivered packets were not seen on the wire (a bit in the sequence number can make one look unseen)",
        delivered.difference(&seen).count()
    );
    // A bit in the sequence number can make a number the sender never used; the run judges the range
    let named: Vec<u64> = seen
        .difference(&delivered)
        .copied()
        .filter(|sequence| *sequence < sent.packets)
        .collect();
    let dropped = sent.packets - received.packets;
    // 10 % of 2,500 are corrupted, 236 of 242 bytes of a frame drop it: 244 dropped, give or take 4 deviations
    assert!(
        (190..=300).contains(&dropped),
        "{dropped} packets were not delivered"
    );
    // The names are a subset of the packets that were not delivered: the link loses nothing
    assert!(
        u64::try_from(named.len()).unwrap() <= dropped,
        "{} names for {dropped} dropped packets",
        named.len()
    );
    // 200 byte payloads: 218 of the 236 bytes that drop a frame leave it readable, 92 %, less the bits that
    // turn a sequence number into another one (4 bytes). Far from all, far from none.
    let share = named.len() as f64 / dropped as f64;
    assert!(
        (0.80..=0.95).contains(&share),
        "{} of {dropped} dropped packets were named ({share:.3})",
        named.len()
    );
}

/// Canary: a link that does not corrupt has nothing to name, and the tap still sees every packet.
#[test]
fn the_tap_names_nothing_on_a_link_that_does_not_corrupt() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (sent, received, file, wire) = stream(
        "      forward: { delay: 5ms }\n",
        "2000000",
        "200",
        "1000000000",
        "agent-tap-clean",
        &[],
        true,
    );
    let wire = wire.unwrap();
    // 2 mbit with 200 byte payloads is 1,250 packets per second
    assert_eq!((sent.packets, received.packets), (1250, 1250));
    assert_eq!(wire.records.len(), 1250, "the tap sees every packet once");
    let delivered: std::collections::BTreeSet<u64> =
        file.records.iter().map(|r| r.sequence).collect();
    assert!(wire.records.iter().all(|r| delivered.contains(&r.sequence)));
}
