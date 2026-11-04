//! The `trace-replay` workload: a trace named by a file, read when the scenario is loaded, and kept in the scenario (WRK-03,
//! D-083, D-085).

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::fmt::Write as _;

use netscenario_model::{Scenario, Workload};

const HEAD: &str = "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n";

fn scenario(link_extra: &str, workload_extra: &str) -> Result<Scenario, String> {
    let text = format!(
        "{HEAD}{link_extra}workloads:\n  - {{ id: call, kind: trace-replay, from: a, to: b, trace: {{ file: call.csv }}{workload_extra} }}\n"
    );
    Scenario::from_yaml(&text, "t.yaml").map_err(|error| error.render())
}

fn csv(packets: usize) -> Vec<u8> {
    let mut text = String::from("time,size\n");
    for i in 0..packets {
        let _ = writeln!(text, "{}ms,{}b", i * 20, 60 + (i % 7) * 100);
    }
    text.into_bytes()
}

#[test]
fn a_trace_replay_workload_is_read_and_says_what_it_needs() {
    let parsed = scenario("", "").unwrap();
    let Workload::TraceReplay(replay) = &parsed.workloads[0] else {
        panic!("{:?}", parsed.workloads[0]);
    };
    assert_eq!(replay.trace.file.as_deref(), Some("call.csv"));
    assert_eq!(parsed.unresolved_files(), vec!["call.csv".to_owned()]);
    assert_eq!(parsed.workloads[0].kind().as_str(), "trace-replay");
    // A window and the flags of the other generators work the same way
    assert!(
        scenario(
            "",
            ", start: 1s, stop: 5s, udp-checksum: false, dont-fragment: true"
        )
        .is_ok()
    );
}

#[test]
fn reading_the_file_puts_the_packets_into_the_scenario_and_the_scenario_then_needs_no_file() {
    let mut parsed = scenario("", "").unwrap();
    let reads = Cell::new(0);
    let problems = parsed.resolve_files(|path| {
        reads.set(reads.get() + 1);
        assert_eq!(path, "call.csv");
        Ok(csv(200))
    });
    assert_eq!(problems, Vec::new());
    assert_eq!(reads.get(), 1);
    assert_eq!(parsed.unresolved_files(), Vec::<String>::new());
    let Workload::TraceReplay(replay) = &parsed.workloads[0] else {
        panic!();
    };
    let data = replay.trace.packets.as_ref().unwrap();
    assert_eq!(data.len(), 200);
    assert_eq!(data.moments_ns()[3], 60_000_000);
    assert_eq!((data.smallest(), data.largest()), (60, 660));
    assert_eq!(replay.trace.sha256.as_ref().map(String::len), Some(64));
    // The canonical form holds the packets: it reads back to an equal scenario, with no file
    let yaml = parsed.to_yaml().unwrap();
    assert!(yaml.contains("packets:"), "{yaml}");
    let again = Scenario::from_yaml(&yaml, "scenario.yaml").unwrap();
    assert_eq!(again, parsed);
    assert_eq!(again.unresolved_files(), Vec::<String>::new());
    // A change of the file is a change of the scenario
    let mut other = scenario("", "").unwrap();
    assert_eq!(other.resolve_files(|_| Ok(csv(201))), Vec::new());
    assert_ne!(other.to_yaml().unwrap(), yaml);
}

#[test]
fn a_file_that_makes_no_trace_is_a_problem_at_the_file_and_the_scenario_stays_as_it_was() {
    for (content, expected) in [
        (
            Err("cannot read 'call.csv': no such file".to_owned()),
            "no such file",
        ),
        (Ok(b"0ms,100b\n5,100b\n".to_vec()), "line 2: the time"),
        (Ok(b"0ms,8b\n".to_vec()), "below the smallest payload of 16"),
        (Ok(b"".to_vec()), "no packets"),
        (Ok(vec![0xff, 0xfe]), "not text in UTF-8"),
        (Ok(b"not a capture".to_vec()), "line 1"),
    ] {
        let mut parsed = scenario("", "").unwrap();
        let before = parsed.clone();
        let problems = parsed.resolve_files(|_| content.clone());
        assert_eq!(problems.len(), 1, "{expected}: {problems:?}");
        assert!(
            problems[0].message.contains(expected),
            "{}",
            problems[0].message
        );
        assert_eq!(problems[0].path.as_deref(), Some("workloads[0].trace.file"));
        assert_eq!(parsed, before);
    }
}

