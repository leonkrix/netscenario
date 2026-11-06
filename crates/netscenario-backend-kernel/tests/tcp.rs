//! The TCP workloads in real namespaces (WRK-04, WRK-05, PRB-12; D-086): what a bulk transfer and a request/response workload
//! report, against what the network physically allows, and what a failure, an unknown congestion control and the class of the
//! run look like in the result.
//!
//! The bounds are derived and written next to each assertion. A time has its physical lower bound exactly (nothing is faster than
//! the link) and room upward, because a shared machine only ever makes a thread late.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The bounds are worked out in floating point from a rate and a size, and compared with times in nanoseconds
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod support;

use std::path::Path;
use std::time::Duration;

use netscenario_core::{Error, RunOptions, RunResult, TcpResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario(link: &str, workload: &str, duration: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: tcp-test\nseed: 5\nduration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {workload}\n"
        ),
        "tcp-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn go(dir: &Path, scenario: &Scenario) -> Result<RunResult, Error> {
    let mut backend = backend(|_| {});
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_millis(500));
    let result = run(&mut backend, scenario, &options);
    assert_nothing_left(backend.run_id(), "after the run");
    result
}

fn ready() -> bool {
    namespaces_available() && agent_exe().is_some()
}

fn only(result: &RunResult) -> &TcpResult {
    assert_eq!(result.tcp.len(), 1);
    &result.tcp[0]
}

/// The frame of a link of `rate` carries 1448 bytes of payload in 1514 bytes (the headers of Ethernet, IP and TCP with the
/// timestamps option), so no more than 1448/1514 of the rate can be goodput.
const PAYLOAD_SHARE: f64 = 1448.0 / 1514.0;

/// A bulk transfer over a link of 10 mbit with 20 ms each way (a round trip of 40 ms): every byte arrives, the transfer is
/// complete after at least the time the link needs to carry it, and the kernel's round trip time is at least the round trip of
/// the link.
#[test]
fn a_bulk_transfer_arrives_whole_and_is_not_faster_than_the_link() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-bulk");
    let bytes = 2_000_000_u64;
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 20ms, rate: 10mbit }\n      reverse: { delay: 20ms }\n",
            &format!("{{ id: down, kind: tcp-bulk, from: client, to: server, bytes: {bytes}b }}"),
            "8s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    assert_eq!(tcp.kind, "tcp-bulk");
    assert_eq!(
        (tcp.connections, tcp.failed_connections),
        (1, 0),
        "{:?}",
        tcp.first_error
    );
    // Reliable: every byte that was sent arrived, to the byte
    assert_eq!((tcp.bytes_sent, tcp.bytes_received), (bytes, bytes));
    let transfer = &tcp.transfers[0];
    assert!(transfer.ok);
    // Physical lower bound: the frames of the whole payload at the rate of the link, 1514 bytes for each 1448, and the setup of a
    // round trip before. Nothing is faster than that.
    let carry_ns = (bytes as f64 * 8.0 / (10e6 * PAYLOAD_SHARE) * 1e9) as u64;
    assert!(
        transfer.duration_ns() >= carry_ns,
        "done after {} ns, the link needs {carry_ns} ns",
        transfer.duration_ns()
    );
    // Room upward: twice the time to carry it and ten round trips, for the slow start, a loss of a segment and a shared machine
    assert!(
        transfer.duration_ns() <= 2 * carry_ns + 10 * 40_000_000,
        "done after {} ns, the link needs {carry_ns} ns",
        transfer.duration_ns()
    );
    // The goodput cannot be above the share of the rate, and not far below it
    let goodput = tcp.goodput.as_ref().unwrap();
    let mean = goodput.mean_bits_per_second.unwrap() as f64;
    assert!(mean <= 10e6 * PAYLOAD_SHARE * 1.001, "{mean}");
    assert!(
        mean >= bytes as f64 * 8.0 / ((2 * carry_ns + 400_000_000) as f64 / 1e9),
        "{mean}"
    );
    // The series adds up to the payload: each bin is bits per second over a bin, so bits/s times the bin is the bits in it
    let bits: f64 = goodput
        .bits_per_second
        .iter()
        .map(|&b| b as f64 * goodput.bin_ns as f64 / 1e9)
        .sum();
    assert!(
        (bits - bytes as f64 * 8.0).abs()
            <= bytes as f64 * 8.0 * 1e-6 + goodput.bits_per_second.len() as f64,
        "{bits}"
    );
    // The kernel's round trip time is never below the round trip of the link
    let rtt = tcp.rtt.as_ref().unwrap();
    assert!(rtt.min_ns >= 40_000_000, "{rtt:?}");
    // A TCP run says that it is statistical, and that it was not accounted for packet by packet
    assert_eq!(result.reproducibility.class, "statistical");
    assert_eq!(result.directions.len(), 0);
    assert_eq!(
        tcp.congestion.as_deref(),
        Some("cubic"),
        "the default is cubic"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A closed loop sends the next request when the response has arrived: it cannot do more than one request per round trip, and the
/// latency from the moment due is the round trip itself (a request is due when it is sent).
#[test]
fn a_closed_loop_does_one_request_for_each_round_trip_at_most() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-closed");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 25ms }\n      reverse: { delay: 25ms }\n",
            "{ id: api, kind: request-response, from: client, to: server, request: 200b, response: 3kb, cc: reno }",
            "2s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    let requests = tcp.requests.as_ref().unwrap();
    assert_eq!(tcp.failed_connections, 0, "{:?}", tcp.first_error);
    assert_eq!(requests.failed, 0);
    assert_eq!(requests.sent, requests.answered);
    // The round trip is 50 ms and the connection takes one round trip to be made: in 2 s at most (2000 - 50) / 50 requests
    assert!(requests.answered <= 39, "{}", requests.answered);
    // And not much fewer: each takes a round trip and a little (the link has no rate), a shared machine delays some
    assert!(requests.answered >= 20, "{}", requests.answered);
    let latency = requests.latency.as_ref().unwrap();
    let trip = requests.round_trip.as_ref().unwrap();
    assert!(trip.min_ns >= 50_000_000, "{trip:?}");
    assert!(trip.p50_ns <= 100_000_000, "{trip:?}");
    // Due when sent: nothing waited for a free connection
    assert_eq!(latency, trip);
    assert_eq!(tcp.congestion.as_deref(), Some("reno"));
    assert_eq!(tcp.bytes_received, requests.answered * 3_000);
    let _ = std::fs::remove_dir_all(dir);
}

