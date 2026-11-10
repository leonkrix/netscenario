//! The profile library in a real run (SCN-06, D-095): for every profile a link with `profile: <id>` delays the packets of each
//! direction by what the profile says, the rate of a profile is the rate of the link, and the loss of a profile is in the queue of
//! the interface. The numbers the tests expect are written here and not read from the library, so that a changed number of the
//! library fails until it is changed here on purpose; a test that lists the profiles fails for a profile without a test.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

mod support;

use std::time::Duration;

use netscenario_agent::records::{self, Record};
use netscenario_core::{RunOptions, RunResult, run};
use netscenario_model::{Scenario, library};
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

/// A terminal and a network with the profile on the link and one stream of 20 packets a second in each direction. `extra` is
/// written next to the profile (the keys of the file win), `streams` replaces the two streams.
fn scenario(id: &str, extra: &str, streams: &str) -> Scenario {
    let yaml = format!(
        "version: 0\nname: profile-test\nduration: 3s\ntopology:\n  nodes: [terminal, network]\n  links:\n    - id: access\n      between: [terminal, network]\n      profile: {id}\n{extra}workloads:\n{streams}"
    );
    let mut scenario = Scenario::from_yaml(&yaml, "profile-test.yaml").unwrap();
    scenario.measure.packets = true;
    scenario
}

const TWO_STREAMS: &str = "  - { id: up, kind: udp-cbr, from: terminal, to: network, rate: 160kbit, packet-size: 1000b }\n  - { id: down, kind: udp-cbr, from: network, to: terminal, rate: 160kbit, packet-size: 1000b }\n";

fn go(scenario: &Scenario, label: &str) -> (RunResult, Vec<Vec<Record>>, std::path::PathBuf) {
    let dir = work_dir(label);
    let mut backend = backend(|_| {});
    let mut options = RunOptions::new(agent_exe().unwrap(), &dir);
    options.settle = Some(Duration::from_secs(1));
    let result = run(&mut backend, scenario, &options).unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    let records = result
        .workloads
        .iter()
        .map(|w| {
            records::read(std::fs::File::open(dir.join(&w.records_file)).unwrap())
                .unwrap()
                .records
        })
        .collect();
    (result, records, dir)
}

/// The delays of a stream, in seconds, and whether they hold: none is below `declared` (a packet is held for the delay and never
/// arrives earlier, so the lower bound is exact), and the median is above it by what the two interfaces and the receiver cost, which
/// is a few milliseconds and no more than `room`.
fn delay_holds(records: &[Record], declared: f64, room: f64) -> Result<(), String> {
    if records.len() < 20 {
        return Err(format!("only {} packets arrived", records.len()));
    }
    let mut delays: Vec<f64> = records.iter().map(|r| r.delay_ns() as f64 / 1e9).collect();
    delays.sort_by(f64::total_cmp);
    let (smallest, median) = (delays[0], delays[delays.len() / 2]);
    if smallest < declared - 1e-9 {
        return Err(format!(
            "a packet arrived after {smallest} s, before the declared {declared} s"
        ));
    }
    if median > declared + room {
        return Err(format!(
            "the median delay is {median} s, more than {room} s above the declared {declared} s"
        ));
    }
    Ok(())
}

/// The delay of each direction in seconds (up, down) as written in the table of docs/PROFILES.md.
const DELAYS: &[(&str, f64, f64)] = &[
    ("3g-good", 0.015, 0.018),
    ("3g-typical", 0.02115, 0.02115),
    ("lte-good", 0.024, 0.008),
    ("lte-typical", 0.01055, 0.01055),
    ("5g-typical", 0.0091, 0.0091),
    ("5g-poor", 0.0218, 0.0218),
    ("dsl-good", 0.010, 0.010),
    ("dsl-poor", 0.0305, 0.0305),
    ("geo-good", 0.2985, 0.2985),
    ("geo-typical", 0.33675, 0.33675),
    ("geo-poor", 0.4176, 0.4176),
    ("leo-typical", 0.0225, 0.0225),
    ("datacenter-rack", 0.000_108, 0.000_108),
    ("datacenter-fabric", 0.000_134, 0.000_134),
];

/// What a few milliseconds of two interfaces, a receiver that is not scheduled at once and a busy machine add to a delay.
const ROOM: f64 = 0.015;

