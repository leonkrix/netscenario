//! The timeline in a real run (TML-01, step 4.3b): a `set` changes the link while the workloads go, the result says
//! when, and the throughput shows it. The steps are done by the clock; what that costs in reproducibility is in the
//! result and tested here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Counts of packets and rates of a few megabits are far below the precision of a double
    clippy::cast_precision_loss
)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use netscenario_core::{CancelToken, Error, EventRecord, RunEvent, RunOptions, run};
use netscenario_model::Scenario;
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

/// How late a step may be, at most. A step is due at a moment of the clock and done by a `tc` process through
/// `nsenter`: about a millisecond on a quiet machine and a few on a busy one (S11, D-038). Noise only goes one way,
/// so the lower bound is exact (a step is never done before its moment) and the upper bound is room for a shared
/// runner: 50 ms is more than ten times what was measured with every processor busy.
const LATEST: Duration = Duration::from_millis(50);

/// The judgement of the lateness of the steps, in one place so that the canary below uses the same one.
fn lateness_problems(events: &[EventRecord]) -> Vec<String> {
    let mut problems = Vec::new();
    for event in events {
        if event.started_ns < event.scheduled_ns {
            problems.push(format!(
                "event {} began {} ns before its moment",
                event.event,
                event.scheduled_ns - event.started_ns
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
    assert_eq!(lateness_problems(&result.events), Vec::<String>::new());
    // A rate is not drawn: the run says nothing against reproducing the lost packets
    assert!(result.seed.loss_reproducible, "{:?}", result.seed);
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// Canary: the judgement of the lateness can fail. A step 80 ms late, one that began before its moment and one that
/// ended before it began are all named; a step on time is not.
#[test]
fn canary_a_step_that_is_late_or_early_fails_the_judgement() {
    let record = |scheduled: u64, started: u64, applied: u64| {
        EventRecord::new(0, scheduled, started, applied)
    };
    assert_eq!(
        lateness_problems(&[record(1_000_000, 1_000_500, 2_500_000)]),
        Vec::<String>::new()
    );
    let late = lateness_problems(&[record(1_000_000, 1_000_500, 81_000_000)]);
    assert!(late.iter().any(|p| p.contains("late")), "{late:?}");
    let early = lateness_problems(&[record(5_000_000, 4_000_000, 6_000_000)]);
    assert!(
        early.iter().any(|p| p.contains("before its moment")),
        "{early:?}"
    );
    let backwards = lateness_problems(&[record(5_000_000, 6_000_000, 5_500_000)]);
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
    assert_eq!(lateness_problems(&result.events), Vec::<String>::new());
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

/// A step that changes what a queue draws numbers for is applied by the clock, and the result says that the lost
/// packets of the seed are not promised to repeat, with the reason; a step that changes the rate does not.
#[test]
fn a_step_that_changes_the_loss_makes_the_run_not_reproducible_and_says_why() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("timeline-loss");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "seed: 5\n",
            "      forward: { loss: { model: random, p: 5% } }\n",
            "2s",
            "  - { at: 1s, link: access, set: { forward: { loss: { model: random, p: 20% } } } }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    assert!(!result.seed.loss_reproducible);
    assert!(
        result
            .seed
            .not_reproducible_because
            .iter()
            .any(|why| why.contains("applies a step by the clock")),
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
