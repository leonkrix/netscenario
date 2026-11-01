//! The timeline in a real run (TML-01, D-060, D-062): a `set` changes the link while the workloads go, the result says
//! when, and the throughput shows it. A step is done by the clock or at a barrier of the workload; what the first costs
//! in reproducibility is in the result and tested here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Counts of packets and rates of a few megabits are far below the precision of a double
    clippy::cast_precision_loss
)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use netscenario_core::{
    Backend, CancelToken, Direction, Error, EventRecord, LinkHandle, NodeHandle, Program,
    ProgramHandle, RunEvent, RunOptions, run,
};
use netscenario_model::{Impairments, Name, Scenario};
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario(head: &str, link: &str, duration: &str, timeline: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: timeline-test\n{head}duration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: 3mbit, packet-size: 1000b }}\ntimeline:\n{timeline}"
        ),
        "timeline-test.yaml",
    )
    .unwrap()
}

fn options(dir: &std::path::Path) -> RunOptions {
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_secs(1));
    options
}

/// How late a step may be, at most. A step by the clock is due at a moment and done by a `tc` process through
/// `nsenter`: 2 ms on the development machine, a few on a busy one (S11, D-038, D-060). Noise only goes one way,
/// so the lower bound is exact (a step by the clock is never done before its moment) and the upper bound is room
/// for a shared runner: 50 ms is more than ten times what was measured with every processor busy.
const LATEST: Duration = Duration::from_millis(50);

/// The judgement of the lateness of the steps, in one place so that the canary below uses the same one. A step at a
/// barrier is done when the sender has sent the packet before its moment, so it may begin up to `gap_ns` before it.
fn lateness_problems(events: &[EventRecord], gap_ns: u64) -> Vec<String> {
    let mut problems = Vec::new();
    for event in events {
        let earliest = if event.packet.is_some() {
            event.scheduled_ns.saturating_sub(gap_ns)
        } else {
            event.scheduled_ns
        };
        if event.started_ns < earliest {
            problems.push(format!(
                "event {} began {} ns before it may",
                event.event,
                earliest - event.started_ns
            ));
        }
        let late = Duration::from_nanos(event.applied_ns.saturating_sub(event.scheduled_ns));
        if late > LATEST {
            problems.push(format!("event {} was {late:?} late", event.event));
        }
        if event.applied_ns < event.started_ns {
            problems.push(format!("event {} ended before it began", event.event));
        }
    }
    problems
}

/// The gap between two packets of 3 mbit in packets of 1,000 bytes: 8,000 bits every 2.667 ms.
const GAP_NS: u64 = 2_666_667;

