//! The echo of a UDP stream in real namespaces (PRB-12, D-087): the round trip of the packets, against what the two directions of
//! the link physically allow, and what the loss of either way does to the packets that come back.
//!
//! The bounds are derived and written next to each assertion. A time has its physical lower bound exactly and room upward, because
//! a shared machine only ever makes a thread late; a count gets four standard deviations on each side of the model.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The bounds are worked out in floating point from a count and a probability
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod support;

use std::path::Path;
use std::time::Duration;

use netscenario_core::{Error, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario(link: &str, workload: &str, duration: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: echo-test\nseed: 5\nduration: {duration}\ntopology:\n  nodes: [caller, callee]\n  links:\n    - id: access\n      between: [caller, callee]\n{link}workloads:\n  - {workload}\n"
        ),
        "echo-test.yaml",
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

const MS: u64 = 1_000_000;

/// 300 packets over a link with 20 ms one way and 30 ms back, and nothing lost: every packet comes back, and none of them in less
/// than the sum of the two delays. The way back alone is never shorter than its delay, so each round trip is at least the delay
/// of its way there plus 30 ms.
#[test]
fn every_packet_comes_back_and_not_faster_than_the_two_delays() {
    if !ready() {
        return;
    }
    let dir = work_dir("echo-round-trip");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 20ms }\n      reverse: { delay: 30ms }\n",
            "{ id: call, kind: udp-cbr, from: caller, to: callee, rate: 800kbit, packet-size: 1000b, echo: true }",
            "3s",
        ),
    )
    .unwrap();
    let workload = &result.workloads[0];
    let echo = workload.echo.as_ref().expect("a workload with an echo");
    // Nothing is lost on a link without loss: 3 s at 100 packets a second
    assert_eq!(workload.stats.sent, 300);
    assert_eq!(
        (echo.stats.sent, echo.stats.received, echo.stats.lost),
        (300, 300, 0)
    );
    assert_eq!((echo.echoed, echo.echo_errors), (300, 0));
    let trip = echo.stats.delay.as_ref().unwrap();
    let way = workload.stats.delay.as_ref().unwrap();
    // Physics: the way there holds a packet for 20 ms and the way back for 30 ms
    assert!(
        trip.min_ns >= 50 * MS,
        "the round trip was {} ns",
        trip.min_ns
    );
    assert!(
        trip.min_ns >= way.min_ns + 30 * MS,
        "the round trip was {} ns, the way there {} ns",
        trip.min_ns,
        way.min_ns
    );
    // Room upward: the median is the delays and the path, a shared machine adds a few milliseconds
    assert!(trip.p50_ns <= 50 * MS + 5 * MS, "{} ns", trip.p50_ns);
    // The collector wrote its records next to the others
    assert!(dir.join(&echo.records_file).is_file());
}

/// A stream of 4,000 packets with a loss of 10 % on the way there and 15 % on the way back. A packet comes back if it survives both:
/// 0.9 x 0.85 = 76.5 %, 3,060 packets with a standard deviation of sqrt(4000 x 0.765 x 0.235) = 26.8, so 3,060 plus and minus 107
/// (four of them). Every packet that comes back arrived on the way there first, so the packets that came back are never more than
/// the ones that arrived, and the packets that did not are `sent - returned`.
#[test]
fn the_loss_of_either_way_is_missing_from_the_packets_that_came_back() {
    if !ready() {
        return;
    }
    let dir = work_dir("echo-loss");
    let result = go(
        &dir,
        &scenario(
            "      forward: { delay: 5ms, loss: { model: random, p: 10% } }\n      reverse: { delay: 5ms, loss: { model: random, p: 15% } }\n",
            "{ id: call, kind: udp-cbr, from: caller, to: callee, rate: 8mbit, packet-size: 1000b, echo: true }",
            "4s",
        ),
    )
    .unwrap();
    let workload = &result.workloads[0];
    let echo = workload.echo.as_ref().unwrap();
    assert_eq!(workload.stats.sent, 4000);
    assert!(
        (3060 - 107..=3060 + 107).contains(&echo.stats.received),
        "{} packets came back, the model gives 3060 plus and minus 107",
        echo.stats.received
    );
    assert!(echo.stats.received <= workload.stats.received);
    assert_eq!(echo.stats.lost, 4000 - echo.stats.received);
    // The far node sent back every packet it recorded
    assert_eq!(
        echo.echoed,
        workload.stats.received + workload.stats.duplicates
    );
    // The way back alone: of the packets that arrived, 85 % come back. 3,600 plus and minus four of sqrt(3600 x 0.85 x 0.15) = 21.4
    let arrived = workload.stats.received as f64;
    let share = echo.stats.received as f64 / arrived;
    let sigma = (0.85 * 0.15 / arrived).sqrt();
    assert!(
        (0.85 - 4.0 * sigma..=0.85 + 4.0 * sigma).contains(&share),
        "{share} of the packets that arrived came back"
    );
}

