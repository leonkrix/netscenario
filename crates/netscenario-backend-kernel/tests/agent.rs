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
) -> (SendSummary, ReceiveSummary, records::File) {
    let agent = agent_exe().expect("checked by the caller");
    let dir = work_dir(label);
    let file = dir.join("flow-1.records");
    let mut backend = backend(|_| {});
    let built = provision(&mut backend, &scenario("", link)).unwrap();
    let (client, server) = (built.nodes[0].1, built.nodes[1].1);
    let limit = 1 << 30;
    let argv = |args: &[&str]| {
        let mut argv = vec![agent.display().to_string()];
        argv.extend(args.iter().map(|a| (*a).to_owned()));
        Program::new(argv).with_memory_limit(limit)
    };

    let receiver = backend
        .start_program(
            server,
            &argv(&[
                "recv",
                "--bind",
                "10.100.0.2:5000",
                "--flow",
                "1",
                "--out",
                file.to_str().unwrap(),
            ]),
        )
        .unwrap();
    // The sender starts when the receiver is ready
    let ready = backend
        .poll_program(receiver, Duration::from_secs(10))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Event>(&ready.lines[0]).unwrap(),
        Event::Ready {
            flow: 1,
            address: "10.100.0.2:5000".to_owned()
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
    let _ = std::fs::remove_dir_all(dir);
    (sent, heard, records)
}

#[test]
fn the_agent_measures_the_delay_of_the_link() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    // 2 mbit with 1200 byte payloads is 208.3 packets per second: 209 in one second
    let (sent, received, file) = stream(
        "      forward: { delay: 20ms }\n",
        "2000000",
        "1200",
        "1000000000",
        "agent-delay",
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
    // first packets do not wait for ARP because the provisioner sets the neighbours.
    assert!(
        (20.0..20.5).contains(&median),
        "the median delay is {median} ms"
    );
    assert!(
        delays[0] >= 20.0,
        "a packet was faster than the link allows: {} ms",
        delays[0]
    );
    assert!(
        *delays.last().unwrap() < 30.0,
        "the slowest packet took {} ms",
        delays.last().unwrap()
    );
    // The machine kept the schedule of the sender: it says by how much it did not
    assert!(
        sent.max_lateness_ns < 20_000_000,
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
    let (_, received, file) = stream(
        "      reverse: { delay: 50ms }\n",
        "1000000",
        "500",
        "500000000",
        "agent-reverse",
    );
    assert_eq!(received.packets, 125);
    let slowest = file
        .records
        .iter()
        .map(records::Record::delay_ns)
        .max()
        .unwrap();
    assert!(
        slowest < 5_000_000,
        "the stream took {slowest} ns although only the way back is slow"
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