/// A rate that drops while the workload goes is in the throughput: 3 mbit offered, a link of 4 mbit that is limited
/// to 1 mbit at 3 s. The frames of the link are the payload and 42 bytes, so 1 mbit of frames is 0.9597 mbit of
/// payload; before the drop all 3 mbit of payload come through. A bin is one second, a packet is 0.27 % of one before
/// and 0.83 % after, so 2 % around the number is room for the packets at the edges.
#[test]
fn a_rate_that_drops_in_the_course_of_the_run_is_in_the_throughput_and_the_result_says_when() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-rate");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "",
            "      forward: { delay: 10ms, rate: 4mbit }\n",
            "6s",
            "  - { at: 3s, link: access, set: { forward: { rate: 1mbit } } }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let bins = &result.workloads[0].stats.throughput.bits_per_second;
    let mbit = |bin: usize| bins[bin] / 1e6;
    // Seconds 0 to 2 before the drop, 3 and 4 after (the sixth second is only partly filled)
    for bin in 0..3 {
        assert!(
            (mbit(bin) - 3.0).abs() < 0.06,
            "second {bin}: {} mbit/s before the drop",
            mbit(bin)
        );
    }
    for bin in [4, 5] {
        assert!(
            (mbit(bin) - 0.9597).abs() < 0.02,
            "second {bin}: {} mbit/s after the drop",
            mbit(bin)
        );
    }
    // The result says what happened
    assert_eq!(result.events.len(), 1);
    let step = &result.events[0];
    assert_eq!(
        (
            step.event,
            step.kind.as_str(),
            step.link.as_str(),
            step.direction.as_deref()
        ),
        (0, "set", "access", Some("forward"))
    );
    assert_eq!(step.scheduled_ns, 3_000_000_000);
    // The workload sends into the direction, so the sender waits for the step: 375 packets a second, 1,125 packets
    // before 3 s, and the step is done before packet 1,125
    assert_eq!(step.packet, Some(1125));
    assert_eq!(
        lateness_problems(&result.events, GAP_NS),
        Vec::<String>::new()
    );
    // A rate is not drawn: the run says nothing against reproducing the lost packets
    assert!(result.seed.loss_reproducible, "{:?}", result.seed);
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// Canary: the judgement of the lateness can fail. A step 80 ms late, a step by the clock that began before its moment, a
/// step at a barrier that began more than a gap early, and one that ended before it began are all named; a step on
/// time, and a barrier step a little early, are not.
#[test]
fn canary_a_step_that_is_late_or_early_fails_the_judgement() {
    let record = |scheduled: u64, started: u64, applied: u64| {
        EventRecord::new(0, scheduled, started, applied)
    };
    let barrier = |scheduled: u64, started: u64, applied: u64| {
        let mut event = EventRecord::new(0, scheduled, started, applied);
        event.packet = Some(7);
        event
    };
    let none = Vec::<String>::new();
    assert_eq!(
        lateness_problems(&[record(1_000_000, 1_000_500, 2_500_000)], GAP_NS),
        none
    );
    // A barrier step begins when the sender has sent the packet before: up to a gap early is right
    assert_eq!(
        lateness_problems(&[barrier(10_000_000, 8_000_000, 9_000_000)], GAP_NS),
        none
    );
    let late = lateness_problems(&[record(1_000_000, 1_000_500, 81_000_000)], GAP_NS);
    assert!(late.iter().any(|p| p.contains("late")), "{late:?}");
    let early = lateness_problems(&[record(5_000_000, 4_000_000, 6_000_000)], GAP_NS);
    assert!(
        early.iter().any(|p| p.contains("before it may")),
        "{early:?}"
    );
    let too_early = lateness_problems(&[barrier(10_000_000, 5_000_000, 6_000_000)], GAP_NS);
    assert!(
        too_early.iter().any(|p| p.contains("before it may")),
        "{too_early:?}"
    );
    let backwards = lateness_problems(&[record(5_000_000, 6_000_000, 5_500_000)], GAP_NS);
    assert!(
        backwards.iter().any(|p| p.contains("ended before")),
        "{backwards:?}"
    );
}

/// A `both` is two steps of one event, one for each direction, done one after the other and in order.
#[test]
fn an_event_on_both_directions_is_two_steps_in_order() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-both");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "",
            "      both: { delay: 10ms }\n",
            "3s",
            "  - { at: 1s, link: access, set: { both: { delay: 30ms } } }\n  - { at: 2s, link: access, set: { reverse: { delay: 5ms } } }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let steps: Vec<(usize, &str)> = result
        .events
        .iter()
        .map(|e| (e.event, e.direction.as_deref().unwrap()))
        .collect();
    assert_eq!(
        steps,
        [(0, "forward"), (0, "reverse"), (1, "reverse")],
        "{:?}",
        result.events
    );
    assert!(
        result
            .events
            .windows(2)
            .all(|w| w[0].applied_ns <= w[1].started_ns)
    );
    assert_eq!(
        lateness_problems(&result.events, GAP_NS),
        Vec::<String>::new()
    );
    // The delay of the packets went from 10 to 30 ms at one second: the median of the whole run is 30 ms
    // (two seconds of three), and the smallest delay is the 10 ms of the first second
    let delay = result.workloads[0].stats.delay.as_ref().unwrap();
    assert!(
        (10_000_000..10_500_000).contains(&delay.min_ns),
        "{}",
        delay.min_ns
    );
    assert!(
        (30_000_000..30_500_000).contains(&delay.p50_ns),
        "{}",
        delay.p50_ns
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The run looks at the token between steps: cancelled when the first step is done, it ends without doing the
/// second, and nothing is left.
#[test]
fn a_cancel_between_two_steps_ends_the_run_and_leaves_nothing() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-cancel");
    let mut backend = backend(|_| {});
    let token = CancelToken::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observer = {
        let (token, seen) = (token.clone(), Arc::clone(&seen));
        Arc::new(move |event: &RunEvent| {
            if let RunEvent::StepDone { event, .. } = event {
                seen.lock().unwrap().push(*event);
                token.cancel();
            }
        })
    };
    let options = options(&dir).with_observer(observer).with_cancel(token);
    let error = run(
        &mut backend,
        &scenario(
            "",
            "      forward: { delay: 10ms }\n",
            "10s",
            "  - { at: 1s, link: access, set: { forward: { delay: 20ms } } }\n  - { at: 5s, link: access, set: { forward: { delay: 30ms } } }\n",
        ),
        &options,
    )
    .unwrap_err();
    assert!(matches!(error, Error::Cancelled), "{error}");
    assert_eq!(*seen.lock().unwrap(), [0], "only the first step was done");
    assert_nothing_left(backend.run_id(), "after the cancel");
    let _ = std::fs::remove_dir_all(dir);
}

