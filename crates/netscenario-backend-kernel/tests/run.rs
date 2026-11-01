//! A whole run: from a scenario to a result, in real namespaces (WRK-01, PRB-01 to PRB-03, PRB-05, ENV-01,
//! ENV-02, RPT-01). The scenario is written as a person would write it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Delays of a few milliseconds and rates of a few megabits are far below the precision of a double
    clippy::cast_precision_loss
)]

mod support;

use std::path::Path;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use netscenario_core::{CancelToken, Error, RunEvent, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario(head: &str, link: &str, workloads: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: media-test\n{head}duration: 2s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n{workloads}"
        ),
        "media-test.yaml",
    )
    .unwrap()
}

const MEDIA: &str =
    "  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }\n";

fn options(dir: &Path, packets: bool) -> RunOptions {
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.export_packets = packets;
    options
}

fn lost_in(dir: &Path, workload: &str) -> Vec<u64> {
    std::fs::read_to_string(dir.join(format!("packets-{workload}.jsonl")))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["lost"] == true)
        .map(|v| v["sequence"].as_u64().unwrap())
        .collect()
}

#[test]
fn a_run_measures_the_link_and_writes_a_result() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("run-delay");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario("", "      forward: { delay: 20ms }\n", MEDIA),
        &options(&dir, false),
    )
    .unwrap();

    let media = &result.workloads[0];
    // 2 mbit with 1200 byte payloads is 208.33 packets per second: 417 in two seconds
    assert_eq!(media.stats.sent, 417);
    assert_eq!(
        media.stats.received, 417,
        "a link without loss loses nothing"
    );
    assert_eq!(
        (
            media.stats.lost,
            media.stats.duplicates,
            media.foreign_datagrams
        ),
        (0, 0, 0)
    );
    assert!(media.stats.loss_ratio.abs() < f64::EPSILON);
    assert_eq!(media.sender.send_errors, 0);
    let delay = media.stats.delay.as_ref().unwrap();
    assert!(
        (20_000_000..22_000_000).contains(&delay.p50_ns),
        "the median delay is {} ns",
        delay.p50_ns
    );
    assert!(
        delay.min_ns >= 20_000_000,
        "faster than the link: {} ns",
        delay.min_ns
    );
    // One packet may be held up by a busy machine for some milliseconds, but not by a hundred
    // (the exact deviations are for the fidelity tests, QA-06; the first packets not waiting for ARP is
    // checked in `agent.rs`)
    assert!(
        delay.max_ns < 120_000_000,
        "the slowest packet took {} ns",
        delay.max_ns
    );
    // The first second carries about the rate of the workload
    let first = media.stats.throughput.bits_per_second[0];
    assert!(
        (1_800_000.0..2_100_000.0).contains(&first),
        "{first} bit/s in the first second"
    );

    // Where it came from
    assert_eq!(result.provenance.backend, "kernel");
    assert!(result.provenance.backend_details["tc"].starts_with("iproute2 "));
    assert!(
        result
            .provenance
            .kernel
            .as_deref()
            .is_some_and(|k| !k.is_empty())
    );
    assert!(result.provenance.cpus.is_some_and(|n| n > 0));
    assert!(result.provenance.started_at.ends_with('Z'));
    assert_eq!(
        result
            .addresses
            .iter()
            .map(|a| a.address.as_str())
            .collect::<Vec<_>>(),
        ["10.100.0.1", "10.100.0.2"]
    );
    assert_eq!(
        (media.from_address.as_str(), media.to_address.as_str()),
        ("10.100.0.1", "10.100.0.2:5000")
    );

    // A scenario without a seed got one that was drawn, so the run can be repeated
    assert!(result.seed.drawn && result.seed.value.is_some() && result.seed.loss_reproducible);

    // What is on disk is what came back, and the raw records are next to it
    let written = std::fs::read_to_string(dir.join("result.json")).unwrap();
    assert_eq!(RunResult::from_json(&written).unwrap(), result);
    assert!(dir.join(&media.records_file).is_file());
    assert!(media.packets_file.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// The same seed loses the same packets, another seed other packets (ENV-01), as the per-packet export shows.
#[test]
fn the_same_seed_gives_the_same_lost_packets() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let lossy = "      forward: { loss: { model: random, p: 20% } }\n";
    let lost = |seed: u32, label: &str| -> (Vec<u64>, RunResult) {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario(&format!("seed: {seed}\n"), lossy, MEDIA),
            &options(&dir, true),
        )
        .unwrap();
        let lost = lost_in(&dir, "media");
        let _ = std::fs::remove_dir_all(dir);
        (lost, result)
    };
    let (first, result) = lost(42, "run-seed-a");
    let (again, _) = lost(42, "run-seed-b");
    let (other, _) = lost(43, "run-seed-c");

    assert_eq!(result.seed.value, Some(42));
    assert!(!result.seed.drawn);
    // 417 packets at 20 percent: about 83 lost; the band is wide on purpose, far from none and from all
    let media = &result.workloads[0];
    assert_eq!(usize::try_from(media.stats.lost).unwrap(), first.len());
    assert!(
        (0.10..0.30).contains(&media.stats.loss_ratio),
        "{} of {} packets were lost",
        media.stats.lost,
        media.stats.sent
    );
    assert_eq!(first, again, "the same seed must lose the same packets");
    assert_ne!(first, other, "another seed must lose other packets");
    assert_queues_saw_the_workloads(&result, 417);
}