fn measured_delays(id: &str) {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let &(_, up, down) = DELAYS.iter().find(|(name, ..)| *name == id).unwrap();
    let (_, records, dir) = go(&scenario(id, "", TWO_STREAMS), &format!("profile-{id}"));
    // `up` is the first workload, from the terminal, which is the first node of the link: its delay is `forward`
    delay_holds(&records[0], up, ROOM).unwrap_or_else(|why| panic!("{id}, up: {why}"));
    delay_holds(&records[1], down, ROOM).unwrap_or_else(|why| panic!("{id}, down: {why}"));
    let _ = std::fs::remove_dir_all(dir);
}

macro_rules! delay_tests {
    ($($name:ident: $id:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                measured_delays($id);
            }
        )*
    };
}

delay_tests! {
    the_delays_of_3g_good_are_the_published_medians: "3g-good",
    the_delays_of_3g_typical_are_half_of_the_round_trip: "3g-typical",
    the_delays_of_lte_good_differ_by_direction: "lte-good",
    the_delays_of_lte_typical_are_half_of_the_round_trip: "lte-typical",
    the_delays_of_5g_typical_are_half_of_the_round_trip: "5g-typical",
    the_delays_of_5g_poor_are_the_published_average: "5g-poor",
    the_delays_of_dsl_good_are_half_of_the_round_trip: "dsl-good",
    the_delays_of_dsl_poor_are_half_of_the_round_trip: "dsl-poor",
    the_delays_of_geo_good_are_half_of_the_round_trip: "geo-good",
    the_delays_of_geo_typical_are_half_of_the_median: "geo-typical",
    the_delays_of_geo_poor_are_half_of_the_median: "geo-poor",
    the_delays_of_leo_typical_are_half_of_the_middle_median: "leo-typical",
    the_delays_of_datacenter_rack_are_half_of_the_round_trip: "datacenter-rack",
    the_delays_of_datacenter_fabric_are_half_of_the_round_trip: "datacenter-fabric",
}

/// Canary for the list above: a profile in the library without a test here fails, and so does a test for a profile that is gone.
#[test]
fn every_profile_of_the_library_is_tested_and_the_table_is_the_library() {
    let mut tested: Vec<&str> = DELAYS.iter().map(|(id, ..)| *id).collect();
    let mut library_ids: Vec<&str> = library().profiles().iter().map(|p| p.id.as_str()).collect();
    tested.sort_unstable();
    library_ids.sort_unstable();
    assert_eq!(
        tested, library_ids,
        "a profile without a test, or a test without a profile"
    );
    // The numbers of the table are those of the library: a changed number fails here until the table is changed on purpose
    for &(id, up, down) in DELAYS {
        let profile = library().get(id).unwrap();
        let seconds = |d: Option<netscenario_model::Duration>| {
            std::time::Duration::from(d.unwrap()).as_secs_f64()
        };
        assert!(
            (seconds(profile.forward().delay) - up).abs() < 1e-9,
            "{id} up"
        );
        assert!(
            (seconds(profile.reverse().delay) - down).abs() < 1e-9,
            "{id} down"
        );
    }
}