/// A backend that is slow at changing a link, and otherwise the Kernel backend: the machine that is busy at the
/// wrong moment, made on purpose. The barrier is for exactly this.
struct Slow {
    inner: netscenario_backend_kernel::KernelBackend,
    delay: Duration,
}

impl Backend for Slow {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn capabilities(&self) -> netscenario_core::Capabilities {
        self.inner.capabilities()
    }
    fn create_node(&mut self, name: &Name) -> Result<NodeHandle, Error> {
        self.inner.create_node(name)
    }
    fn create_link(&mut self, name: &Name, between: [NodeHandle; 2]) -> Result<LinkHandle, Error> {
        self.inner.create_link(name, between)
    }
    fn check_values(&self, scenario: &Scenario) -> Result<(), netscenario_core::CapabilityError> {
        self.inner.check_values(scenario)
    }
    fn plan_states(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        states: &[Impairments],
    ) -> Result<(), Error> {
        self.inner.plan_states(link, direction, states)
    }
    fn apply(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), Error> {
        self.inner.apply(link, direction, impairments, seed)
    }
    fn change(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
    ) -> Result<(), Error> {
        std::thread::sleep(self.delay);
        self.inner.change(link, direction, impairments)
    }
    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &Program,
    ) -> Result<ProgramHandle, Error> {
        self.inner.start_program(node, program)
    }
    fn queue_packets(
        &mut self,
        link: LinkHandle,
        direction: Direction,
    ) -> Result<Option<u64>, Error> {
        self.inner.queue_packets(link, direction)
    }
    fn node_counts(
        &mut self,
        link: LinkHandle,
        direction: Direction,
    ) -> Result<Option<netscenario_core::NodeCounts>, Error> {
        self.inner.node_counts(link, direction)
    }
    fn address(&self, link: LinkHandle, end: usize) -> Result<std::net::Ipv4Addr, Error> {
        self.inner.address(link, end)
    }
    fn describe(&self) -> netscenario_core::BackendInfo {
        self.inner.describe()
    }
    fn poll_program(
        &mut self,
        program: ProgramHandle,
        wait: Duration,
    ) -> Result<netscenario_core::ProgramState, Error> {
        self.inner.poll_program(program, wait)
    }
    fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error> {
        self.inner.send_line(program, line)
    }
    fn teardown(&mut self) -> Result<(), Error> {
        self.inner.teardown()
    }
}

/// The lost packets of a run in which the loss comes on at 1 s: from nothing to 30 %. With no loss a packet draws no
/// number, so a step that lands one packet earlier or later shifts every draw after it and the whole pattern of lost
/// packets with it: the sharpest test of where a step lands. `change_takes` is how long the backend sleeps before
/// it changes the link; `barriers` is the switch of `RunOptions`.
fn lost_packets(
    change_takes: Duration,
    barriers: bool,
    label: &str,
) -> (Vec<u64>, EventRecord, bool) {
    let dir = work_dir(label);
    let mut backend = Slow {
        inner: backend(|_| {}),
        delay: change_takes,
    };
    let mut options = options(&dir);
    options.export_packets = true;
    options.barriers = barriers;
    let result = run(
        &mut backend,
        &scenario(
            "seed: 5\n",
            "      forward: { delay: 5ms }\n",
            "2s",
            "  - { at: 1s, link: access, set: { forward: { loss: { model: random, p: 30% } } } }\n",
        ),
        &options,
    )
    .unwrap();
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
}