/// What a seed repeats in a direction with a burst loss, a duplication and a reordering (D-054): the packets that
/// were lost, the ones that arrived twice, and the ones that `netem` sent on at once.
fn drawn_by(dir: &Path, workload: &str) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let lines: Vec<serde_json::Value> =
        std::fs::read_to_string(dir.join(format!("packets-{workload}.jsonl")))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let sequences = |keep: &dyn Fn(&serde_json::Value) -> bool| -> Vec<u64> {
        lines
            .iter()
            .filter(|v| keep(v))
            .map(|v| v["sequence"].as_u64().unwrap())
            .collect()
    };
    (
        sequences(&|v| v["lost"] == true),
        sequences(&|v| v["duplicate"] == true),
        // The held packets wait 10 ms, the others arrive within the time of the rate (2 ms at most)
        sequences(&|v| {
            v["duplicate"] != true && v["delay_ns"].as_u64().is_some_and(|d| d < 5_000_000)
        }),
    )
}

/// The same seed repeats a loss in bursts, the duplication and the reordering, in a direction of two queues; another
/// seed does not. The queue check (ENV-07) does not apply to a duplicating direction: the queue counts the copies.
#[test]
fn the_same_seed_repeats_burst_loss_duplication_and_reordering() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let link = "      forward: { delay: 10ms, loss: { model: gilbert-elliott, p: 10%, r: 30%, loss-bad: 100%, loss-good: 0% }, duplicate: 20%, reorder: { share: 50% }, rate: 10mbit }\n";
    let drawn = |seed: u32, label: &str| {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario(&format!("seed: {seed}\n"), link, MEDIA),
            &options(&dir, true),
        )
        .unwrap();
        let drawn = drawn_by(&dir, "media");
        let _ = std::fs::remove_dir_all(dir);
        (drawn, result)
    };
    let ((lost, doubled, early), result) = drawn(7, "run-models-a");
    let ((lost_again, doubled_again, early_again), _) = drawn(7, "run-models-b");
    let ((lost_other, doubled_other, early_other), _) = drawn(8, "run-models-c");
    // Something of each happened, so that "the same" means something
    assert!(!lost.is_empty() && !doubled.is_empty() && !early.is_empty());
    assert_eq!(lost, lost_again, "the same seed must lose the same packets");
    assert_eq!(
        doubled, doubled_again,
        "the same seed must duplicate the same packets"
    );
    assert_eq!(
        early, early_again,
        "the same seed must send the same packets on at once"
    );
    assert!(lost != lost_other && doubled != doubled_other && early != early_other);
    assert!(result.seed.loss_reproducible);
    assert!(
        result.seed.queue_checks.is_empty(),
        "a duplicating direction is not checked: {:?}",
        result.seed.queue_checks
    );
}

