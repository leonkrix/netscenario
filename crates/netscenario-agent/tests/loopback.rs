//! The sender and the receiver together over the loopback interface of the machine, without namespaces:
//! what the sender says it sent is what the receiver recorded, in the library and as real programs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::events::Event;
use netscenario_agent::receiver::{ReceiveConfig, Receiver};
use netscenario_agent::records;
use netscenario_agent::schedule::Stream;
use netscenario_agent::sender::{SendConfig, run};

fn receiver() -> Receiver {
    Receiver::bind(&ReceiveConfig {
        bind: ([127, 0, 0, 1], 0).into(),
        flow: 4,
        max_time_ns: Some(30_000_000_000),
    })
    .unwrap()
}

#[test]
fn what_the_sender_sent_is_what_the_receiver_recorded() {
    let receiver = receiver();
    let target = receiver.local_addr().unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let recording = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut file = Vec::new();
            let summary = receiver.run(&stop, &mut file).unwrap();
            (summary, file)
        })
    };
    // 1 mbit with 1000 byte packets is 125 packets per second: 100 packets in 0.8 s
    let sent = run(&SendConfig {
        target,
        flow: 4,
        stream: Stream {
            bits_per_second: 1_000_000,
            packet_size: 1000,
        },
        duration_ns: 800_000_000,
        start_at_ns: None,
    })
    .unwrap();
    stop.store(true, Ordering::SeqCst);
    let (seen, file) = recording.join().unwrap();

    assert_eq!(sent.packets, 100);
    assert_eq!((sent.errors, sent.flow), (0, 4));
    assert_eq!(seen.packets, 100, "nothing is lost on the loopback");
    assert_eq!(seen.foreign, 0);

    let file = records::read(&file[..]).unwrap();
    assert_eq!(file.flow, 4);
    let sequences: Vec<u64> = file.records.iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, (0..100).collect::<Vec<u64>>());
    for record in &file.records {
        assert!(record.receive_ns >= record.send_ns, "{record:?}");
        assert!(
            record.delay_ns() < 50_000_000,
            "the loopback took {} ns",
            record.delay_ns()
        );
    }
    // The stream keeps its schedule: packet 99 is due 99 intervals of 8 ms after packet 0 (792 ms). The first
    // packet may leave a little late, so the spread may be a little short.
    let spread = sent.last_send_ns.unwrap() - sent.first_send_ns.unwrap();
    assert!(
        (792_000_000 - 5_000_000..792_000_000 + 100_000_000).contains(&spread),
        "{spread} ns between the first and the last packet"
    );
    assert!(seen.first_receive_ns.is_some());
}

/// A datagram that is not a packet of the agent is counted and not recorded: a stray packet must not be
/// taken for a measurement.
#[test]
fn a_foreign_datagram_is_counted_and_not_recorded() {
    let receiver = receiver();
    let target = receiver.local_addr().unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let recording = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut file = Vec::new();
            let summary = receiver.run(&stop, &mut file).unwrap();
            (summary, file)
        })
    };
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .send_to(b"hello, this is not an agent", target)
        .unwrap();
    socket.send_to(b"short", target).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    stop.store(true, Ordering::SeqCst);
    let (seen, file) = recording.join().unwrap();
    assert_eq!((seen.packets, seen.foreign), (0, 2));
    assert_eq!(records::read(&file[..]).unwrap().records.len(), 0);
}

#[test]
fn a_sender_starts_at_the_moment_it_is_given() {
    let receiver = receiver();
    let target = receiver.local_addr().unwrap();
    drop(receiver);
    let start = monotonic_ns() + 300_000_000;
    let sent = run(&SendConfig {
        target,
        flow: 0,
        stream: Stream {
            bits_per_second: 800_000,
            packet_size: 100,
        },
        duration_ns: 10_000_000,
        start_at_ns: Some(start),
    })
    .unwrap();
    assert!(sent.first_send_ns.unwrap() >= start);
    assert!(sent.first_send_ns.unwrap() < start + 50_000_000);
}

#[test]
fn the_receiver_stops_by_itself_when_its_time_is_up() {
    let receiver = Receiver::bind(&ReceiveConfig {
        bind: ([127, 0, 0, 1], 0).into(),
        flow: 0,
        max_time_ns: Some(200_000_000),
    })
    .unwrap();
    let began = Instant::now();
    let summary = receiver.run(&AtomicBool::new(false), Vec::new()).unwrap();
    assert_eq!(summary.packets, 0);
    assert!(began.elapsed() >= Duration::from_millis(200));
    assert!(began.elapsed() < Duration::from_secs(3));
}

/// The two programs as a supervisor starts them: `recv` says that it is ready and where, `send` runs, a line
/// `stop` ends `recv`, and both say what they did.
#[test]
fn the_programs_work_together_and_speak_the_documented_lines() {
    let agent = env!("CARGO_BIN_EXE_netscenario-agent");
    let dir = std::env::temp_dir().join(format!("netscenario-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("flow-1.records");
    let mut recv = Command::new(agent)
        .args(["recv", "--bind", "127.0.0.1:0", "--flow", "1", "--out"])
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(recv.stdout.take().unwrap()).lines();
    let Event::Ready { flow, address } =
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    else {
        panic!("the first line of recv must be ready");
    };
    assert_eq!(flow, 1);

    let sent = Command::new(agent)
        .args([
            "send", "--to", &address, "--rate", "1000000", "--size", "500",
        ])
        .args(["--duration-ns", "400000000", "--flow", "1"])
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let Event::Sent(sent) = serde_json::from_slice(&sent.stdout).unwrap() else {
        panic!("send must say sent");
    };
    // 1 mbit with 500 byte packets is 250 packets per second: 100 in 0.4 s
    assert_eq!(sent.packets, 100);

    writeln!(recv.stdin.take().unwrap(), "stop").unwrap();
    let Event::Received(received) = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    else {
        panic!("recv must say received");
    };
    assert!(recv.wait().unwrap().success());
    assert_eq!(received.packets, 100);

    let records = records::read(std::fs::File::open(&file).unwrap()).unwrap();
    assert_eq!(records.flow, 1);
    assert_eq!(records.records.len(), 100);
    let _ = std::fs::remove_dir_all(dir);
}

/// The end of the standard input is a stop too: a receiver whose supervisor died does not run for ever.
#[test]
fn a_closed_standard_input_stops_the_receiver() {
    let agent = env!("CARGO_BIN_EXE_netscenario-agent");
    let dir = std::env::temp_dir().join(format!("netscenario-agent-eof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut recv = Command::new(agent)
        .args(["recv", "--bind", "127.0.0.1:0", "--out"])
        .arg(dir.join("x.records"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(recv.stdout.take().unwrap()).lines();
    let _ready = lines.next().unwrap().unwrap();
    drop(recv.stdin.take());
    let began = Instant::now();
    let last = lines.next().unwrap().unwrap();
    assert!(last.contains(r#""event":"received""#), "{last}");
    assert!(recv.wait().unwrap().success());
    assert!(began.elapsed() < Duration::from_secs(3));
    let _ = std::fs::remove_dir_all(dir);
}
