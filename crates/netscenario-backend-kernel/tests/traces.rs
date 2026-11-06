//! Trace-driven links in a real run (LMD-03, D-091): the rate of a link follows a staircase from a file, a Mahimahi file of a
//! known rate gives that rate, a rate that changes faster than the backend follows is refused unless the scenario accepts the
//! restriction (and the result says it did), and the delay and the loss of a trace act in their phases.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Counts of packets and seconds in nanoseconds are far below the precision of a double
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    // Text for a file is built a piece at a time
    clippy::format_push_string
)]

mod support;

use std::time::Duration;

use netscenario_agent::records::{self, Record};
use netscenario_core::{Error, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

/// The frame of a packet of 1,000 bytes of payload: the payload, 8 of UDP, 20 of IP and 14 of Ethernet.
const FRAME_1000: f64 = 1042.0;

/// A scenario with this link (the lines under `links`), one stream from the client to the server, and the files the link names.
fn scenario(
    link: &str,
    duration: &str,
    rate: &str,
    packet: &str,
    head: &str,
    files: &[(&str, String)],
) -> Scenario {
    let yaml = format!(
        "version: 0\nname: traces-test\n{head}duration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: {rate}, packet-size: {packet} }}\n"
    );
    let mut scenario = Scenario::from_yaml(&yaml, "traces-test.yaml").unwrap();
    scenario.measure.packets = true;
    let problems = scenario.resolve_files(|name| {
        files
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, text)| text.clone().into_bytes())
            .ok_or_else(|| format!("no such file {name}"))
    });
    assert!(problems.is_empty(), "{problems:?}");
    scenario
}

fn go(
    scenario: &Scenario,
    label: &str,
) -> Result<(RunResult, Vec<Record>, std::path::PathBuf), Error> {
    let dir = work_dir(label);
    let mut backend = backend(|_| {});
    let mut options = RunOptions::new(agent_exe().unwrap(), &dir);
    options.settle = Some(Duration::from_secs(1));
    let result = run(&mut backend, scenario, &options)?;
    assert_nothing_left(backend.run_id(), "after the run");
    let records =
        records::read(std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap())
            .unwrap()
            .records;
    Ok((result, records, dir))
}

/// How far the edges of a step may be moved on a busy machine, in seconds: the steps are done before the first packet that the sender
/// sends at or after their moment, so a step is as late as the sender was, which the result records for each step; the arrivals are
/// stamped by the receiver, which can be 5 ms late at each end of a window. A quiet machine gives a few milliseconds.
fn slack(result: &RunResult) -> f64 {
    let latest = result
        .events
        .iter()
        .map(|event| event.applied_ns.saturating_sub(event.scheduled_ns))
        .max()
        .unwrap_or(0);
    latest as f64 / 1e9 + 0.005
}

/// The moment the measured phase began, on the clock of the records: the first packet is sent at the start.
fn start_of(records: &[Record]) -> u64 {
    records
        .iter()
        .map(|r| r.send_ns)
        .min()
        .expect("a packet arrived")
}

