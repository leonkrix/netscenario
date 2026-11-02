//! The capture of the interfaces of a link in a real run (PRB-09, D-047, D-073): the files are pcap that other tools read,
//! the frames in them are the frames of the stream (as many as the network delivered, duplicates included), their times
//! are the times of the run, and a capture that lost frames says so.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::time::Duration;

use netscenario_agent::pcap::{self, Frame};
use netscenario_agent::records;
use netscenario_core::{CaptureResult, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario(link: &str, measure: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: capture-test\nseed: 5\nduration: 3s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }}\nmeasure:\n{measure}"
        ),
        "capture-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn go(dir: &Path, scenario: &Scenario) -> RunResult {
    let mut backend = backend(|_| {});
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_secs(1));
    let result = run(&mut backend, scenario, &options).unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    result
}

fn ready() -> bool {
    namespaces_available() && agent_exe().is_some()
}

fn frames_of(dir: &Path, capture: &CaptureResult) -> Vec<Frame> {
    let file = std::fs::File::open(dir.join(&capture.file)).unwrap();
    pcap::Reader::new(std::io::BufReader::new(file))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("{}: {error}", capture.file))
}

/// The frames that are packets of the stream: IPv4, UDP, to the port of the first workload (5000). The wire format puts the
/// sequence number of the agent after the header of UDP and the 4 bytes of magic, version and flags.
fn stream_frames(frames: &[Frame]) -> Vec<&Frame> {
    frames
        .iter()
        .filter(|frame| {
            frame.bytes.len() >= 14 + 20 + 8
                && frame.bytes[12..14] == [0x08, 0x00]
                && frame.bytes[23] == 17
                && u16::from_be_bytes([frame.bytes[36], frame.bytes[37]]) == 5000
        })
        .collect()
}

/// Both ends of a link without loss: two files that `tcpdump` and our reader both read, with the frames of the stream and
/// nothing else, as many as the interface counted, and as many as the workload sent.
#[test]
fn both_ends_of_a_link_are_captured_whole_and_read_by_other_tools() {
    if !ready() {
        return;
    }
    let dir = work_dir("capture-whole");
    let result = go(
        &dir,
        &scenario("      forward: { delay: 10ms }\n", "  capture: [access]\n"),
    );
    let sent = result.workloads[0].stats.sent;
    assert_eq!(result.captures.len(), 2);
    let (client, server) = (&result.captures[0], &result.captures[1]);
    assert_eq!(
        (client.node.as_str(), server.node.as_str()),
        ("client", "server")
    );
    assert_eq!(client.file, "capture-access-client.pcap");
    for capture in &result.captures {
        let frames = frames_of(&dir, capture);
        // The file holds what the interface counted, and says so
        assert_eq!(capture.packets, frames.len() as u64);
        assert_eq!(capture.missing, Some(0), "{capture:?}");
        assert_eq!(capture.interface_frames, Some(capture.packets));
        // The only traffic of this run is the stream: no ARP (the neighbour is fixed), no IPv6
        assert_eq!(stream_frames(&frames).len(), frames.len());
        assert_eq!(frames.len() as u64, sent, "{}", capture.file);
        assert!(capture.receive_buffer_bytes > 0);
        // `tcpdump`, where there is one, counts the same frames
        if let Ok(output) = std::process::Command::new("tcpdump")
            .args(["-nn", "-r"])
            .arg(dir.join(&capture.file))
            .output()
        {
            let lines = String::from_utf8_lossy(&output.stdout).lines().count();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(lines as u64, capture.packets, "{}", capture.file);
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// What a capture shows is what the network delivered: with loss and duplication on the link, the frames at either end are the
/// packets that left the queue, the lost ones are in no file, and the duplicates are in both. The receiver's records say how
/// many arrived, so the files are held to them: frames = sent - lost in the queue + duplicated = arrivals (duplicates included).
#[test]
fn the_files_hold_what_the_network_delivered_with_the_lost_ones_missing_and_the_duplicates_in() {
    if !ready() {
        return;
    }
    let dir = work_dir("capture-lossy");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 10ms, loss: { model: random, p: 10% }, duplicate: 10% }\n",
            "  capture: [access]\n",
        ),
    );
    let stats = &result.workloads[0].stats;
    let arrivals = stats.received + stats.duplicates;
    assert!(stats.lost > 0 && stats.duplicates > 0, "{stats:?}");
    for capture in &result.captures {
        let frames = frames_of(&dir, capture);
        assert_eq!(stream_frames(&frames).len(), frames.len());
        assert_eq!(
            frames.len() as u64,
            arrivals,
            "{} at {}",
            capture.file,
            capture.node
        );
        assert_eq!(capture.missing, Some(0));
    }
    // Canary: the packets that were sent are more than the packets in the file, by what the link dropped less what it copied
    let sent = stats.sent;
    assert!(sent > arrivals - stats.duplicates);
    let _ = std::fs::remove_dir_all(dir);
}