/// An open loop has a request due every interval: the grid fixes how many there are, whatever the connections do, and a request
/// that waited for a busy connection has the wait in its latency.
#[test]
fn an_open_loop_has_a_request_due_every_interval() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-open");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 10ms }\n      reverse: { delay: 10ms }\n",
            "{ id: api, kind: request-response, from: client, to: server, request: 100b, response: 100b, connections: 2, interval: 100ms }",
            "2s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    let requests = tcp.requests.as_ref().unwrap();
    assert_eq!(tcp.failed_connections, 0, "{:?}", tcp.first_error);
    // 2 s every 100 ms: the moments 0, 100, ... 1900 ms, twenty requests, all answered (a round trip of 20 ms)
    assert_eq!(
        (requests.sent, requests.answered, requests.failed),
        (20, 20, 0)
    );
    let latency = requests.latency.as_ref().unwrap();
    assert!(latency.min_ns >= 20_000_000, "{latency:?}");
    assert!(latency.p50_ns <= 120_000_000, "{latency:?}");
    assert_eq!(tcp.connections, 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// Canary: a request that gets no answer in its time is a failure that the result shows, with the connection that carried it.
#[test]
fn a_request_without_an_answer_in_time_is_a_failure_in_the_result() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-timeout");
    // A round trip of 400 ms and a patience of 100 ms: the connection is made, no response ever comes in time
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 200ms }\n      reverse: { delay: 200ms }\n",
            "{ id: api, kind: request-response, from: client, to: server, request: 100b, response: 100b, timeout: 100ms }",
            "3s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    let requests = tcp.requests.as_ref().unwrap();
    assert_eq!(requests.answered, 0);
    assert!(requests.failed >= 1, "{requests:?}");
    assert_eq!(tcp.failed_connections, 1);
    assert!(tcp.first_error.is_some());
    assert!(!tcp.transfers[0].ok);
    let _ = std::fs::remove_dir_all(dir);
}

/// A congestion control that the machine does not offer is refused before anything is built, by name, with the line of the
/// workload; one that it offers is used and named in the result (D-086).
#[test]
fn an_unknown_congestion_control_is_refused_by_name_before_anything_is_built() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-cc");
    let error = go(
        &dir,
        &scenario(
            "      forward: { delay: 1ms }\n",
            "{ id: down, kind: tcp-bulk, from: client, to: server, bytes: 1000b, cc: no-such-cc }",
            "2s",
        ),
    );
    let Err(Error::Capability(refused)) = error else {
        panic!("{error:?}");
    };
    let [refusal] = refused.refusals() else {
        panic!("{refused:?}");
    };
    assert_eq!(refusal.path, "workloads[0].cc");
    assert!(refusal.reason.contains("'no-such-cc'"), "{refusal:?}");
    let _ = std::fs::remove_dir_all(dir);
}