/// A staircase between 6 and 2 mbit with steps of 500 ms, from a CSV file, is the rate of the link: the bytes that arrive in each
/// step are the rate times the time, in the units of the frames. 7 mbit of payload are offered, more than the link takes, so the
/// queue is never empty and what leaves it is the rate. The bound for a step is the model (rate x 0.5 s x 1000 / 1042 of the
/// payload) plus or minus what the edges can move: the bucket of the class (1,600 bytes), two frames that are cut by the edge
/// (2 x 1,042), and the packet that waits for the barrier of the step (up to a gap of 1.2 ms of the fastest rate, 900 bytes), so
/// 5,000 bytes. A netem rate would deliver 75 % of the bytes of the staircase (D-070), a deviation of 100,000 bytes and more.
#[test]
fn a_staircase_of_rates_from_a_csv_file_is_followed_step_by_step() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let csv = "time,rate\n0ms,6mbit\n500ms,2mbit\n1000ms,6mbit\n1500ms,2mbit\n2000ms,6mbit\n2500ms,2mbit\n3000ms,6mbit\n3500ms,2mbit\n"
        .to_owned();
    let link = "      forward: { delay: 10ms }\n      model:\n        kind: trace\n        repeat: false\n        forward: { file: staircase.csv, period: 4s }\n";
    let (result, records, dir) = go(
        &scenario(link, "4s", "7mbit", "1000b", "", &[("staircase.csv", csv)]),
        "traces-staircase",
    )
    .unwrap();
    assert_eq!(result.accepted_restrictions, []);
    let begin = start_of(&records);
    // 5,000 bytes, and the bytes of the fastest rate in the time that an edge can be moved
    let room = 5000.0 + 6e6 / 8.0 * slack(&result);
    for step in 0..8_u64 {
        let rate = if step % 2 == 0 { 6e6 } else { 2e6 };
        // The packets that left the shaper in this step arrive 10 ms later
        let from = begin + 10_000_000 + step * 500_000_000;
        let to = from + 500_000_000;
        // Skip the first and the last step: the first starts with an empty queue, the last ends with the end of the stream
        if step == 0 || step == 7 {
            continue;
        }
        let bytes: f64 = records
            .iter()
            .filter(|r| r.receive_ns >= from && r.receive_ns < to)
            .count() as f64
            * 1000.0;
        let expected = rate * 0.5 / 8.0 * 1000.0 / FRAME_1000;
        assert!(
            (bytes - expected).abs() < room,
            "step {step}: {bytes} bytes of payload arrived, the model says {expected} (room {room})"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A Mahimahi file of one delivery opportunity in every millisecond is a link of 12.112 mbit: with packets whose frames are the
/// opportunity (1,472 bytes of payload) 1,000 packets arrive in a second. Offered: 14 mbit. The bound is the model, 2,000 packets
/// in 2 s, plus or minus the bucket of the class (1 packet) and the packets at the two edges of the window (2): 4.
#[test]
fn a_mahimahi_file_of_a_known_rate_gives_that_rate() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let trace: String = (1..=1000).fold(String::new(), |mut text, ms| {
        text.push_str(&format!("{ms}\n"));
        text
    });
    // The second step of the trace is the same rate: the link is built through the fabric because the rate is set
    let link = "      forward: { delay: 5ms }\n      model:\n        kind: trace\n        forward: { file: steady.up, format: mahimahi }\n";
    let (result, records, dir) = go(
        &scenario(link, "4s", "14mbit", "1472b", "", &[("steady.up", trace)]),
        "traces-mahimahi-steady",
    )
    .unwrap();
    assert_eq!(result.accepted_restrictions, []);
    let begin = start_of(&records);
    let (from, to) = (begin + 1_005_000_000, begin + 3_005_000_000);
    let arrived = records
        .iter()
        .filter(|r| r.receive_ns >= from && r.receive_ns < to)
        .count() as i64;
    let room = 4.0 + 1000.0 * slack(&result);
    assert!(
        ((arrived - 2000) as f64).abs() <= room,
        "{arrived} packets arrived in two seconds, the file says 2000 (room {room})"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A Mahimahi file that alternates between one and two opportunities per millisecond in windows of 100 ms is followed: over ten
/// windows (1 s) the link delivers 1,500 packets of 1,472 bytes. The steps are 100 ms apart, which the Kernel backend follows
/// within 0.4 % (D-091): 6 packets, and the edges, 4: the bound is 12. Without the steps (a `netem` rate) the link would deliver
/// the rate of the first window to the end of the backlog.
#[test]
fn a_mahimahi_file_that_alternates_every_100_ms_is_followed() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let mut trace = String::new();
    for ms in 1..=1000_u64 {
        let window = (ms - 1) / 100;
        trace.push_str(&format!("{ms}\n"));
        if window % 2 == 1 {
            trace.push_str(&format!("{ms}\n"));
        }
    }
    let link = "      forward: { delay: 5ms }\n      model:\n        kind: trace\n        forward: { file: alternating.up, format: mahimahi }\n";
    let (result, records, dir) = go(
        &scenario(
            link,
            "4s",
            "25mbit",
            "1472b",
            "",
            &[("alternating.up", trace)],
        ),
        "traces-mahimahi-alternating",
    )
    .unwrap();
    assert!(
        result.accepted_restrictions.is_empty(),
        "100 ms is followed"
    );
    let begin = start_of(&records);
    let (from, to) = (begin + 1_005_000_000, begin + 3_005_000_000);
    let arrived = records
        .iter()
        .filter(|r| r.receive_ns >= from && r.receive_ns < to)
        .count() as i64;
    let room = 24.0 + 2000.0 * slack(&result);
    assert!(
        ((arrived - 3000) as f64).abs() <= room,
        "{arrived} packets arrived in two passes (2 s), the file says 3000 (room {room})"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A trace whose rate changes every 50 ms is followed only in part: it is refused unless the scenario lists `fast-rate-steps` in
/// `allow-limited`, the refusal names what to write, and with it the run goes and the result records the restriction. Delivered:
/// within 2 % under the model (the measure at 20 ms, D-091) and the edges above it. A trace of steps of 10 ms is refused even
/// then (canary: the acceptance of one feature is not the acceptance of another).
#[test]
fn a_fast_trace_needs_allow_limited_and_the_result_says_it_was_accepted() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let alternating = |window: u64| -> String {
        let mut text = String::new();
        for ms in 1..=1000_u64 {
            text.push_str(&format!("{ms}\n"));
            if ((ms - 1) / window) % 2 == 1 {
                text.push_str(&format!("{ms}\n"));
            }
        }
        text
    };
    let link = |window: &str| {
        format!(
            "      forward: {{ delay: 5ms }}\n      model:\n        kind: trace\n        forward: {{ file: fast.up, format: mahimahi, window: {window} }}\n"
        )
    };
    let files = [("fast.up", alternating(50))];
    // Without the acceptance
    let error = go(
        &scenario(&link("50ms"), "3s", "25mbit", "1472b", "", &files),
        "traces-fast-refused",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("Rate steps closer than 100 ms")
            && error.contains("`allow-limited: [fast-rate-steps]`"),
        "{error}"
    );
    // With it
    let (result, records, dir) = go(
        &scenario(
            &link("50ms"),
            "3s",
            "25mbit",
            "1472b",
            "allow-limited: [fast-rate-steps]\n",
            &files,
        ),
        "traces-fast-accepted",
    )
    .unwrap();
    let [accepted] = result.accepted_restrictions.as_slice() else {
        panic!("{:?}", result.accepted_restrictions);
    };
    assert_eq!(accepted.feature, "fast-rate-steps");
    assert!(accepted.restriction.contains("2 % fewer"), "{accepted:?}");
    let begin = start_of(&records);
    // Two passes of the trace (2 s) between 0.5 s and 2.5 s: 1,500 packets of the model per second
    let (from, to) = (begin + 505_000_000, begin + 2_505_000_000);
    let arrived = records
        .iter()
        .filter(|r| r.receive_ns >= from && r.receive_ns < to)
        .count() as f64;
    let room = 6.0 + 2000.0 * slack(&result);
    assert!(
        (3000.0 * 0.98 - room..=3000.0 + room).contains(&arrived),
        "{arrived} packets arrived, the model says 3000 (up to 2 % fewer, room {room})"
    );
    let _ = std::fs::remove_dir_all(dir);
    // Canary: the acceptance of one feature does not accept another, and a feature that cannot be had is not accepted at all
    let finer = [("fast.up", alternating(10))];
    let error = go(
        &scenario(
            &link("10ms"),
            "3s",
            "25mbit",
            "1472b",
            "allow-limited: [fast-rate-steps]\n",
            &finer,
        ),
        "traces-too-fast",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("Rate steps closer than 20 ms") && error.contains("at least 20 ms"),
        "{error}"
    );
}

/// A row without a delivery is a stall, which a shaper cannot hold exactly: refused unless `stalled-link` is accepted, and then
/// one frame is delivered in the row. Offered 25 mbit against 2 packets per millisecond (24.2 mbit): in 1 s the trace has nine
/// windows of 200 packets and one of none, which is 1 packet here. Over the two passes from the first to the last arrival, 3,602
/// packets arrive plus or minus the bucket and the edges (8).
///
/// A stall may last one row longer than the trace says, and that is the restriction: the frame of the stall is delivered at the
/// start of the row and costs a row at the rate of the stall; a second frame is eligible a row later, and if the step that ends the
/// stall is applied later than that (a fraction of a millisecond is enough, and under load it is tens), the second frame leaves at
/// the rate of the stall and its cost, one more row, is still to be paid when the rate has returned (measured under load: the
/// arrivals of a pass show 11, 1 and 189 packets in the bins around the stall where 11 and 190 are the rule). Each of the two stalls
/// in the window can lose one row of packets (200) so the deficit is at most 400; a surplus is not possible but by the bucket and
/// by a step applied late at the start of the stall, which is the lateness of the steps in the result (`slack`) times the rate for
/// each of the two. A link that did not stall at all would deliver 4,000, 398 above the model and beyond the surplus allowed.
#[test]
fn a_row_without_a_delivery_is_a_stall_that_the_scenario_must_accept() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let mut trace = String::new();
    for ms in 1..=1000_u64 {
        let window = (ms - 1) / 100;
        if window == 4 {
            continue;
        }
        trace.push_str(&format!("{ms}\n{ms}\n"));
    }
    let last = "1000\n";
    assert!(trace.ends_with(last) || trace.ends_with("1000\n1000\n"));
    let link = "      forward: { delay: 5ms }\n      model:\n        kind: trace\n        forward: { file: gap.up, format: mahimahi }\n";
    let files = [("gap.up", trace)];
    let error = go(
        &scenario(link, "3s", "25mbit", "1472b", "", &files),
        "traces-stall-refused",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("A trace row in which nothing is delivered")
            && error.contains("`allow-limited: [stalled-link]`"),
        "{error}"
    );
    let (result, records, dir) = go(
        &scenario(
            link,
            "3s",
            "25mbit",
            "1472b",
            "allow-limited: [stalled-link]\n",
            &files,
        ),
        "traces-stall-accepted",
    )
    .unwrap();
    assert_eq!(result.accepted_restrictions[0].feature, "stalled-link");
    let begin = start_of(&records);
    // The three seconds are three passes; one pass delivers 1,801 packets (nine windows of 200 and one frame)
    let (from, to) = (begin + 1_005_000_000, begin + 3_005_000_000);
    let arrived = records
        .iter()
        .filter(|r| r.receive_ns >= from && r.receive_ns < to)
        .count() as i64;
    let late = 2.0 * 2000.0 * slack(&result);
    let (upper, lower) = (3602.0 + 8.0 + late, 3602.0 - 8.0 - 400.0 - late);
    // Canary: the surplus allowed leaves out the 4,000 of a link that does not stall, so the bound can fail
    assert!(
        upper < 4000.0,
        "the lateness of the steps ({late}) makes the bound useless"
    );
    assert!(
        (lower..=upper).contains(&(arrived as f64)),
        "{arrived} packets arrived in two passes, the model says 3602 (at most {upper}, at least {lower}); a link that does not stall delivers 4000"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The delay and the loss of a trace act in their phases: a delay of 20 ms, then 80 ms, then a loss of 30 % at 20 ms. At 1 mbit of
/// payload (125 packets a second) nothing queues, so the delay of a packet is the delay of its phase plus a frame time (1042
/// bytes at no rate: the veth). The delay is never below the trace's (the physical bound) and the median is within 3 ms of it. The
/// loss: 250 packets in the third phase, mean 75, four standard deviations of the binomial (7.2) are 29: between 46 and 104; the
/// other phases lose none.
#[test]
fn the_delay_and_the_loss_of_a_trace_act_in_their_phases() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let csv = "time,delay,loss\n0s,20ms,0%\n2s,80ms,0%\n4s,20ms,30%\n".to_owned();
    let link = "      forward: { delay: 1ms }\n      model:\n        kind: trace\n        repeat: false\n        forward: { file: phases.csv, period: 6s }\n";
    let (_, records, dir) = go(
        &scenario(
            link,
            "6s",
            "1mbit",
            "1000b",
            "seed: 11\n",
            &[("phases.csv", csv)],
        ),
        "traces-delay-loss",
    )
    .unwrap();
    let begin = start_of(&records);
    let phase = |from_s: u64, to_s: u64| -> Vec<&Record> {
        records
            .iter()
            .filter(|r| {
                r.send_ns >= begin + from_s * 1_000_000_000 + 100_000_000
                    && r.send_ns < begin + to_s * 1_000_000_000 - 100_000_000
            })
            .collect()
    };
    let median = |records: &[&Record], low_ms: f64| {
        let mut delays: Vec<f64> = records.iter().map(|r| r.delay_ns() as f64 / 1e6).collect();
        delays.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            delays[0] >= low_ms,
            "a packet was faster than the delay of the phase: {} < {low_ms}",
            delays[0]
        );
        delays[delays.len() / 2]
    };
    let first = phase(0, 2);
    let second = phase(2, 4);
    assert!((median(&first, 20.0) - 20.0).abs() < 3.0);
    assert!((median(&second, 80.0) - 80.0).abs() < 3.0);
    // 125 packets a second, 1.8 s in each phase window: 225 sent, all arrived in the first two
    assert!(
        first.len() >= 220 && second.len() >= 220,
        "{} {}",
        first.len(),
        second.len()
    );
    let sent_in_third = (125.0 * 1.8) as i64;
    let third = phase(4, 6);
    let lost = sent_in_third - third.len() as i64;
    assert!(
        (lost - 68).abs() < 27,
        "{lost} of {sent_in_third} packets lost in the phase of 30 %"
    );
    let _ = std::fs::remove_dir_all(dir);
}
