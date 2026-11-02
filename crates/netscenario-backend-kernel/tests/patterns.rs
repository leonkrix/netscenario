//! The traffic patterns in a real run (WRK-02, D-071): a Poisson source, an on/off source and bursts send their packets at
//! the moments of a seeded schedule, the same seed gives the same packets, and a step of the timeline lands before the
//! packet that the schedule says is the first one at its moment.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Counts of packets are far below the precision of a double
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

mod support;

use std::time::Duration;

use netscenario_agent::records;
use netscenario_agent::rng::derive_seed;
use netscenario_agent::schedule::{Distribution, Schedule, Stream};
use netscenario_core::{RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

const SECOND: u64 = 1_000_000_000;

fn scenario(seed: u64, duration: &str, link: &str, workload: &str, timeline: &str) -> Scenario {
    let timeline = if timeline.is_empty() {
        String::new()
    } else {
        format!("timeline:\n{timeline}")
    };
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: patterns-test\nseed: {seed}\nduration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {workload}\n{timeline}"
        ),
        "patterns-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn options(dir: &std::path::Path) -> RunOptions {
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_secs(1));
    options
}

fn records_of(result: &RunResult, dir: &std::path::Path) -> Vec<records::Record> {
    records::read(std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap())
        .unwrap()
        .records
}

/// What the sender did, against what the schedule says: every packet of the schedule was sent, none more, and each left
/// at its moment or later. The start of the measured phase is not known to the nanosecond, so it is taken as the smallest
/// `send - moment` of all packets, which is the start plus the lateness of the least late packet; every other packet then
/// is late by at most the `max_lateness` that the sender reported (a packet is never early).
fn assert_sent_as_scheduled(
    schedule: &Schedule,
    duration_ns: u64,
    result: &RunResult,
    records: &[records::Record],
) {
    let offsets: Vec<u64> = schedule.offsets(duration_ns).collect();
    let stats = &result.workloads[0].stats;
    assert_eq!(stats.sent, offsets.len() as u64, "packets sent");
    assert_eq!(result.workloads[0].sender.send_errors, 0);
    let start = records
        .iter()
        .map(|record| {
            record
                .send_ns
                .saturating_sub(offsets[usize::try_from(record.sequence).unwrap()])
        })
        .min()
        .expect("a packet arrived");
    let worst = records
        .iter()
        .map(|record| record.send_ns - start - offsets[usize::try_from(record.sequence).unwrap()])
        .max()
        .unwrap();
    assert!(
        worst <= result.workloads[0].sender.max_lateness_ns,
        "a packet left {worst} ns after its moment, the sender says at most {}",
        result.workloads[0].sender.max_lateness_ns
    );
}

