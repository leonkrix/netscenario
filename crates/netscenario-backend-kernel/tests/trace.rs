//! The `trace-replay` workload in a real run (WRK-03, D-085): the packets leave at the moments of the trace and have its sizes on
//! the wire, the result counts the bytes that arrived, and a window cuts the trace. The link has no impairment, so these run
//! wherever the namespaces do.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

mod support;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use netscenario_agent::pcap::{self, Frame};
use netscenario_agent::records;
use netscenario_core::{RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

/// 150 packets: one every 10 ms, two of them at the same moment, with sizes from the smallest payload to the largest that fits.
fn trace() -> (Vec<u64>, Vec<u64>) {
    const SIZES: [u64; 6] = [16, 60, 160, 1200, 1472, 300];
    let mut moments = Vec::new();
    let mut sizes = Vec::new();
    for n in 0..150_u64 {
        moments.push(n * 10_000_000);
        sizes.push(SIZES[n as usize % SIZES.len()]);
    }
    // The 41st packet leaves with the 40th
    moments[40] = moments[39];
    (moments, sizes)
}

fn csv(moments: &[u64], sizes: &[u64]) -> Vec<u8> {
    let mut text = String::from("time,size\n");
    for (moment, size) in moments.iter().zip(sizes) {
        let _ = writeln!(text, "{moment}ns,{size}b");
    }
    text.into_bytes()
}

fn scenario(extra: &str, moments: &[u64], sizes: &[u64]) -> Scenario {
    let mut scenario = Scenario::from_yaml(
        &format!("version: 0\nname: trace\nseed: 3\nduration: 3s\nmeasure:\n  capture: [access]\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\nworkloads:\n  - {{ id: call, kind: trace-replay, from: client, to: server, trace: {{ file: call.csv }}{extra} }}\n"),
        "trace.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let bytes = csv(moments, sizes);
    assert_eq!(scenario.resolve_files(|_| Ok(bytes.clone())), Vec::new());
    scenario
}

fn run_it(name: &str, scenario: &Scenario) -> (RunResult, std::path::PathBuf) {
    let dir = work_dir(name);
    let mut backend = backend(|_| {});
    let mut options = netscenario_core::RunOptions::new(agent_exe().unwrap(), &dir);
    options.settle = Some(Duration::from_secs(1));
    let result = run(&mut backend, scenario, &options).unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    (result, dir)
}

fn ready() -> bool {
    namespaces_available() && agent_exe().is_some()
}

/// The payload sizes of the packets of the stream in a capture file, in the order they were captured.
fn sizes_on_the_wire(dir: &Path, file: &str) -> Vec<u64> {
    let frames: Vec<Frame> = pcap::Reader::new(std::io::BufReader::new(
        std::fs::File::open(dir.join(file)).unwrap(),
    ))
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    frames
        .iter()
        .filter(|frame| {
            frame.bytes.len() >= 14 + 20 + 8
                && frame.bytes[12..14] == [0x08, 0x00]
                && frame.bytes[23] == 17
                && u16::from_be_bytes([frame.bytes[36], frame.bytes[37]]) == 5000
        })
        .map(|frame| frame.bytes.len() as u64 - 14 - 20 - 8)
        .collect()
}

/// The packets of the trace are on the wire with the sizes of the trace, in its order; none is lost or added; the bytes that
/// arrived are the sum of the sizes; and the result says what the trace was. A trace of one size would be a stream, so the
/// sizes on the wire must be more than one.
#[test]
fn a_trace_is_sent_with_its_sizes_and_the_result_counts_its_bytes() {
    if !ready() {
        return;
    }
    let (moments, sizes) = trace();
    let (result, dir) = run_it("trace-sizes", &scenario("", &moments, &sizes));
    let call = &result.workloads[0];
    assert_eq!(call.kind, "trace-replay");
    assert_eq!(
        (call.stats.sent, call.stats.received, call.stats.lost),
        (150, 150, 0)
    );
    assert_eq!(call.sender.send_errors, 0);
    assert_eq!(call.stats.received_bytes, sizes.iter().sum::<u64>());
    assert_eq!(call.payload_received_bytes(), sizes.iter().sum::<u64>());
    assert_eq!(call.frames_per_packet, 1);
    let trace = call.trace.as_ref().unwrap();
    assert_eq!(
        (
            trace.packets,
            trace.padded,
            trace.smallest_bytes,
            trace.largest_bytes
        ),
        (150, 0, 16, 1472)
    );
    assert_eq!(trace.span_ns, 1_490_000_000);
    assert_eq!(trace.sha256.as_ref().map(String::len), Some(64));
    // What the interface of the sender put on the wire
    let capture = result
        .captures
        .iter()
        .find(|capture| capture.node == "client")
        .unwrap();
    let on_the_wire = sizes_on_the_wire(&dir, &capture.file);
    assert_eq!(on_the_wire, sizes);
    let distinct: std::collections::BTreeSet<u64> = on_the_wire.iter().copied().collect();
    assert!(distinct.len() > 1, "{distinct:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A packet never leaves before its moment, and none leaves later than the sender says: the gaps between the packets that were
/// sent are the gaps of the trace, within the lateness that the sender measured. The packets at the same moment leave one after
/// the other.
#[test]
fn the_packets_leave_at_the_moments_of_the_trace() {
    if !ready() {
        return;
    }
    let (moments, sizes) = trace();
    let (result, dir) = run_it("trace-moments", &scenario("", &moments, &sizes));
    let call = &result.workloads[0];
    let file = records::read(std::fs::File::open(dir.join(&call.records_file)).unwrap()).unwrap();
    let mut sent: Vec<(u64, u64)> = file
        .records
        .iter()
        .map(|record| (record.sequence, record.send_ns))
        .collect();
    sent.sort_unstable();
    assert_eq!(sent.len(), 150);
    let late = call.sender.max_lateness_ns;
    let first = sent[0].1;
    for (sequence, send_ns) in &sent {
        let gap = send_ns - first;
        let planned = moments[*sequence as usize] - moments[0];
        // Not before its moment (less the lateness of the first packet), not after it (plus the lateness of this one)
        assert!(
            gap + late >= planned && gap <= planned + late,
            "packet {sequence}: left {gap} ns after the first, the trace says {planned} ns, the sender was late by at most {late} ns"
        );
    }
    // The packets with the same moment keep the order of the trace
    assert!(sent[40].1 >= sent[39].1);
    let _ = std::fs::remove_dir_all(dir);
}

/// The window of the workload cuts the trace like it cuts any schedule: the packets from `stop` on are not sent. Canary: without
/// the window the same trace sends them all.
#[test]
fn a_window_cuts_the_trace() {
    if !ready() {
        return;
    }
    let (moments, sizes) = trace();
    let (result, dir) = run_it("trace-window", &scenario(", stop: 500ms", &moments, &sizes));
    let call = &result.workloads[0];
    // Packets 0 to 49 leave before 500 ms
    assert_eq!(call.stats.sent, 50);
    assert_eq!(call.stats.received, 50);
    assert_eq!(call.stats.received_bytes, sizes[..50].iter().sum::<u64>());
    let _ = std::fs::remove_dir_all(dir);
    let (whole, dir) = run_it("trace-window-whole", &scenario("", &moments, &sizes));
    assert_eq!(whole.workloads[0].stats.sent, 150);
    let _ = std::fs::remove_dir_all(dir);
}

/// A capture of a run is a trace of the next one: the sizes that the interface saw are the sizes of the packets that were sent, in
/// the same order, and the scenario made from the capture sends them again. The moments come from the timestamps of the capture, so
/// only their order and the first one (0) are exact.
#[test]
fn the_capture_of_a_run_replays_as_a_trace() {
    if !ready() {
        return;
    }
    let (moments, sizes) = trace();
    let (first, dir) = run_it("trace-capture", &scenario("", &moments, &sizes));
    let capture = first
        .captures
        .iter()
        .find(|capture| capture.node == "client")
        .unwrap();
    let file = std::fs::read(dir.join(&capture.file)).unwrap();
    // The capture holds the packets of one flow (the stream), so no selection is needed
    let mut again = Scenario::from_yaml(
        "version: 0\nname: again\nseed: 3\nduration: 3s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\nworkloads:\n  - { id: call, kind: trace-replay, from: client, to: server, trace: { file: first.pcap } }\n",
        "again.yaml",
    )
    .unwrap();
    assert_eq!(again.resolve_files(|_| Ok(file.clone())), Vec::new());
    let (result, second_dir) = run_it("trace-capture-again", &again);
    let call = &result.workloads[0];
    let trace = call.trace.as_ref().unwrap();
    assert_eq!((trace.packets, trace.padded), (150, 0));
    assert_eq!(call.stats.received_bytes, sizes.iter().sum::<u64>());
    assert_eq!((call.stats.sent, call.stats.received), (150, 150));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(second_dir);
}
