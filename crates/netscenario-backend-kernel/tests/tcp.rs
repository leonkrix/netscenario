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
