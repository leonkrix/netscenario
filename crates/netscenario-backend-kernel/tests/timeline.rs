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

use netscenario_agent::records;
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

/// How late a step may be, at most. A step by the clock is due at a moment and done by the `tc` that lives for the run:
/// 0.03 ms on the development machine, a few ms on a busy one, and 2 to 4 ms when a `tc` process is started each
/// time (S11, D-038, D-060, D-063). Noise only goes one way, so the lower bound is exact (a step by the clock is
/// never done before its moment) and the upper bound is room for a shared runner: 50 ms is more than ten times what
/// was measured with every processor busy.
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
    // The step went through the `tc` that lives for the run: the backend has nothing to note (D-063)
    assert_eq!(result.notes, Vec::<String>::new());
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

/// A `both` is two steps of one event, one for each direction, done one at a time; each direction sees its events in
/// the order of the file.
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
    let mut steps: Vec<(usize, &str)> = result
        .events
        .iter()
        .map(|e| (e.event, e.direction.as_deref().unwrap()))
        .collect();
    // The order of the two directions of one event is not promised: the workload sends forward, so that step is done
    // at a barrier, and nothing sends back, so the clock does the other. A sender that the machine holds back for
    // 20 ms asks after the clock has done the reverse step (found with 24 busy loops on 4 cores: the packet then waited
    // 34 and 59 ms). What is promised: every step is there once, each direction sees the events of the file in their
    // order, and the steps are done one at a time.
    steps.sort_unstable();
    assert_eq!(
        steps,
        [(0, "forward"), (0, "reverse"), (1, "reverse")],
        "{:?}",
        result.events
    );
    let reverse: Vec<usize> = result
        .events
        .iter()
        .filter(|e| e.direction.as_deref() == Some("reverse"))
        .map(|e| e.event)
        .collect();
    assert_eq!(reverse, [0, 1], "{:?}", result.events);
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

/// The sequence numbers of the packets that arrived, in the order of arrival, and when (nanoseconds).
fn arrivals(result: &netscenario_core::RunResult, dir: &std::path::Path) -> Vec<(u64, u64)> {
    let file =
        records::read(std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap())
            .unwrap();
    file.records
        .iter()
        .map(|record| (record.sequence, record.receive_ns))
        .collect()
}

/// What the steps of an outage and the arrivals say about its window, in sequence numbers.
struct Window {
    /// The first packet that can have been dropped, and the last that can have been the first one dropped.
    first: (u64, u64),
    /// The first packet that can have been the last one dropped, and the last that can have been.
    last: (u64, u64),
    /// When the start step began and when it was done, in nanoseconds on the clock of the arrivals.
    began: (u64, u64),
    /// The same for the end step.
    ended: (u64, u64),
}

/// The window of the outage of a run in sequence numbers and in time, from what the run recorded: the moments at which
/// the steps began and ended, and the delay that the packets have (`delay_ns`). A packet is dropped if it is let go by
/// the queue of the delay while the blackout is on, so the packet `k` of a stream that leaves at `k * gap` is dropped
/// if `k * gap + delay` lies between the moment the start step was done and the moment the end step began. `slack`
/// covers the lateness of the sender and the timer of the queue.
fn window_of(
    result: &netscenario_core::RunResult,
    records: &[records::Record],
    delay_ns: u64,
) -> Window {
    let (start, end) = (&result.events[0], &result.events[1]);
    assert_eq!(
        (start.kind.as_str(), end.kind.as_str()),
        ("outage-start", "outage-end")
    );
    let base = start_of(records);
    let slack = result.workloads[0].sender.max_lateness_ns + 5_000_000;
    let at = |moment: u64| base + moment;
    let packet = |moment: u64, round_up: bool| {
        let sent_at = moment.saturating_sub(delay_ns);
        if round_up {
            sent_at.div_ceil(GAP_NS)
        } else {
            sent_at / GAP_NS
        }
    };
    Window {
        first: (
            packet(start.started_ns.saturating_sub(slack), false),
            packet(start.applied_ns + slack, true),
        ),
        last: (
            packet(end.started_ns.saturating_sub(slack), false).saturating_sub(1),
            packet(end.applied_ns + slack, true),
        ),
        began: (at(start.started_ns), at(start.applied_ns)),
        ended: (at(end.started_ns), at(end.applied_ns)),
    }
}