/// The sequence numbers of the packets that came back, from the records of the collector.
fn returned(dir: &Path, result: &RunResult) -> Vec<u64> {
    let echo = result.workloads[0].echo.as_ref().unwrap();
    let file = std::fs::File::open(dir.join(&echo.records_file)).unwrap();
    let mut sequences: Vec<u64> = netscenario_agent::records::read(std::io::BufReader::new(file))
        .unwrap()
        .records
        .iter()
        .map(|record| record.sequence)
        .collect();
    sequences.sort_unstable();
    sequences.dedup();
    sequences
}

const CALL: &str = "{ id: call, kind: udp-cbr, from: caller, to: callee, rate: 800kbit, packet-size: 1000b, echo: true }";

/// The packets of the echo enter the reverse direction, which the run cannot hold to its own count: it leaves the tally of the
/// directions and the check of the queues out. A stream that the far node sends back in the order it arrived, over a way there
/// that keeps the order, meets the numbers of the queue of the way back in a fixed order, so the same seed loses the same packets
/// on both ways, and the run says that it can be repeated; the proof is two runs with the same packets back.
#[test]
fn the_same_seed_loses_the_same_packets_on_the_way_there_and_the_way_back() {
    if !ready() {
        return;
    }
    let link = "      forward: { loss: { model: random, p: 10% } }\n      reverse: { loss: { model: random, p: 10% } }\n";
    let scenario = scenario(link, CALL, "2s");
    let first_dir = work_dir("echo-same-1");
    let first = go(&first_dir, &scenario).unwrap();
    let second_dir = work_dir("echo-same-2");
    let second = go(&second_dir, &scenario).unwrap();
    assert!(first.seed.loss_reproducible, "{:?}", first.seed);
    assert!(first.seed.queue_checks.is_empty() && first.directions.is_empty());
    let (one, two) = (returned(&first_dir, &first), returned(&second_dir, &second));
    // 200 packets with a loss of 10 % each way: 162 come back, and not the same few by chance
    assert!(
        one.len() > 120 && one.len() < 195,
        "{} came back",
        one.len()
    );
    assert_eq!(one, two, "the same seed gave other packets back");
    // A canary: another seed loses other packets, so the comparison can fail
    let mut other = scenario.clone();
    other.seed = Some(6);
    let third_dir = work_dir("echo-same-3");
    let third = go(&third_dir, &other).unwrap();
    assert_ne!(one, returned(&third_dir, &third));
}

/// What can change the order of the packets on the way there, or put the packets of another flow into the queue of the way back,
/// takes the guarantee away, and the run says which.
#[test]
fn a_jitter_before_the_echo_or_a_second_echo_into_the_same_queue_makes_the_run_not_reproducible() {
    if !ready() {
        return;
    }
    let loss_back = "      reverse: { loss: { model: random, p: 10% } }\n";
    let jitter = go(
        &work_dir("echo-jitter"),
        &scenario(
            &format!("      forward: {{ delay: 40ms, jitter: {{ amount: 10ms, distribution: normal }} }}\n{loss_back}"),
            CALL,
            "2s",
        ),
    )
    .unwrap();
    assert!(!jitter.seed.loss_reproducible);
    assert!(
        jitter
            .seed
            .not_reproducible_because
            .iter()
            .any(|reason| reason.contains("'call' (its echo)")
                && reason.contains("change the order")),
        "{:?}",
        jitter.seed.not_reproducible_because
    );
    // Two streams echoed through the same queue
    let second = format!(
        "{CALL}\n  - {{ id: video, kind: udp-cbr, from: caller, to: callee, rate: 800kbit, packet-size: 1000b, echo: true }}"
    );
    let two = go(&work_dir("echo-two"), &scenario(loss_back, &second, "2s")).unwrap();
    assert!(
        two.seed.not_reproducible_because.iter().any(|reason| {
            reason.contains("'call' (its echo)") && reason.contains("'video' (its echo)")
        }),
        "{:?}",
        two.seed.not_reproducible_because
    );
    // A canary: without an echo the queue of the way back has no packets and nothing is said
    let silent = go(
        &work_dir("echo-silent"),
        &scenario(
            loss_back,
            "{ id: call, kind: udp-cbr, from: caller, to: callee, rate: 800kbit, packet-size: 1000b }",
            "2s",
        ),
    )
    .unwrap();
    assert!(silent.seed.loss_reproducible, "{:?}", silent.seed);
    assert!(silent.workloads[0].echo.is_none());
}

/// A canary for the bound of the first test: a way back without a delay gives a round trip below the sum of the two delays, so the
/// bound of 50 ms is one that a run can fail, and the round trip is the way back too and not only the way there.
#[test]
fn canary_a_way_back_without_a_delay_is_a_shorter_round_trip() {
    if !ready() {
        return;
    }
    let result = go(
        &work_dir("echo-canary"),
        &scenario(
            "      forward: { delay: 20ms }
",
            "{ id: call, kind: udp-cbr, from: caller, to: callee, rate: 800kbit, packet-size: 1000b, echo: true }",
            "2s",
        ),
    )
    .unwrap();
    let trip = result.workloads[0]
        .echo
        .as_ref()
        .unwrap()
        .stats
        .delay
        .as_ref()
        .unwrap();
    assert!(
        trip.min_ns >= 20 * MS && trip.min_ns < 50 * MS,
        "{} ns",
        trip.min_ns
    );
}