/// The same seed chooses the same packets for the corruption and the loss, also in a direction of two queues, and
/// the tally of the direction adds up to the last packet: lost in the queue, dropped by the node as damaged, or
/// delivered (D-040). What is **not** the same from run to run is where `netem` flips the bit, because it takes that
/// from the generator of the kernel and not from the seed: a bit in the source address of the Ethernet header harms
/// nothing (6 of 1,242 bytes here), so a corrupted packet now and then arrives in one run and is dropped in another.
/// So the packets lost in the queue are exactly the same, the packets lost in all are the same but for a few, and
/// the result says that the run is not reproducible and why.
#[test]
fn the_same_seed_chooses_the_same_packets_to_corrupt_and_every_packet_is_accounted_for() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let link = "      forward: { delay: 10ms, jitter: { amount: 2ms }, loss: { model: random, p: 10% }, corrupt: 10%, rate: 10mbit }\n";
    let drawn = |seed: u32, label: &str| {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario(&format!("seed: {seed}\n"), link, MEDIA),
            &options(&dir, true),
        )
        .unwrap();
        let lost = lost_in(&dir, "media");
        let _ = std::fs::remove_dir_all(dir);
        (lost, result)
    };
    let (lost, result) = drawn(7, "run-corrupt-a");
    let (lost_again, again) = drawn(7, "run-corrupt-b");
    let (lost_other, other) = drawn(8, "run-corrupt-c");
    let tally = |result: &RunResult| {
        let [tally] = result.directions.as_slice() else {
            panic!("expected one direction, got {}", result.directions.len());
        };
        tally.clone()
    };
    let (tally, tally_again, tally_other) = (tally(&result), tally(&again), tally(&other));
    assert_eq!(tally.direction, "forward");
    // The loss is seeded and in front of the corruption: the queue loses the same packets, and another seed others
    assert_eq!(tally.lost_in_queue, tally_again.lost_in_queue);
    assert_ne!(tally.lost_in_queue, tally_other.lost_in_queue);
    assert_ne!(lost, lost_other);
    // The lost packets are the same but for the corrupted ones whose bit fell where it harms nothing. 41 packets
    // are corrupted, each harmless with a chance of about 0.5 %, so more than three differences in a pair of runs is
    // far beyond chance (about 1 in 100,000).
    let (a, b): (std::collections::BTreeSet<_>, std::collections::BTreeSet<_>) =
        (lost.iter().collect(), lost_again.iter().collect());
    let differing = a.symmetric_difference(&b).count();
    assert!(
        differing <= 3,
        "{differing} packets differ: {lost:?} against {lost_again:?}"
    );
    assert!(
        tally
            .dropped_corrupted
            .abs_diff(tally_again.dropped_corrupted)
            <= 3,
        "{} against {}",
        tally.dropped_corrupted,
        tally_again.dropped_corrupted
    );
    let in_queue = tally.lost_in_queue.expect("no duplication here");
    assert!(
        in_queue > 0 && tally.dropped_corrupted > 0,
        "something of each: {} lost in the queue, {} dropped by the node",
        in_queue,
        tally.dropped_corrupted
    );
    for (tally, result) in [
        (&tally, &result),
        (&tally_again, &again),
        (&tally_other, &other),
    ] {
        let in_queue = tally.lost_in_queue.expect("no duplication here");
        assert_eq!(
            in_queue + tally.dropped_corrupted + tally.delivered + tally.receive_buffer_drops,
            tally.sent,
            "the packets of the direction must add up"
        );
        // The program sees the sum of both as loss
        assert_eq!(
            result.workloads[0].stats.lost,
            in_queue + tally.dropped_corrupted
        );
        // The queue saw exactly the packets of the workload: the only reason for the warning is the corruption
        assert_eq!(
            result.seed.queue_checks,
            [netscenario_core::QueueCheck::new(
                "access", "forward", tally.sent, tally.sent
            )]
        );
        // The result does not promise what it cannot keep, and says why
        assert!(!result.seed.loss_reproducible);
        assert!(
            result
                .seed
                .not_reproducible_because
                .iter()
                .any(|reason| reason.contains("corrupts packets")),
            "{:?}",
            result.seed.not_reproducible_because
        );
    }
}