/// The moment the measured phase began, on the clock of the records: a packet is never sent before its slot, so
/// `send_ns - sequence * gap` is at or after the start for every packet, and the smallest of them (the packet that was
/// the least late) is the start to within the lateness of that packet, a few microseconds.
fn start_of(records: &[records::Record]) -> u64 {
    records
        .iter()
        .map(|record| record.send_ns.saturating_sub(record.sequence * GAP_NS))
        .min()
        .expect("a packet arrived")
}

/// The packets that arrived although they were let go by the queue in the window, which cannot happen: a packet is
/// let go at its own moment of sending plus the least delay of the link, or later, and it is delivered after that. So a
/// packet that was sent late enough to be let go after the start step was done, and was received before the end step
/// began, was delivered in a window in which nothing is delivered. This holds whatever the machine does, also when the
/// receiver is held back and reads a packet long after it came (1 ms is for the start, which is known to a few
/// microseconds).
fn delivered_in_the_window(
    records: &[records::Record],
    applied_start: u64,
    started_end: u64,
    least_delay_ns: u64,
) -> Vec<u64> {
    records
        .iter()
        .filter(|record| {
            record.send_ns + least_delay_ns >= applied_start + 1_000_000
                && record.receive_ns + 1_000_000 < started_end
        })
        .map(|record| record.sequence)
        .collect()
}

/// The arrivals of the packets of the first workload, in the order of arrival.
fn records_of(result: &netscenario_core::RunResult, dir: &std::path::Path) -> Vec<records::Record> {
    records::read(std::fs::File::open(dir.join(&result.workloads[0].records_file)).unwrap())
        .unwrap()
        .records
}