/// A pinned fingerprint is a promise about the file. Canary: the right one passes.
#[test]
fn a_pinned_fingerprint_must_match_the_file() {
    let bytes = csv(30);
    let hash = netscenario_model::sha256_hex(&bytes);
    let with = |hash: &str| {
        Scenario::from_yaml(
            &format!("{HEAD}workloads:\n  - {{ id: call, kind: trace-replay, from: a, to: b, trace: {{ file: call.csv, sha256: {hash} }} }}\n"),
            "t.yaml",
        )
        .unwrap()
    };
    assert_eq!(with(&hash).resolve_files(|_| Ok(bytes.clone())), Vec::new());
    let problems = with(&"0".repeat(64)).resolve_files(|_| Ok(bytes.clone()));
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0]
            .message
            .contains("is not the file the scenario was written for")
    );
}

/// The sizes of the trace are checked against the path once the file is read: the largest packet must fit the MTU of the first
/// link (or a link with an `mtu` is the way to ask for fragments), and a stream without the checksum has a larger smallest
/// payload.
#[test]
fn the_sizes_of_a_trace_are_checked_against_the_links_and_the_checksum() {
    let big = b"0ms,100b\n1ms,1600b\n".to_vec();
    let mut parsed = scenario("", "").unwrap();
    let problems = parsed.resolve_files(|_| Ok(big.clone()));
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0]
            .message
            .contains("1600 bytes is outside 16 to 1472"),
        "{}",
        problems[0].message
    );
    assert_eq!(problems[0].path.as_deref(), Some("workloads[0].trace"));
    // A link that says its MTU allows the larger packet: the sender cuts it
    let mut with_mtu = scenario("      mtu: 9000\n", "").unwrap();
    assert_eq!(with_mtu.resolve_files(|_| Ok(big.clone())), Vec::new());
    // A stream without the checksum carries a CRC-32 and needs 20 bytes
    let small = b"0ms,18b\n1ms,100b\n".to_vec();
    let mut plain = scenario("", "").unwrap();
    assert_eq!(plain.resolve_files(|_| Ok(small.clone())), Vec::new());
    let mut without = scenario("", ", udp-checksum: false").unwrap();
    let problems = without.resolve_files(|_| Ok(small.clone()));
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0]
            .message
            .contains("below the smallest payload of 20 bytes for `udp-checksum: false`")
            || problems[0]
                .message
                .contains("below the smallest payload of 20"),
        "{}",
        problems[0].message
    );
}

#[test]
fn a_trace_workload_needs_the_nodes_and_a_path_like_any_other() {
    let text = format!(
        "{HEAD}workloads:\n  - {{ id: call, kind: trace-replay, from: a, to: nowhere, trace: {{ file: call.csv }} }}\n"
    );
    let error = Scenario::from_yaml(&text, "t.yaml").unwrap_err().render();
    assert!(
        error.contains("the node 'nowhere' does not exist"),
        "{error}"
    );
}

fn seed(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/seeds")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The seeds of the fuzz targets are real files of the formats: the capture of one flow is a trace of 120 packets, the same flow
/// in `pcapng` is the first 60 of them, and the capture of two flows is refused with the list. A change of a reader that breaks a
/// seed shows here, and the fuzzer starts from inputs that go deep.
#[test]
fn the_seeds_of_the_fuzz_targets_are_files_of_the_formats() {
    use netscenario_model::{FlowSelection, read_capture, read_csv};
    let flow = read_capture(&seed("captures/flow.pcap"), &FlowSelection::default(), 16).unwrap();
    assert_eq!((flow.data.len(), flow.padded), (120, 0));
    assert_eq!(flow.data.moments_ns()[0], 0);
    let ng = read_capture(&seed("captures/flow.pcapng"), &FlowSelection::default(), 16).unwrap();
    assert_eq!(ng.data.len(), 60);
    assert_eq!(ng.data.sizes(), &flow.data.sizes()[..60]);
    assert_eq!(ng.data.moments_ns(), &flow.data.moments_ns()[..60]);
    let error = read_capture(
        &seed("captures/two-flows.pcap"),
        &FlowSelection::default(),
        16,
    )
    .unwrap_err();
    assert!(error.contains("the capture has 2 flows"), "{error}");
    let call = read_csv(&String::from_utf8(seed("traces/call.csv")).unwrap(), 16).unwrap();
    assert_eq!(call.data.len(), 150);
    let bad = read_csv(&String::from_utf8(seed("traces/bad.csv")).unwrap(), 16).unwrap_err();
    assert!(bad.contains("line 3: the time"), "{bad}");
}