/// The loss stays in front of the rate in a direction that is built from two qdiscs (a jitter and a rate), so
/// the same seed still loses the same packets there (D-030).
#[test]
fn the_same_seed_loses_the_same_packets_through_a_jitter_and_a_rate() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let link = "      forward: { delay: 20ms, jitter: { amount: 2ms }, loss: { model: random, p: 20% }, rate: 10mbit }\n";
    let lost = |label: &str| -> (Vec<u64>, RunResult) {
        let dir = work_dir(label);
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario("seed: 5\n", link, MEDIA),
            &options(&dir, true),
        )
        .unwrap();
        let lost = lost_in(&dir, "media");
        let _ = std::fs::remove_dir_all(dir);
        (lost, result)
    };
    let (first, result) = lost("run-two-stages-a");
    let (again, _) = lost("run-two-stages-b");
    assert_eq!(first, again, "the same seed must lose the same packets");
    let media = &result.workloads[0];
    assert!(
        (0.10..0.30).contains(&media.stats.loss_ratio),
        "{} of {} packets were lost",
        media.stats.lost,
        media.stats.sent
    );
    // The jitter reached the packets that were kept: they were not all delayed by exactly 20 ms
    let delay = media.stats.delay.as_ref().unwrap();
    assert!(delay.max_ns - delay.min_ns > 2_000_000, "{delay:?}");
    assert_queues_saw_the_workloads(&result, 417);
}

/// ENV-07: the queue of the direction with loss counted the 417 packets of the workload, none more, and the
/// run says that the loss is reproducible. The reverse direction has no loss and is not counted.
fn assert_queues_saw_the_workloads(result: &RunResult, packets: u64) {
    assert!(result.seed.loss_reproducible, "{:?}", result.seed);
    assert_eq!(
        result.seed.queue_checks,
        [netscenario_core::QueueCheck::new(
            "access", "forward", packets, packets
        )]
    );
}