/// D-038: the same seed gives the same lost packets, whatever the machine does at the moment of the step. The link
/// has no loss for the first second and 30 % from then on; the backend is made to take 0 ms, 40 ms (14 packets) and
/// 120 ms (45 packets) for the change, and the sender waits for it each time, so the step lands before the same packet and
/// the lost packets are the same.
#[test]
fn the_same_seed_loses_the_same_packets_whatever_the_machine_does_at_the_step() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (first, event, reproducible) = lost_packets(Duration::ZERO, true, "timeline-same-a");
    let (slow, slow_event, _) = lost_packets(Duration::from_millis(40), true, "timeline-same-b");
    let (slower, _, _) = lost_packets(Duration::from_millis(120), true, "timeline-same-c");
    // Something was lost, so that "the same" means something: 30 % of the 375 packets of the second second
    assert!(
        (80..150).contains(&first.len()),
        "{} packets lost",
        first.len()
    );
    // No packet was lost before the step: it landed before packet 375 and not one earlier
    assert_eq!(event.packet, Some(375));
    assert_eq!(slow_event.packet, Some(375));
    assert!(first.iter().all(|sequence| *sequence >= 375), "{first:?}");
    assert_eq!(first, slow, "40 ms for the change moved the lost packets");
    assert_eq!(
        first, slower,
        "120 ms for the change moved the lost packets"
    );
    assert!(
        reproducible,
        "the run says it cannot be repeated although a sender waits for the step"
    );
    // The sender waited for the slow change, and says how long: the packet left late, the next ones caught up
    assert!(
        slow_event.held_ns.unwrap() >= 30_000_000,
        "{:?}",
        slow_event.held_ns
    );
}

/// Canary for the test above: without the barrier, the same slowness moves the lost packets. The step is done by the
/// clock, 40 ms after its moment it lands before a packet 14 later, and the draws of the whole second second are shifted.
/// If the barrier were not what keeps the packets the same, this and the test above would agree, and the test above
/// would prove nothing. The run says why it cannot be repeated.
#[test]
fn canary_without_the_barrier_the_same_slowness_moves_the_lost_packets() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (quick, quick_event, _) = lost_packets(Duration::ZERO, false, "timeline-clock-a");
    let (slow, slow_event, reproducible) =
        lost_packets(Duration::from_millis(80), false, "timeline-clock-b");
    assert!(quick_event.packet.is_none() && slow_event.packet.is_none());
    assert_ne!(
        quick, slow,
        "80 ms for the change did not move the lost packets: the barrier proves nothing"
    );
    assert!(
        !reproducible,
        "the run says it can be repeated although the clock did the step"
    );
}

/// The reason that the result gives for a step by the clock is in it, with the command line and Python reading it
/// from there.
#[test]
fn a_step_by_the_clock_that_changes_the_loss_says_why_the_run_cannot_be_repeated() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-reason");
    let mut backend = backend(|_| {});
    let mut options = options(&dir);
    options.barriers = false;
    let result = run(
        &mut backend,
        &scenario(
            "seed: 5\n",
            "      forward: { loss: { model: random, p: 5% } }\n",
            "2s",
            "  - { at: 1s, link: access, set: { forward: { loss: { model: random, p: 20% } } } }\n",
        ),
        &options,
    )
    .unwrap();
    assert!(!result.seed.loss_reproducible);
    assert!(
        result
            .seed
            .not_reproducible_because
            .iter()
            .any(|why| why.contains("barriers are switched off")),
        "{:?}",
        result.seed.not_reproducible_because
    );
    // 5 % of the 375 packets of the first second and 20 % of those of the second: 12.5 % with a standard
    // deviation of 1.18 points, so 7.8 to 17.2 % is four deviations on each side
    let media = &result.workloads[0].stats;
    assert!(
        (0.078..0.172).contains(&media.loss_ratio),
        "{} lost of {}",
        media.lost,
        media.sent
    );
    let _ = std::fs::remove_dir_all(dir);
}