/// A Poisson source of 1 mbit with 125 byte packets is a mean of 1,000 packets a second, with the gaps drawn from the seed
/// of the workload: the sender sends exactly the packets of that schedule, at their moments, and a link without loss
/// delivers them all.
#[test]
fn a_poisson_source_sends_the_packets_of_its_schedule_at_their_moments() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("patterns-poisson");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            5,
            "6s",
            "      forward: { delay: 5ms }\n",
            "{ id: media, kind: udp-poisson, from: client, to: server, rate: 1mbit, packet-size: 125b }",
            "",
        ),
        &options(&dir),
    )
    .unwrap();
    let schedule = Schedule::Poisson {
        stream: Stream {
            bits_per_second: 1_000_000,
            packet_size: 125,
        },
        seed: derive_seed(5, 0),
    };
    let records = records_of(&result, &dir);
    assert_sent_as_scheduled(&schedule, 6 * SECOND, &result, &records);
    let workload = &result.workloads[0];
    assert_eq!(workload.kind, "udp-poisson");
    assert_eq!(workload.rate_bits_per_second, 1_000_000);
    assert_eq!(workload.stats.lost, 0);
    // The schedule is not a constant rate: the gaps differ (a constant bit rate has one gap)
    let mut gaps: Vec<u64> = records
        .windows(2)
        .map(|w| w[1].send_ns - w[0].send_ns)
        .collect();
    gaps.sort_unstable();
    assert!(
        gaps[gaps.len() * 9 / 10] > 4 * gaps[gaps.len() / 10],
        "the gaps are all alike"
    );
    assert!(!result.seed.drawn && result.seed.value == Some(5));
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// An on/off source whose cycle is a bin of the series: 8 mbit with 100 byte packets for 20 ms (a packet every 100 us) and
/// off for 80 ms, so every bin of 100 ms holds exactly 200 packets that were sent in it and, with 0.1 ms of delay on the
/// link, arrived in it.
#[test]
fn an_on_off_source_fills_every_bin_of_its_cycle_in_the_series() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("patterns-onoff");
    let mut backend = backend(|_| {});
    let mut binned = scenario(
        5,
        "3s",
        "",
        "{ id: media, kind: udp-onoff, from: client, to: server, rate: 8mbit, packet-size: 100b, on: 20ms, off: 80ms }",
        "",
    );
    let mut series = netscenario_model::SeriesSettings::default();
    series.bin = Some("100ms".parse().unwrap());
    binned.measure.series = Some(series);
    let result = run(&mut backend, &binned, &options(&dir)).unwrap();
    let schedule = Schedule::OnOff {
        stream: Stream {
            bits_per_second: 8_000_000,
            packet_size: 100,
        },
        on_ns: 20_000_000,
        off_ns: 80_000_000,
        distribution: Distribution::Fixed,
        seed: derive_seed(5, 0),
    };
    let records = records_of(&result, &dir);
    assert_sent_as_scheduled(&schedule, 3 * SECOND, &result, &records);
    let text = std::fs::read_to_string(dir.join("series-media.jsonl")).unwrap();
    let bins: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for bin in &bins[..30] {
        assert_eq!(bin["by_send"]["sent"], 200, "{bin}");
        assert_eq!(bin["by_send"]["received"], 200, "{bin}");
    }
    // Nothing is sent after the 3 s: the bin after them is empty
    assert!(bins.iter().skip(30).all(|bin| bin["by_send"]["sent"] == 0));
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// Bursts of 5 packets 200 us apart with a drawn pause of about 20 ms: the sender sends the packets of the schedule, and
/// the second run with the same seed sends the same number, a run with another seed does not.
#[test]
fn the_same_seed_gives_the_same_bursts_and_another_seed_does_not() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let sent = |seed: u64, label: &str| {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario(
                seed,
                "3s",
                "",
                "{ id: media, kind: udp-bursty, from: client, to: server, packet-size: 200b, burst-packets: 5, gap: 200us, pause: 20ms, distribution: exponential }",
                "",
            ),
            &options(&dir),
        )
        .unwrap();
        let schedule = Schedule::Bursty {
            packet_size: 200,
            packets: 5,
            gap_ns: 200_000,
            pause_ns: 20_000_000,
            distribution: Distribution::Exponential,
            seed: derive_seed(seed, 0),
        };
        let records = records_of(&result, &dir);
        assert_sent_as_scheduled(&schedule, 3 * SECOND, &result, &records);
        let sent = result.workloads[0].stats.sent;
        let _ = std::fs::remove_dir_all(dir);
        sent
    };
    let (first, again, other) = (
        sent(11, "patterns-bursty-a"),
        sent(11, "patterns-bursty-b"),
        sent(12, "patterns-bursty-c"),
    );
    assert_eq!(first, again);
    // 3 s hold about 3 / 0.021 = 143 bursts of 5, and the count has a standard deviation of about 12 bursts (60 packets)
    // for the draw; a different seed giving exactly the same count would be a chance of about 1 in 100
    assert_ne!(first, other, "two seeds gave the same {first} packets");
}

/// A step of the timeline lands before the packet that the schedule says is the first at its moment, also when the moments
/// of the packets are drawn (D-038): the barrier of a Poisson workload is the number of packets of the schedule before the
/// moment, and the lost packets are the same in two runs with the same seed.
#[test]
fn a_step_lands_before_the_packet_that_the_random_schedule_says_and_the_loss_repeats() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let lost = |label: &str| {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let mut exporting = scenario(
            5,
            "4s",
            "      forward: { delay: 5ms }\n",
            "{ id: media, kind: udp-poisson, from: client, to: server, rate: 1mbit, packet-size: 125b }",
            "  - { at: 2s, link: access, set: { forward: { loss: { model: random, p: 30% } } } }\n",
        );
        exporting.measure.packets = true;
        let result = run(&mut backend, &exporting, &options(&dir)).unwrap();
        let lost: Vec<u64> = std::fs::read_to_string(dir.join("packets-media.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|packet| packet["lost"] == true)
            .map(|packet| packet["sequence"].as_u64().unwrap())
            .collect();
        let _ = std::fs::remove_dir_all(dir);
        (
            lost,
            result.events[0].clone(),
            result.seed.loss_reproducible,
        )
    };
    let (first, step, reproducible) = lost("patterns-step-a");
    let (second, _, _) = lost("patterns-step-b");
    let schedule = Schedule::Poisson {
        stream: Stream {
            bits_per_second: 1_000_000,
            packet_size: 125,
        },
        seed: derive_seed(5, 0),
    };
    let before = schedule.packets_before_each(&[2 * SECOND])[0];
    assert_eq!(
        step.packet,
        Some(before),
        "the step lands before packet {before}"
    );
    assert!(reproducible);
    assert_eq!(
        first, second,
        "the lost packets differ between two runs with the same seed"
    );
    // The link loses nothing before the step, and 30 % of 2,000 packets after it (standard deviation 20, four of them 80)
    assert!(
        first.iter().all(|n| *n >= before),
        "a packet before the step was lost"
    );
    assert!(
        (first.len() as f64 - 0.3 * 2000.0).abs() < 4.0 * (2000.0_f64 * 0.3 * 0.7).sqrt() + 100.0,
        "{} lost",
        first.len()
    );
}