/// Canary: a packet that no workload sent (here from a program of the test, in the node of the sender, into
/// the same link) enters the queue, shifts the pattern of lost packets, and the run says so.
#[test]
fn a_packet_that_no_workload_sent_makes_the_run_not_reproducible() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("run-extra-packet");
    let mut backend = Disturbed(backend(|_| {}));
    let lossy = "      forward: { loss: { model: random, p: 20% } }\n";
    let result = run(
        &mut backend,
        &scenario("seed: 42\n", lossy, MEDIA),
        &options(&dir, false),
    )
    .unwrap();
    assert!(!result.seed.loss_reproducible, "{:?}", result.seed);
    let check = &result.seed.queue_checks[0];
    assert_eq!(check.direction, "forward");
    assert_eq!(
        check.counted,
        check.expected + 1,
        "exactly the one extra packet: {check:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A Kernel backend that, when the sender of a workload starts, also puts one more datagram into the link
/// from the same node: the packet that the check has to find.
struct Disturbed(netscenario_backend_kernel::KernelBackend);

impl netscenario_core::Backend for Disturbed {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn capabilities(&self) -> netscenario_core::Capabilities {
        self.0.capabilities()
    }
    fn create_node(
        &mut self,
        name: &netscenario_model::Name,
    ) -> Result<netscenario_core::NodeHandle, Error> {
        self.0.create_node(name)
    }
    fn create_link(
        &mut self,
        name: &netscenario_model::Name,
        between: [netscenario_core::NodeHandle; 2],
    ) -> Result<netscenario_core::LinkHandle, Error> {
        self.0.create_link(name, between)
    }
    fn apply(
        &mut self,
        link: netscenario_core::LinkHandle,
        direction: netscenario_core::Direction,
        impairments: &netscenario_model::Impairments,
        seed: Option<u64>,
    ) -> Result<(), Error> {
        self.0.apply(link, direction, impairments, seed)
    }
    fn start_program(
        &mut self,
        node: netscenario_core::NodeHandle,
        program: &netscenario_core::Program,
    ) -> Result<netscenario_core::ProgramHandle, Error> {
        if program.argv.get(1).map(String::as_str) == Some("send") {
            // One datagram of the agent's own kind to a port nobody listens on: the receiver counts it as
            // foreign, the queue counts it as a packet
            let mut extra = program.clone();
            let to = extra.argv[extra.argv.iter().position(|a| a == "--to").unwrap() + 1]
                .replace(":5000", ":5999");
            for (name, value) in [
                ("--flow", "99"),
                ("--to", to.as_str()),
                ("--rate", "1000"),
                ("--size", "125"),
                ("--duration-ns", "1000000000"),
            ] {
                let at = extra.argv.iter().position(|a| a == name).unwrap();
                value.clone_into(&mut extra.argv[at + 1]);
            }
            self.0.start_program(node, &extra)?;
        }
        self.0.start_program(node, program)
    }
    fn address(
        &self,
        link: netscenario_core::LinkHandle,
        end: usize,
    ) -> Result<std::net::Ipv4Addr, Error> {
        self.0.address(link, end)
    }
    fn describe(&self) -> netscenario_core::BackendInfo {
        self.0.describe()
    }
    fn queue_packets(
        &mut self,
        link: netscenario_core::LinkHandle,
        direction: netscenario_core::Direction,
    ) -> Result<Option<u64>, Error> {
        self.0.queue_packets(link, direction)
    }
    fn poll_program(
        &mut self,
        program: netscenario_core::ProgramHandle,
        wait: Duration,
    ) -> Result<netscenario_core::ProgramState, Error> {
        self.0.poll_program(program, wait)
    }
    fn send_line(
        &mut self,
        program: netscenario_core::ProgramHandle,
        line: &str,
    ) -> Result<(), Error> {
        self.0.send_line(program, line)
    }
    fn teardown(&mut self) -> Result<(), Error> {
        self.0.teardown()
    }
}

#[test]
fn workloads_run_together_and_a_window_limits_one_of_them() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("run-two");
    let mut backend = backend(|_| {});
    let workloads = format!(
        "{MEDIA}  - {{ id: burst, kind: udp-cbr, from: server, to: client, rate: 1mbit, packet-size: 500b, start: 500ms, stop: 1500ms }}\n"
    );
    let result = run(
        &mut backend,
        &scenario("", "      both: { delay: 5ms }\n", &workloads),
        &options(&dir, false),
    )
    .unwrap();
    assert_eq!(result.workloads.len(), 2);
    let (media, burst) = (&result.workloads[0], &result.workloads[1]);
    assert_eq!((media.id.as_str(), burst.id.as_str()), ("media", "burst"));
    assert_eq!(media.stats.sent, 417);
    // 1 mbit with 500 byte payloads is 250 packets per second, for one second
    assert_eq!(
        (burst.start_ns, burst.duration_ns),
        (500_000_000, 1_000_000_000)
    );
    assert_eq!(burst.stats.sent, 250);
    assert_eq!(burst.stats.received, 250);
    // They went in opposite directions on their own ports
    assert_eq!(
        (burst.from_address.as_str(), burst.to_address.as_str()),
        ("10.100.0.2", "10.100.0.1:5001")
    );
    for workload in &result.workloads {
        let delay = workload.stats.delay.as_ref().unwrap();
        assert!(
            (5_000_000..8_000_000).contains(&delay.p50_ns),
            "{}: {} ns",
            workload.id,
            delay.p50_ns
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A real run tells its phases in order, and the progress once a second up to the full length.
#[test]
fn a_run_reports_its_phases_and_its_progress() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("run-events");
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let observer = move |event: &RunEvent| seen.lock().unwrap().push(event.clone());
    let options = options(&dir, false).with_observer(Arc::new(observer));
    let mut backend = backend(|_| {});
    run(&mut backend, &scenario("", "", MEDIA), &options).unwrap();

    let events = events.lock().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            RunEvent::Provisioned { .. } => "provisioned",
            RunEvent::ReceiversReady { .. } => "receivers-ready",
            RunEvent::Started { .. } => "started",
            RunEvent::Progress { .. } => "progress",
            RunEvent::Settling { .. } => "settling",
            RunEvent::Collecting => "collecting",
            RunEvent::TearingDown => "tearing-down",
            RunEvent::Finished => "finished",
            _ => "other",
        })
        .collect();
    // The scenario lasts two seconds: one report on the way, and the last at the full length
    let position = |name: &str| kinds.iter().position(|k| *k == name).unwrap();
    assert!(
        position("provisioned") < position("receivers-ready")
            && position("receivers-ready") < position("started")
            && position("started") < position("progress")
            && position("settling") < position("collecting")
            && position("collecting") < position("tearing-down")
            && position("tearing-down") < position("finished"),
        "{kinds:?}"
    );
    assert_eq!(kinds.last(), Some(&"finished"));
    let progress: Vec<(Duration, Duration)> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Progress { elapsed, total } => Some((*elapsed, *total)),
            _ => None,
        })
        .collect();
    assert!(progress.len() >= 2, "{progress:?}");
    assert!(
        progress
            .iter()
            .all(|(_, total)| *total == Duration::from_secs(2)),
        "{progress:?}"
    );
    assert!(
        progress.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "the progress goes back: {progress:?}"
    );
    assert_eq!(
        progress.last().map(|(elapsed, _)| *elapsed),
        Some(Duration::from_secs(2))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A run of ten seconds is cancelled after about one, from the observer (as an interrupt from another thread
/// would do): it returns `Cancelled` soon, the network is gone, and there is no result.
#[test]
fn a_run_can_be_cancelled_and_leaves_nothing() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("run-cancel");
    let token = CancelToken::new();
    let cancelled_at = Arc::new(Mutex::new(None::<Instant>));
    let (cancel, when) = (token.clone(), Arc::clone(&cancelled_at));
    let observer = move |event: &RunEvent| {
        if matches!(event, RunEvent::Progress { .. }) && when.lock().unwrap().is_none() {
            *when.lock().unwrap() = Some(Instant::now());
            cancel.cancel();
        }
    };
    let options = options(&dir, false)
        .with_observer(Arc::new(observer))
        .with_cancel(token);
    let long = Scenario::from_yaml(
        &format!(
            "version: 0\nname: long\nduration: 10s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\nworkloads:\n{MEDIA}"
        ),
        "long.yaml",
    )
    .unwrap();
    let mut backend = backend(|_| {});
    let began = Instant::now();
    let error = run(&mut backend, &long, &options).unwrap_err();
    let finished = Instant::now();

    assert!(matches!(error, Error::Cancelled), "{error}");
    let cancelled = cancelled_at
        .lock()
        .unwrap()
        .expect("the observer cancelled");
    // Back within a moment of the cancel (a poll of 200 ms and the teardown), long before the ten seconds
    assert!(
        finished - cancelled < Duration::from_secs(3),
        "{:?} from the cancel to the return",
        finished - cancelled
    );
    assert!(
        finished - began < Duration::from_secs(7),
        "{:?}",
        finished - began
    );
    assert!(!dir.join("result.json").exists());
    assert_nothing_left(backend.run_id(), "after the cancel");
    let _ = std::fs::remove_dir_all(dir);
}

/// A run that cannot start leaves nothing behind and says what is wrong.
#[test]
fn a_run_without_an_agent_fails_and_leaves_nothing() {
    if !namespaces_available() {
        return;
    }
    let dir = work_dir("run-noagent");
    let mut backend = backend(|_| {});
    let options = RunOptions::new("/nonexistent/netscenario-agent", &dir);
    let error = run(&mut backend, &scenario("", "", MEDIA), &options).unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
    // The network was built and is torn down again: no process of the run is left
    assert_nothing_left(backend.run_id(), "no agent");
    let _ = std::fs::remove_dir_all(dir);
}
