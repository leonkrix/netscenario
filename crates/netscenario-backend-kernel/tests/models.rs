//! Models of a link over time in a real run (LMD-04, LMD-05, TML-03; D-090): a periodic pattern does what the same events
//! written by hand do, and a Markov chain gives the same states and the same lost packets for the same seed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Seconds in nanoseconds and counts of packets are far below the precision of a double
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod support;

use std::time::Duration;

use netscenario_agent::records;
use netscenario_core::{RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

/// The gap between the packets of the stream (1000 bytes at 3 mbit/s), in nanoseconds.
const GAP_NS: u64 = 2_666_667;

fn scenario(head: &str, link: &str, duration: &str, timeline: &str) -> Scenario {
    let mut scenario = Scenario::from_yaml(
        &format!(
            "version: 0\nname: models-test\n{head}duration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: 3mbit, packet-size: 1000b }}\n{timeline}"
        ),
        "models-test.yaml",
    )
    .unwrap();
    scenario.measure.packets = true;
    scenario
}

fn go(scenario: &Scenario, label: &str) -> (RunResult, std::path::PathBuf) {
    let dir = work_dir(label);
    let mut backend = backend(|_| {});
    let mut options = RunOptions::new(agent_exe().unwrap(), &dir);
    options.settle = Some(Duration::from_secs(1));
    let result = run(&mut backend, scenario, &options).unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    (result, dir)
}

fn records_of(result: &RunResult, dir: &std::path::Path) -> Vec<records::Record> {
    records::read(std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap())
        .unwrap()
        .records
}

/// The moment the measured phase began, on the clock of the records (see `timeline.rs`).
fn start_of(records: &[records::Record]) -> u64 {
    records
        .iter()
        .map(|record| record.send_ns.saturating_sub(record.sequence * GAP_NS))
        .min()
        .expect("a packet arrived")
}

fn lost_packets(dir: &std::path::Path) -> Vec<u64> {
    std::fs::read_to_string(dir.join("packets-media.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|packet| packet["lost"] == true)
        .map(|packet| packet["sequence"].as_u64().unwrap())
        .collect()
}

/// What a step was: the kind, the direction, and when the file says it happens.
fn steps(result: &RunResult) -> Vec<(String, Option<String>, u64)> {
    result
        .events
        .iter()
        .map(|e| (e.kind.clone(), e.direction.clone(), e.scheduled_ns))
        .collect()
}

/// A periodic pattern of an `ok` state of 3 s and an outage of 1 s, begun at 0, is the outages at 3 s and 8 s of a run of 12 s
/// (the second period is 5 s to 10 s), which are the same events as the ones written by hand; they are made by the model
/// (`from_model`) and the packets are lost as in any outage: nothing arrives in the windows, and everything arrives between
/// them. The delay of the link is 20 ms, so a packet sent in `[3.0 - 0.02, 4.0 - 0.02)` meets the window; the packets that are
/// sent between 4.1 s and 7.9 s are far from every edge (the slack is 100 ms for the lateness of the sender and the timer of
/// the queue), and those sent between 3.2 s and 3.8 s are inside the first window with the same slack.
#[test]
fn a_periodic_pattern_does_what_the_same_events_written_by_hand_do() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let link = "      both: { delay: 20ms }\n      model:\n        kind: periodic\n        states:\n          - { for: 3s }\n          - { for: 1s, outage: true }\n          - { for: 1s }\n";
    let by_hand = "timeline:\n  - { at: 3s, link: access, outage: 1s }\n  - { at: 8s, link: access, outage: 1s }\n";
    let (modelled, dir) = go(&scenario("", link, "12s", ""), "models-periodic");
    let (written, hand_dir) = go(
        &scenario("", "      both: { delay: 20ms }\n", "12s", by_hand),
        "models-by-hand",
    );
    assert_eq!(steps(&modelled), steps(&written));
    assert_eq!(steps(&modelled).len(), 8, "{:?}", steps(&modelled));
    assert!(modelled.events.iter().all(|event| event.from_model));
    assert!(written.events.iter().all(|event| !event.from_model));
    // The first outage is at 3 s for 1 s, the second at 8 s
    let moments: Vec<u64> = modelled
        .events
        .iter()
        .filter(|e| e.kind == "outage-start" && e.direction.as_deref() == Some("forward"))
        .map(|e| e.scheduled_ns / 1_000_000_000)
        .collect();
    assert_eq!(moments, [3, 8]);
    let records = records_of(&modelled, &dir);
    let base = start_of(&records);
    let arrived: std::collections::BTreeSet<u64> = records.iter().map(|r| r.sequence).collect();
    let sequence_at = |seconds: f64| (seconds * 1e9 / GAP_NS as f64) as u64;
    // The end of the first outage is done by the clock, and on a machine that is busy it can be late: the packets that are checked
    // begin 100 ms after it was really done, which the result records
    let first_end = modelled
        .events
        .iter()
        .find(|e| e.kind == "outage-end" && e.direction.as_deref() == Some("forward"))
        .map_or(0.0, |e| e.applied_ns as f64 / 1e9);
    for n in sequence_at(4.1_f64.max(first_end + 0.1))..sequence_at(7.9) {
        assert!(
            arrived.contains(&n),
            "packet {n} between the outages was lost"
        );
    }
    for n in sequence_at(3.2)..sequence_at(3.8) {
        assert!(!arrived.contains(&n), "packet {n} arrived in the outage");
    }
    // Nothing arrived inside either window, whatever the edges
    for record in &records {
        let at = record.receive_ns.saturating_sub(base);
        let inside = |from: u64, to: u64| {
            at > from * 1_000_000_000 + 100_000_000 && at < to * 1_000_000_000 - 100_000_000
        };
        assert!(
            !inside(3, 4) && !inside(8, 9),
            "packet {} arrived at {at}",
            record.sequence
        );
    }
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(hand_dir);
}

const CHAIN: &str = "      both: { delay: 5ms }\n      model:\n        kind: markov\n        start: calm\n        states:\n          - { id: calm, stay: 600ms, to: { rough: 100% } }\n          - { id: rough, stay: 400ms, to: { calm: 100% }, set: { forward: { loss: { model: random, p: 30% } } } }\n";

/// A Markov chain draws its states from the seed: the same seed gives the same steps and the same lost packets (the steps are
/// done at a barrier of the sender, D-038), another seed gives other ones. The chain alternates between a calm state and a rough
/// one with a loss of 30 % in the direction the stream goes; over 6 s there are about 12 states. (A change of the loss in the other direction, where no workload sends, would be done by the clock and the run would say that it cannot be repeated.)
#[test]
fn the_same_seed_gives_a_chain_the_same_states_and_the_same_lost_packets() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let once = |seed: u32, label: &str| {
        let (result, dir) = go(
            &scenario(&format!("seed: {seed}\n"), CHAIN, "6s", ""),
            label,
        );
        let lost = lost_packets(&dir);
        let _ = std::fs::remove_dir_all(dir);
        (steps(&result), lost, result.seed.clone())
    };
    let (steps_a, lost_a, reproducible) = once(5, "models-chain-a");
    let (steps_b, lost_b, _) = once(5, "models-chain-b");
    let (steps_c, lost_c, _) = once(6, "models-chain-c");
    // Something happened, so that "the same" means something
    assert!(steps_a.len() >= 6, "{steps_a:?}");
    assert_ne!(lost_a, Vec::<u64>::new());
    assert_eq!(steps_a, steps_b, "the same seed made other states");
    assert_eq!(lost_a, lost_b, "the same seed lost other packets");
    assert!(reproducible.loss_reproducible, "{reproducible:?}");
    // Canary: another seed is another run, so the claim above could fail
    assert_ne!(steps_a, steps_c);
    assert_ne!(lost_a, lost_c);
}