/// Canary for the meter: it fails for a link that is faster than the profile says, and for one that is slower.
#[test]
fn the_meter_of_the_delay_can_fail() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    // A link of 10 ms measured against 20 ms: packets arrive before the declared delay
    let (_, records, dir) = go(
        &scenario("lte-good", "      both: { delay: 10ms }\n", TWO_STREAMS),
        "profile-canary",
    );
    assert!(delay_holds(&records[0], 0.020, ROOM).is_err());
    // The same link against 2 ms: the median is more than the room above it
    assert!(delay_holds(&records[0], 0.0, 0.002).is_err());
    assert!(delay_holds(&records[0], 0.010, ROOM).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

/// The keys of the file win over the profile in a real run: the profile says 24 ms up and 8 ms down, the file says 60 ms up.
#[test]
fn a_key_next_to_the_profile_wins_in_the_run() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (_, records, dir) = go(
        &scenario("lte-good", "      forward: { delay: 60ms }\n", TWO_STREAMS),
        "profile-override",
    );
    delay_holds(&records[0], 0.060, ROOM).unwrap();
    delay_holds(&records[1], 0.008, ROOM).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// The result records where the values came from.
#[test]
fn the_result_records_the_profile_and_the_version_of_the_library() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, _, dir) = go(&scenario("dsl-good", "", TWO_STREAMS), "profile-record");
    assert_eq!(
        result.scenario.profiles.get("access").map(String::as_str),
        Some("dsl-good")
    );
    assert_eq!(result.scenario.profile_library, Some(library().version()));
    assert!(
        result
            .scenario
            .profiles_line()
            .unwrap()
            .contains("access=dsl-good (library 1)")
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// `geo-good` has a rate downstream only: a stream above it receives what the rate allows, in the units of the frames (the frame
/// of 1,472 bytes of payload is 1,514 bytes: the payload, 8 of UDP, 20 of IP and 14 of Ethernet), and upstream is not limited.
/// Offered: 160 mbit downstream. The model is 144 mbit / (1,514 x 8) = 11,889 packets a second; over the 2 s window the bound is the
/// model plus or minus the bucket of the class and the packets at the two edges (4), and what the edges move on a busy machine.
#[test]
fn the_rate_of_geo_good_is_the_rate_of_the_link_downstream() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let streams = "  - { id: down, kind: udp-cbr, from: network, to: terminal, rate: 160mbit, packet-size: 1472b }\n";
    let (_, records, dir) = go(&scenario("geo-good", "", streams), "profile-rate");
    let begin = records[0].iter().map(|r| r.send_ns).min().unwrap();
    // The delay of 298.5 ms comes first; the window is two seconds inside the run
    let from = begin + 298_500_000 + 500_000_000;
    let to = from + 2_000_000_000;
    let arrived = records[0]
        .iter()
        .filter(|r| r.receive_ns >= from && r.receive_ns < to)
        .count() as f64;
    let model = 144e6 / (1514.0 * 8.0) * 2.0;
    // 5 ms for each edge of the window, as the packets of a busy machine are stamped late
    let room = 4.0 + 144e6 / (1514.0 * 8.0) * 0.010;
    assert!(
        (arrived - model).abs() <= room,
        "{arrived} packets arrived in two seconds, the model says {model} (room {room})"
    );
    // Canary: a link without the rate would deliver the offered 160 mbit, 11 % more, far outside the room
    assert!(160e6 / (1514.0 * 8.0) * 2.0 - model > 2.0 * room);
    let _ = std::fs::remove_dir_all(dir);
}

fn sh(script: &str) -> netscenario_core::Program {
    netscenario_core::Program::new(vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()])
}

/// The loss of a profile is too small to count in a run (0.025 % of 60 packets), so it is read from the queue of the interface, where
/// `netem` keeps it: the percentage `tc` prints is the one of the profile, within what a probability of 32 bits can show.
#[test]
fn the_loss_of_a_profile_is_in_the_queue_of_the_interface() {
    if !namespaces_available() {
        return;
    }
    for (id, declared) in [
        ("geo-good", 0.025),
        ("datacenter-rack", 0.000_655),
        ("datacenter-fabric", 0.003_775),
    ] {
        let dir = work_dir(&format!("profile-loss-{id}"));
        let mut backend = backend(|_| {});
        let scenario = scenario(id, "", TWO_STREAMS);
        let built = netscenario_core::provision(&mut backend, &scenario).unwrap();
        let tc = support::tools().tc;
        let mut shown = Vec::new();
        for (index, (node, file)) in [
            (built.nodes[0].1, "terminal.txt"),
            (built.nodes[1].1, "network.txt"),
        ]
        .into_iter()
        .enumerate()
        {
            let script = format!(
                "{} qdisc show dev l0 > {}/{file}",
                tc.display(),
                dir.display()
            );
            netscenario_core::Backend::start_program(&mut backend, node, &sh(&script)).unwrap();
            let text = support::wait_for(&dir.join(file), "netem");
            shown.push((index, text));
        }
        for (index, text) in shown {
            let percent = text
                .split_whitespace()
                .skip_while(|word| *word != "loss")
                .find_map(|word| word.strip_suffix('%')?.parse::<f64>().ok())
                .unwrap_or_else(|| panic!("{id}: no loss in: {text}"));
            assert!(
                (percent - declared).abs() <= declared * 0.02,
                "{id}, node {index}: the queue shows {percent} %, the profile says {declared} %: {text}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