/// An outage is a window in the time of the receiver: nothing arrives in it, not even a packet that was on its way when
/// it began (LNK-12, D-064). 375 packets a second, a delay of 100 ms, an outage from 2 s to 4 s: the packets sent in
/// the 100 ms before 2 s are on their way when it begins, 37 of them, and they are dropped too; an outage that only
/// lost what enters the link would lose 750 to 1,499 and nothing before. The window is placed by the clock, so the
/// packet at each edge can differ from run to run, and the bounds come from the moments that the steps recorded.
#[test]
fn an_outage_is_a_window_in_which_nothing_arrives_not_even_what_was_on_its_way() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-outage");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "",
            "      forward: { delay: 100ms }\n",
            "6s",
            "  - { at: 2s, link: access, outage: 2s, direction: forward }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let records = records_of(&result, &dir);
    let window = window_of(&result, &records, 100_000_000);
    let stats = &result.workloads[0].stats;
    // What arrived is every packet but one run of them
    let mut got: Vec<u64> = records.iter().map(|record| record.sequence).collect();
    got.sort_unstable();
    let missing: Vec<u64> = (0..stats.sent)
        .filter(|n| got.binary_search(n).is_err())
        .collect();
    let (first, last) = (missing[0], *missing.last().unwrap());
    assert_eq!(
        missing,
        (first..=last).collect::<Vec<u64>>(),
        "the lost packets are not one run"
    );
    // The in-flight packets are in it: it begins about 37 packets before the packet that the moment 2 s is, not at it
    assert!(
        first < 750 - 30,
        "the packets on their way were not dropped: the run begins at {first}"
    );
    assert!(
        (window.first.0..=window.first.1).contains(&first),
        "the run begins at {first}, the steps allow {:?}",
        window.first
    );
    assert!(
        (window.last.0..=window.last.1).contains(&last),
        "the run ends at {last}, the steps allow {:?}",
        window.last
    );
    // Nothing was delivered in the window
    let inside = delivered_in_the_window(&records, window.began.1, window.ended.0, 100_000_000);
    assert_eq!(inside, Vec::<u64>::new(), "packets arrived in the window");
    let kinds: Vec<(&str, Option<&str>, Option<u64>)> = result
        .events
        .iter()
        .map(|e| (e.kind.as_str(), e.direction.as_deref(), e.packet))
        .collect();
    // By the clock, never at a barrier: the window is in the time of the receiver
    assert_eq!(
        kinds,
        [
            ("outage-start", Some("forward"), None),
            ("outage-end", Some("forward"), None)
        ]
    );
    assert_eq!(
        lateness_problems(&result.events, GAP_NS),
        Vec::<String>::new()
    );
    // The edges are placed by the clock, and the result says so
    assert!(!result.seed.loss_reproducible);
    assert!(
        result
            .seed
            .not_reproducible_because
            .iter()
            .any(|reason| reason.contains("has an outage")),
        "{:?}",
        result.seed
    );
    assert_eq!(result.notes, Vec::<String>::new());
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// An outage is a window and not a value (D-064): a loss that a `set` leaves inside it goes on as the file says, and a
/// `set` inside the window is done like any other. After the window the link is what the file says it is. Here the
/// delay changes in the middle of an outage, and the packets that leave after its end have the new delay.
#[test]
fn a_set_inside_an_outage_is_done_and_the_values_go_on_after_it() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-outage-set");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "",
            "      forward: { delay: 20ms }\n",
            "5s",
            "  - { at: 1s, link: access, outage: 2s, direction: forward }\n  - { at: 2s, link: access, set: { forward: { delay: 80ms, loss: { model: random, p: 100% } } } }\n  - { at: 3500ms, link: access, set: { forward: { loss: null } } }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let order: Vec<&str> = result.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(order, ["outage-start", "set", "outage-end", "set"]);
    let records = records_of(&result, &dir);
    // The link had a loss of 100 % from 2 s to 3.5 s: nothing sent in that time arrived, outage or not; before the
    // outage and after both everything did, with the delay the file says
    let sent_in = |from_ms: u64, to_ms: u64| -> Vec<&records::Record> {
        records
            .iter()
            .filter(|r| {
                let at = r.sequence * GAP_NS / 1_000_000;
                at >= from_ms && at < to_ms
            })
            .collect()
    };
    assert!(
        sent_in(2050, 3450).is_empty(),
        "packets got through a loss of 100 %"
    );
    // 700 ms at 375 packets a second are about 260 packets, and nothing is lost before the outage
    assert!(sent_in(100, 800).len() > 250);
    let late = sent_in(3600, 4800);
    // 1,200 ms are about 450 packets
    assert!(late.len() > 430, "{} packets", late.len());
    let delays: Vec<u64> = late.iter().map(|r| r.delay_ns()).collect();
    assert!(
        delays.iter().all(|d| (80_000_000..90_000_000).contains(d)),
        "the delay after the window was not the new one: {:?}",
        (delays.iter().min(), delays.iter().max())
    );
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// An outage in a direction of two queues (a rate with a jitter, then the delay) and a loss: the blackout is the third
/// queue, nothing arrives in the window whatever the jitter does, and the count of the first queue still agrees with
/// the packets that were sent (the packets that the blackout drops are in its counters, D-064).
#[test]
fn an_outage_in_two_queues_with_a_loss_drops_what_leaves_and_the_queue_still_counts() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-outage-two");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "seed: 9\n",
            "      forward: { delay: 60ms, jitter: { amount: 10ms }, rate: 20mbit, loss: { model: random, p: 10% } }\n",
            "4s",
            "  - { at: 1500ms, link: access, outage: 1s, direction: forward }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let records = records_of(&result, &dir);
    let base = start_of(&records);
    // The least delay of the link is 60 ms less the 10 ms of the jitter, and the rate only adds
    let inside = delivered_in_the_window(
        &records,
        base + result.events[0].applied_ns,
        base + result.events[1].started_ns,
        50_000_000,
    );
    assert_eq!(inside, Vec::<u64>::new(), "packets arrived in the window");
    assert!(records.len() > 700, "{} packets arrived", records.len());
    // The queue counted every packet that the workload sent into it, the blackout's drops included
    let [check] = result.seed.queue_checks.as_slice() else {
        panic!("{:?}", result.seed.queue_checks);
    };
    assert!(check.agrees(), "{check:?}");
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// A link that is down refuses what its senders send, and the sender counts it; the packets that were on their way
/// are dropped by the kernel, a handful at 10 ms of delay; afterwards everything arrives again (TML-02, D-064). The
/// sender asks the run at a barrier, so the number of refused packets is exact: 375 a second from 2 s to 3 s.
#[test]
fn a_link_that_is_down_refuses_the_packets_sent_while_it_is_down_and_comes_back() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-down");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "",
            "      forward: { delay: 10ms }\n",
            "5s",
            "  - { at: 2s, link: access, down: true }\n  - { at: 3s, link: access, up: true }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let workload = &result.workloads[0];
    assert_eq!(workload.sender.send_errors, 375, "{:?}", workload.sender);
    let first_error = workload.sender.first_error.as_deref().unwrap_or("");
    assert!(first_error.contains("unreachable"), "{first_error}");
    // 1,875 packets are due in 5 s; 375 were refused, the others were handed to the kernel. The refused ones are a
    // run of sequence numbers, they are not sent and not lost, and no arrival is out of the range (D-064)
    assert_eq!(workload.sender.refused, [[750, 1124]]);
    assert_eq!(workload.stats.sent, 1500);
    assert_eq!(workload.stats.unexpected, 0, "{:?}", workload.stats);
    // Lost: the packets that were in flight when the interface went down. 10 ms of delay at 375 packets a second
    // is 3.75 packets, so at most 4 are in the queue (the sender is not late, the bound is the physical one)
    assert!(workload.stats.lost <= 4, "{:?}", workload.stats);
    let seen = arrivals(&result, &dir);
    assert!(
        seen.iter()
            .all(|(sequence, _)| !(750..1125).contains(sequence)),
        "a packet that was refused arrived"
    );
    assert!(
        seen.iter().any(|(sequence, _)| *sequence >= 1870),
        "nothing arrived at the end"
    );
    let kinds: Vec<(&str, Option<&str>, Option<u64>)> = result
        .events
        .iter()
        .map(|e| (e.kind.as_str(), e.direction.as_deref(), e.packet))
        .collect();
    assert_eq!(kinds, [("down", None, Some(750)), ("up", None, Some(1125))]);
    assert_eq!(result.notes, Vec::<String>::new());
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// A link that goes down while its queue draws random numbers cannot be held to the count of packets that were sent
/// (the kernel drops the packets in flight without counting them), and the run says so instead of reporting a
/// disagreement of the queue (D-064).
#[test]
fn a_link_that_goes_down_with_a_loss_says_why_the_queue_is_not_checked() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-down-loss");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "seed: 5\n",
            "      forward: { delay: 10ms, loss: { model: random, p: 10% } }\n",
            "3s",
            "  - { at: 1s, link: access, down: true }\n  - { at: 2s, link: access, up: true }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    assert!(!result.seed.loss_reproducible);
    assert!(result.seed.queue_checks.is_empty(), "{:?}", result.seed);
    let reasons = &result.seed.not_reproducible_because;
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("goes down"), "{}", reasons[0]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The same seed gives the same lost packets with an outage in the run, outside the edges of its window: the queues that
/// draw numbers do not know of the outage, and the packets at the two edges are placed by the clock (D-064). The loss
/// is 20 % all the time, and the count of the first queue agrees in both runs.
#[test]
fn the_same_seed_loses_the_same_packets_outside_the_edges_of_an_outage() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let mut runs = Vec::new();
    for round in 0..2 {
        let dir = work_dir(&format!("timeline-outage-seed-{round}"));
        let mut backend = backend(|_| {});
        let result = run(
            &mut backend,
            &scenario(
                "seed: 11\n",
                "      forward: { delay: 50ms, loss: { model: random, p: 20% } }\n",
                "4s",
                "  - { at: 1500ms, link: access, outage: 1s, direction: forward }\n",
            ),
            &options(&dir),
        )
        .unwrap();
        let [check] = result.seed.queue_checks.as_slice() else {
            panic!("{:?}", result.seed.queue_checks);
        };
        assert!(check.agrees(), "{check:?}");
        let records = records_of(&result, &dir);
        let window = window_of(&result, &records, 50_000_000);
        let mut got: Vec<u64> = records.iter().map(|record| record.sequence).collect();
        got.sort_unstable();
        got.dedup();
        runs.push((got, window));
        let _ = std::fs::remove_dir_all(dir);
    }
    // Outside the widest span that either run allows for the edges, the two runs lost the same packets
    let from = runs.iter().map(|(_, w)| w.first.0).min().unwrap();
    let to = runs.iter().map(|(_, w)| w.last.1).max().unwrap();
    let outside = |got: &[u64]| -> Vec<u64> {
        got.iter()
            .copied()
            .filter(|n| *n < from || *n > to)
            .collect()
    };
    assert_eq!(outside(&runs[0].0), outside(&runs[1].0));
    // And the loss is there outside it: about 20 % of the packets, 4 s at 375 a second is 1,500 packets
    let kept = outside(&runs[0].0).len() as f64;
    let wanted = (1500 - (to - from + 1)) as f64;
    assert!(
        (kept / wanted - 0.8).abs() < 0.05,
        "{kept} of {wanted} packets outside the window"
    );
}