fn with_timeline(link: &str, workload: &str, duration: &str, timeline: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: tcp-test\nseed: 5\nduration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {workload}\ntimeline:\n{timeline}"
        ),
        "tcp-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

/// A transfer on a link of 10 mbit with 20 ms each way and an outage from 6 s to 9 s. Nothing is delivered in an outage, not even a
/// packet that was on its way (D-064), so the bins that lie wholly inside it (7 s and 8 s) have no goodput at all. The baseline is
/// the goodput before the outage, at most the share of the rate that is payload (9.56 mbit) and, out of the slow start, not far
/// below it. TCP sends again with the timer of retransmission, which doubles after each try from 200 ms at the least: the tries
/// after the first one come at 0.2, 0.6, 1.4, 3.0 and 6.2 s of silence, so at the latest 3.2 s after the end of the outage one gets
/// through, and the window is back at the baseline within a few round trips after that; 17 s leaves room.
#[test]
fn a_transfer_stops_in_an_outage_and_the_entry_says_when_its_goodput_is_back() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-recovery");
    let result = go(
        &dir,
        &with_timeline(
            "      both: { delay: 20ms, rate: 10mbit }\n",
            "{ id: down, kind: tcp-bulk, from: client, to: server, cc: reno }",
            "22s",
            "  - { at: 6s, link: access, outage: 3s }\n",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    assert_eq!(tcp.failed_connections, 0, "{:?}", tcp.first_error);
    let goodput = tcp.goodput.as_ref().unwrap();
    // Nothing arrives in the bins that lie inside the outage
    assert_eq!(
        goodput.bits_per_second[7], 0,
        "{:?}",
        goodput.bits_per_second
    );
    assert_eq!(
        goodput.bits_per_second[8], 0,
        "{:?}",
        goodput.bits_per_second
    );
    // The end of the outage has an entry on the way of the data, on the goodput
    let entry = tcp
        .recoveries
        .iter()
        .find(|recovery| {
            recovery.kind == "outage-end" && recovery.direction.as_deref() == Some("forward")
        })
        .unwrap_or_else(|| panic!("{:?}", tcp.recoveries));
    assert_eq!(entry.basis, netscenario_core::Basis::Goodput);
    assert_eq!(entry.disturbance_ns, 6_000_000_000);
    let baseline = entry.baseline.expect("a baseline before the outage");
    assert!(
        (7.8e6..=10e6 * PAYLOAD_SHARE * 1.001).contains(&baseline),
        "the baseline was {baseline} bit/s"
    );
    let recovered = entry.recovered_at_ns.unwrap_or_else(|| {
        panic!(
            "not recovered: {:?} (goodput {:?})",
            entry.reason, goodput.bits_per_second
        )
    });
    eprintln!(
        "RECOVERED {recovered} baseline {baseline} goodput {:?}",
        goodput.bits_per_second
    );
    assert!(recovered >= 9_000_000_000, "recovered at {recovered} ns");
    assert!(recovered <= 17_000_000_000, "recovered at {recovered} ns");
    let _ = std::fs::remove_dir_all(dir);
}

/// A transfer that is done has not dropped to zero: an event after its end has no baseline, and the entry says that the workload
/// was not working. The transfer of 500 kb at 10 mbit takes under a second.
#[test]
fn canary_a_finished_transfer_has_no_baseline_for_an_event_after_its_end() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-recovery-done");
    let result = go(
        &dir,
        &with_timeline(
            "      both: { delay: 20ms, rate: 10mbit }\n",
            "{ id: down, kind: tcp-bulk, from: client, to: server, bytes: 500kb }",
            "8s",
            "  - { at: 5s, link: access, set: { both: { rate: 2mbit } } }\n",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    assert_eq!(tcp.failed_connections, 0, "{:?}", tcp.first_error);
    let done = tcp.completion_ns().expect("the transfer was done");
    assert!(done < 4_000_000_000, "the transfer took {done} ns");
    let recovery = &tcp.recoveries[0];
    assert_eq!((recovery.baseline, recovery.time_ns), (None, None));
    assert!(
        recovery
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("was not working in the bins before")),
        "{:?}",
        recovery.reason
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A `request-response` workload has a goodput of its own: the responses by bin, each where its last byte was read, and the bins
/// add up to the payload that arrived at the client.
#[test]
fn the_responses_of_a_request_workload_are_its_goodput() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-request-goodput");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 10ms }\n      reverse: { delay: 10ms }\n",
            "{ id: api, kind: request-response, from: client, to: server, request: 100b, response: 2kb, connections: 2, interval: 50ms }",
            "3s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    assert_eq!(tcp.failed_connections, 0, "{:?}", tcp.first_error);
    let goodput = tcp.goodput.as_ref().expect("a goodput for the responses");
    let bits: f64 = goodput
        .bits_per_second
        .iter()
        .map(|&b| b as f64 * goodput.bin_ns as f64 / 1e9)
        .sum();
    let expected = tcp.bytes_received as f64 * 8.0;
    assert!(
        (bits - expected).abs() <= expected * 1e-6 + goodput.bits_per_second.len() as f64,
        "{bits} bits in the bins, {expected} arrived"
    );
    assert!(goodput.mean_bits_per_second.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

/// A connection through a router needs the answer to find its way back. The client's address in the answer is the one the kernel
/// chose as the source, and the other nodes have a route to the address of a node only, so the routes name that address as the
/// source: without it the answer went to the address of the link, nothing routed it, and the connection hung until the watchdog
/// ended the run (the first run of TCP over a dumbbell). A bulk transfer and a request workload, in both families, through two
/// routers, with a rate on one link; every byte arrives and no connection fails.
#[test]
fn a_connection_through_routers_gets_its_answer_in_both_families() {
    if !ready() {
        return;
    }
    for (family, tag) in [("v4", "tcp-routed-v4"), ("v6", "tcp-routed-v6")] {
        let dir = work_dir(tag);
        let scenario = Scenario::from_yaml(
            &format!(
                "version: 0\nname: tcp-routed\nseed: 5\nduration: 6s\ntopology:\n  ip: dual\n  nodes: [client, {{ id: edge, role: router }}, {{ id: core, role: router }}, server]\n  links:\n    - {{ id: access, between: [client, edge], both: {{ delay: 5ms, rate: 20mbit }} }}\n    - {{ id: backbone, between: [edge, core], both: {{ delay: 10ms }} }}\n    - {{ id: delivery, between: [core, server], both: {{ delay: 5ms }} }}\nworkloads:\n  - {{ id: bulk, kind: tcp-bulk, from: client, to: server, bytes: 500kb, ip: {family} }}\n  - {{ id: api, kind: request-response, from: server, to: client, request: 100b, response: 2kb, interval: 100ms, stop: 3s, ip: {family} }}\n"
            ),
            "tcp-routed.yaml",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let result = go(&dir, &scenario).unwrap();
        assert_eq!(result.tcp.len(), 2);
        for tcp in &result.tcp {
            assert_eq!(
                (tcp.failed_connections, tcp.connections),
                (0, 1),
                "{family}, {}: {:?}",
                tcp.kind,
                tcp.first_error
            );
        }
        // Reliable: every byte of the transfer arrived, to the byte
        assert_eq!(
            (result.tcp[0].bytes_sent, result.tcp[0].bytes_received),
            (500_000, 500_000),
            "{family}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The agent runs under a limit on its address space (SAF-02), and every connection has two threads in it. The allocator of
/// glibc reserved 64 MiB for the arena of each thread, so the limit was gone with 16 connections and the agent died with
/// "failed to allocate an alternative stack" (found by the first multipath experiment). 64 connections, each with its bytes
/// delivered to the byte, are four times that.
#[test]
fn many_connections_of_one_workload_fit_into_the_memory_limit_of_the_agent() {
    if !ready() {
        return;
    }
    let dir = work_dir("tcp-many-connections");
    let result = go(
        &dir,
        &scenario(
            "      both: { delay: 5ms, rate: 100mbit }\n",
            "{ id: many, kind: tcp-bulk, from: client, to: server, bytes: 50kb, connections: 64 }",
            "5s",
        ),
    )
    .unwrap();
    let tcp = only(&result);
    assert_eq!(
        (tcp.connections, tcp.failed_connections),
        (64, 0),
        "{:?}",
        tcp.first_error
    );
    assert_eq!(
        (tcp.bytes_sent, tcp.bytes_received),
        (64 * 50_000, 64 * 50_000)
    );
    let _ = std::fs::remove_dir_all(dir);
}