/// The times of the frames are on the clock of the run: never before the sender sent the packet plus the delay of the link
/// (a frame cannot arrive before that), never after the receiver recorded it, and the result's offset turns the times of the
/// file into the times of the records.
#[test]
fn the_timestamps_are_monotonic_inside_the_run_and_agree_with_the_records() {
    if !ready() {
        return;
    }
    let dir = work_dir("capture-times");
    let result = go(
        &dir,
        &scenario("      forward: { delay: 10ms }\n", "  capture: [access]\n"),
    );
    let server = result.captures.iter().find(|c| c.node == "server").unwrap();
    let frames = frames_of(&dir, server);
    assert!(
        frames.windows(2).all(|w| w[0].time_ns <= w[1].time_ns),
        "the times go back"
    );
    // Inside the run: the first frame is after the start and the last before the end plus the wait for the packets
    let (first, last) = (server.first_ns.unwrap(), server.last_ns.unwrap());
    assert!(first > 0, "{first}");
    assert!(last < 3_000_000_000 + 1_000_000_000, "{last}");
    // The records: each packet of the stream, by sequence number
    let file = std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap();
    let records = records::read(std::io::BufReader::new(file))
        .unwrap()
        .records;
    let offset = server.realtime_minus_monotonic_ns;
    let stream = stream_frames(&frames);
    assert_eq!(stream.len(), records.len());
    let mut worst_before_receive = i128::MAX;
    for (frame, record) in stream.iter().zip(&records) {
        let monotonic = i128::from(frame.time_ns) - i128::from(offset);
        // The frame of this packet is the one the record is of: the sequence number is in it (big-endian, after the magic)
        let sequence = u32::from_be_bytes(
            frame.bytes[14 + 20 + 8 + 4..14 + 20 + 8 + 8]
                .try_into()
                .unwrap(),
        );
        assert_eq!(u64::from(sequence), record.sequence);
        // It arrived after the sender sent it and the link held it for 10 ms (a millisecond for the offset), and before the
        // receiver read it
        assert!(
            monotonic + 1_000_000 >= i128::from(record.send_ns) + 10_000_000,
            "seq {sequence}"
        );
        worst_before_receive = worst_before_receive.min(i128::from(record.receive_ns) - monotonic);
    }
    assert!(
        worst_before_receive >= -1_000_000,
        "a frame was stamped after the receiver read it: {worst_before_receive}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A capture that is cut short is not read as a whole one: the reader names it, wherever the cut is.
#[test]
fn canary_a_capture_that_is_cut_short_is_refused() {
    if !ready() {
        return;
    }
    let dir = work_dir("capture-cut");
    let result = go(&dir, &scenario("", "  capture: [access]\n"));
    let bytes = std::fs::read(dir.join(&result.captures[0].file)).unwrap();
    for cut in [bytes.len() - 1, bytes.len() / 2 + 7, pcap::HEADER_LEN + 5] {
        let outcome: Result<Vec<_>, _> = pcap::Reader::new(&bytes[..cut])
            .unwrap()
            .collect::<Result<Vec<_>, _>>();
        assert!(outcome.is_err(), "cut at {cut} of {}", bytes.len());
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn until_exit(
    backend: &mut netscenario_backend_kernel::KernelBackend,
    program: netscenario_core::ProgramHandle,
) {
    use netscenario_core::Backend;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(100))
            .unwrap();
        if state.exit.is_some() {
            return;
        }
        assert!(std::time::Instant::now() < deadline);
    }
}

/// A capture whose buffer is too small loses frames, and says exactly how many: the frames in the file and the missing ones add
/// up to what the interface counted. The agent is started by hand with a receive buffer of a few frames and is stopped with
/// `kill -STOP` while a stream goes by.
#[test]
#[allow(clippy::too_many_lines)] // three programs started by hand, one story
fn a_capture_that_loses_frames_says_how_many_from_the_counter_of_the_interface() {
    use netscenario_agent::events::Event;
    use netscenario_core::{Backend, Program, provision};

    if !ready() {
        return;
    }
    let dir = work_dir("capture-lossy-buffer");
    let mut backend = backend(|_| {});
    let built = provision(&mut backend, &scenario("", "")).unwrap();
    let (client, server) = (built.nodes[0].1, built.nodes[1].1);
    let agent = agent_exe().unwrap().display().to_string();
    let file = dir.join("small.pcap");
    // The capture is started through a shell that says its process id and becomes the agent, so that the test can stop it for a
    // while with `kill -STOP`: a capture that does not read cannot keep up, whatever its buffer is
    let capture = backend
        .start_program(
            server,
            &Program::new(
                [
                    "sh",
                    "-c",
                    "echo $$; exec \"$0\" \"$@\"",
                    &agent,
                    "capture",
                    "--interface",
                    "l0",
                    "--out",
                    &file.display().to_string(),
                    "--receive-buffer",
                    "4096",
                ]
                .map(str::to_owned)
                .to_vec(),
            ),
        )
        .unwrap();
    let wait_lines =
        |backend: &mut netscenario_backend_kernel::KernelBackend, program, count: usize| {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let mut lines: Vec<String> = Vec::new();
            while lines.len() < count {
                let state = backend
                    .poll_program(program, Duration::from_millis(100))
                    .unwrap();
                lines.extend(state.lines);
                assert!(state.exit.is_none(), "{}", state.stderr);
                assert!(std::time::Instant::now() < deadline);
            }
            lines
        };
    let lines = wait_lines(&mut backend, capture, 2);
    let pid = lines[0].trim().to_owned();
    assert!(matches!(
        serde_json::from_str::<Event>(&lines[1]).unwrap(),
        Event::CaptureReady { .. }
    ));
    let freeze = backend
        .start_program(
            server,
            &Program::new(["kill", "-STOP", &pid].map(str::to_owned).to_vec()),
        )
        .unwrap();
    until_exit(&mut backend, freeze);
    // A receiver, and a sender of 30,000 small packets a second for one second
    let receiver = backend
        .start_program(
            server,
            &Program::new(
                [
                    &agent,
                    "recv",
                    "--bind",
                    "10.100.0.2:5000",
                    "--out",
                    &dir.join("r.records").display().to_string(),
                ]
                .map(str::to_owned)
                .to_vec(),
            ),
        )
        .unwrap();
    wait_lines(&mut backend, receiver, 1);
    let sender = backend
        .start_program(
            client,
            &Program::new(
                [
                    &agent,
                    "send",
                    "--to",
                    "10.100.0.2:5000",
                    "--rate",
                    "48000000",
                    "--size",
                    "200",
                    "--duration-ns",
                    "1000000000",
                ]
                .map(str::to_owned)
                .to_vec(),
            ),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = backend
            .poll_program(sender, Duration::from_millis(100))
            .unwrap();
        if state.exit.is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    backend.send_line(receiver, "stop").unwrap();
    let thaw = backend
        .start_program(
            server,
            &Program::new(["kill", "-CONT", &pid].map(str::to_owned).to_vec()),
        )
        .unwrap();
    until_exit(&mut backend, thaw);
    backend.send_line(capture, "stop").unwrap();
    let mut summary = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while summary.is_none() {
        let state = backend
            .poll_program(capture, Duration::from_millis(100))
            .unwrap();
        for line in state.lines {
            if let Event::Captured(found) = serde_json::from_str(&line).unwrap() {
                summary = Some(found);
            }
        }
        assert!(std::time::Instant::now() < deadline);
    }
    let summary = summary.unwrap();
    let counted = summary
        .interface_frames
        .expect("the counter of the interface");
    let missing = summary.missing.expect("the check");
    assert!(
        missing > 0,
        "a buffer of a few frames lost nothing: {summary:?}"
    );
    assert_eq!(summary.packets + missing, counted, "{summary:?}");
    // And the file holds exactly the frames it says
    let frames = pcap::Reader::new(std::io::BufReader::new(std::fs::File::open(&file).unwrap()))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(frames.len() as u64, summary.packets);
    backend.teardown().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
