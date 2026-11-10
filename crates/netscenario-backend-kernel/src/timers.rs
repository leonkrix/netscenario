//! The timer report of `netscenario doctor --timers` (ENV-05, D-097): the precision of the clock and of sleeps (measured by
//! [`netscenario_core::timers`]), and what a change of a qdisc costs through the `tc` that lives for a run (D-038), which
//! is what a step of a timeline waits for.
//!
//! The change is measured in a network namespace of its own, in the way a run takes, so nothing of the host is touched
//! (SAF-01). The judging is a function of the numbers ([`checks`]) so that it can be tested with machines that are not here.

use std::time::Instant;

use netscenario_core::timers::{Effort, SPIN_MARGIN_NS, Spread, Timers, length_name, micros};
use serde::Serialize;

use crate::batch::{ANSWER_WITHIN, Batch, Failure};
use crate::command::{find, namespace_flags};
use crate::doctor::{Check, Status};

/// How long a change of a qdisc takes through the batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeCost {
    /// The kind of qdisc that was changed: `netem` (what a step changes), or `pfifo` where the kernel has no `netem`.
    pub qdisc: String,
    /// The time from writing the line to the answer of `tc`, in nanoseconds.
    #[serde(flatten)]
    pub took: Spread,
}

/// What `doctor --timers` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimerFindings {
    /// The clock and the sleeps of this machine.
    pub timers: Timers,
    /// The cost of a change of a qdisc, if it could be measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc_change: Option<ChangeCost>,
    /// Why the cost of a change could not be measured, if it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc_change_error: Option<String>,
}

/// How slow a change through the batch may be before a step of a timeline is late by more than it needs to be. The runs that
/// were measured took 0.03 ms to 2 ms with every processor busy (`batch.rs`).
pub const SLOW_CHANGE_NS: u64 = 2_000_000;

/// Above this a reading of the clock is slow (a `hpet` or `acpi_pm` clock source, or a virtual machine that traps it): a
/// sender reads the clock for every packet.
pub const SLOW_CLOCK_READ_NS: u64 = 1_000;

/// Measures the timers of this machine and the cost of a change. It takes about a second.
#[must_use]
pub fn measure() -> TimerFindings {
    let timers = netscenario_core::timers::measure(Effort::Thorough);
    match measure_change(500) {
        Ok(cost) => TimerFindings {
            timers,
            tc_change: Some(cost),
            tc_change_error: None,
        },
        Err(why) => TimerFindings {
            timers,
            tc_change: None,
            tc_change_error: Some(why),
        },
    }
}

/// Changes a qdisc `count` times through a batch in a namespace of its own and times each change.
fn measure_change(count: usize) -> Result<ChangeCost, String> {
    let (Some(unshare), Some(tc)) = (find("unshare"), find("tc")) else {
        return Err("`unshare` or `tc` is missing (see the tools above)".to_owned());
    };
    let mut why = Vec::new();
    for user_namespace in [true, false] {
        let mut arguments: Vec<String> = namespace_flags(user_namespace)
            .into_iter()
            .map(String::from)
            .collect();
        arguments.extend(["--".to_owned(), tc.display().to_string()]);
        arguments.extend(["-force", "-batch", "-"].map(String::from));
        match Batch::start(&unshare, &arguments, &[], ANSWER_WITHIN) {
            Ok(mut batch) => {
                let measured = changes(&mut batch, count);
                batch.end();
                return measured;
            }
            Err(message) => why.push(message),
        }
    }
    Err(format!(
        "no network namespace to measure in: {}",
        why.join("; ")
    ))
}

/// The lines that change the root qdisc of the loopback: the first creates it, the later ones are changes.
fn changes(batch: &mut Batch, count: usize) -> Result<ChangeCost, String> {
    // A kernel without `netem` (the sandbox) still shows what the batch costs with another qdisc
    let (qdisc, lines): (&str, [&str; 2]) = match batch.run(
        &["qdisc replace dev lo root netem delay 1ms".to_owned()],
        ANSWER_WITHIN,
    ) {
        Ok(()) => (
            "netem",
            [
                "qdisc replace dev lo root netem delay 2ms",
                "qdisc replace dev lo root netem delay 1ms",
            ],
        ),
        Err(Failure::Refused(_)) => {
            batch
                .run(
                    &["qdisc replace dev lo root pfifo limit 100".to_owned()],
                    ANSWER_WITHIN,
                )
                .map_err(failure_text)?;
            (
                "pfifo",
                [
                    "qdisc replace dev lo root pfifo limit 200",
                    "qdisc replace dev lo root pfifo limit 100",
                ],
            )
        }
        Err(other) => return Err(failure_text(other)),
    };
    let mut took = Vec::with_capacity(count);
    // The first few changes pay for pages that have not been touched yet
    for turn in 0..count + 10 {
        let line = lines[turn % 2].to_owned();
        let begin = Instant::now();
        batch.run(&[line], ANSWER_WITHIN).map_err(failure_text)?;
        if turn >= 10 {
            took.push(u64::try_from(begin.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
    }
    Spread::from_samples(&took)
        .map(|took| ChangeCost {
            qdisc: qdisc.to_owned(),
            took,
        })
        .ok_or_else(|| "no change was measured".to_owned())
}

fn failure_text(failure: Failure) -> String {
    match failure {
        Failure::Refused(message) => format!("`tc` refused a change: {message}"),
        Failure::Broken(message) => format!("the batch of `tc` stopped: {message}"),
    }
}

fn timer(id: &str, status: Status, title: &str, detail: String, fix: Option<String>) -> Check {
    Check {
        section: "timers",
        id: id.to_owned(),
        status,
        title: title.to_owned(),
        detail,
        fix,
    }
}

/// The checks of the timer report for `found`, on a machine with `cpus` processors.
#[must_use]
pub fn checks(found: &TimerFindings, cpus: usize) -> Vec<Check> {
    let mut checks = vec![clock_check(&found.timers)];
    checks.extend(sleep_checks(&found.timers));
    checks.push(slack_check(&found.timers));
    checks.push(load_check(&found.timers, cpus));
    checks.push(change_check(found));
    checks
}

/// The check on the clock.
fn clock_check(timers: &Timers) -> Check {
    let slow_clock = timers.clock_read_ns > SLOW_CLOCK_READ_NS;
    timer(
        "timer-clock",
        if slow_clock { Status::Warn } else { Status::Ok },
        "clock",
        format!(
            "CLOCK_MONOTONIC from {}: a reading costs {} ns, the smallest step seen is {} ns",
            timers.clock_source.as_deref().unwrap_or("an unknown source"),
            timers.clock_read_ns,
            timers.clock_step_ns,
        ),
        slow_clock.then(|| {
            "a sender reads the clock for every packet, so a slow clock source (hpet, acpi_pm) limits the rate it can keep; \
             `tsc` or `kvm-clock` are fast"
                .to_owned()
        }),
    )
}

/// One check for each length of sleep that was measured.
fn sleep_checks(timers: &Timers) -> Vec<Check> {
    let mut checks = Vec::new();
    for sleep in &timers.sleeps {
        let late = sleep.late;
        let over = late.p99_ns > SPIN_MARGIN_NS;
        let name = length_name(sleep.asked_ns);
        checks.push(timer(
            &format!("timer-sleep-{}", name.replace(' ', "")),
            if over { Status::Warn } else { Status::Ok },
            &format!("sleep of {name}"),
            format!(
                "late by {} (median), {} (99th percentile), {} (largest) in {} sleeps",
                micros(late.p50_ns),
                micros(late.p99_ns),
                micros(late.max_ns),
                late.samples,
            ),
            over.then(|| {
                format!(
                    "a sender sleeps until {} before the slot of a packet and spins for the rest, so a wake-up later than that \
                     sends the packet late; `max_lateness_ns` of a run says how late they were. A `performance` governor and \
                     an idle machine help",
                    micros(SPIN_MARGIN_NS),
                )
            }),
        ));
    }
    checks
}

/// The slack of the timers and the governor, for information.
fn slack_check(timers: &Timers) -> Check {
    let slack = timers
        .timer_slack_ns
        .map_or_else(|| "unknown".to_owned(), micros);
    timer(
        "timer-slack",
        Status::Info,
        "timer slack",
        format!(
            "{slack} (the kernel may delay a timer this much to batch it with others); governor: {}",
            timers
                .cpu_governor
                .as_deref()
                .unwrap_or("none (a virtual machine, or no frequency scaling)"),
        ),
        None,
    )
}

/// The load against the number of processors.
fn load_check(timers: &Timers, cpus: usize) -> Check {
    let busy = timers
        .load_1m_hundredths
        .is_some_and(|load| usize::try_from(load).unwrap_or(usize::MAX) > cpus.saturating_mul(100));
    timer(
        "timer-load",
        if busy { Status::Warn } else { Status::Info },
        "load",
        match (timers.load_1m_hundredths, timers.runnable_tasks) {
            (Some(load), Some(tasks)) => format!(
                "load average {}.{:02} of the last minute on {cpus} processors, {tasks} tasks runnable (this one included)",
                load / 100,
                load % 100,
            ),
            _ => "the load could not be read".to_owned(),
        },
        busy.then(|| {
            "the machine has more to do than it has processors: every timer here is later than on an idle one, and \
             the measurements of a run are only as exact as the machine is free"
                .to_owned()
        }),
    )
}

/// The cost of a change of a qdisc, or why it was not measured.
fn change_check(found: &TimerFindings) -> Check {
    match (&found.tc_change, &found.tc_change_error) {
        (Some(cost), _) => {
            let slow = cost.took.p99_ns > SLOW_CHANGE_NS;
            timer(
                "timer-tc-change",
                if slow { Status::Warn } else { Status::Ok },
                "change of a qdisc",
                format!(
                    "{} median, {} (99th percentile), {} (largest) in {} changes of a {} through the `tc` that lives for a run",
                    micros(cost.took.p50_ns),
                    micros(cost.took.p99_ns),
                    micros(cost.took.max_ns),
                    cost.took.samples,
                    cost.qdisc,
                ),
                slow.then(|| {
                    "a step of a timeline is done this much after its moment; the `events` of a result say how late each one was \
                     (`applied_ns - scheduled_ns`)"
                        .to_owned()
                }),
            )
        }
        (None, why) => timer(
            "timer-tc-change",
            Status::Info,
            "change of a qdisc",
            format!(
                "not measured: {}",
                why.as_deref().unwrap_or("no reason was given")
            ),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(p50_ns: u64, p99_ns: u64, max_ns: u64) -> Spread {
        Spread::from_samples(&[p50_ns, p50_ns, p99_ns, max_ns]).unwrap()
    }

    fn found(
        late_p99: u64,
        clock_read_ns: u64,
        load: Option<u32>,
        change_p99: u64,
    ) -> TimerFindings {
        TimerFindings {
            // The types are `non_exhaustive`, so a test of another crate writes them as the result does
            timers: serde_json::from_value(serde_json::json!({
                "clock_source": "tsc",
                "clock_read_ns": clock_read_ns,
                "clock_step_ns": 1,
                "timer_slack_ns": 50_000,
                "cpu_governor": null,
                "load_1m_hundredths": load,
                "runnable_tasks": 2,
                "sleeps": [{
                    "asked_ns": 200_000,
                    "samples": 4,
                    "p50_ns": 60_000,
                    "p99_ns": late_p99,
                    "max_ns": late_p99,
                }],
            }))
            .unwrap(),
            tc_change: Some(ChangeCost {
                qdisc: "netem".to_owned(),
                took: spread(30_000, change_p99, change_p99),
            }),
            tc_change_error: None,
        }
    }

    fn status(checks: &[Check], id: &str) -> Status {
        checks.iter().find(|c| c.id == id).unwrap().status
    }

    #[test]
    fn a_machine_that_is_on_time_has_no_warning() {
        let all = checks(&found(90_000, 25, Some(40), 100_000), 4);
        assert!(all.iter().all(|c| c.status != Status::Warn), "{all:?}");
        assert!(all.iter().all(|c| c.section == "timers"));
        assert_eq!(status(&all, "timer-sleep-200us"), Status::Ok);
    }

    /// Canary: each warning can fire. A wake-up past the spin margin of the sender, a slow clock, a machine with more load
    /// than processors and a slow change each give their own warning with a fix, and none of the others.
    #[test]
    fn each_kind_of_lateness_gives_its_own_warning() {
        let late = checks(&found(SPIN_MARGIN_NS + 1, 25, Some(40), 100_000), 4);
        assert_eq!(status(&late, "timer-sleep-200us"), Status::Warn);
        assert!(
            late.iter()
                .find(|c| c.id == "timer-sleep-200us")
                .unwrap()
                .fix
                .is_some()
        );
        assert_eq!(late.iter().filter(|c| c.status == Status::Warn).count(), 1);

        let slow_clock = checks(&found(90_000, SLOW_CLOCK_READ_NS + 1, Some(40), 100_000), 4);
        assert_eq!(status(&slow_clock, "timer-clock"), Status::Warn);
        assert_eq!(
            slow_clock
                .iter()
                .filter(|c| c.status == Status::Warn)
                .count(),
            1
        );

        let busy = checks(&found(90_000, 25, Some(401), 100_000), 4);
        assert_eq!(status(&busy, "timer-load"), Status::Warn);
        let exactly_full = checks(&found(90_000, 25, Some(400), 100_000), 4);
        assert_eq!(status(&exactly_full, "timer-load"), Status::Info);

        let slow_change = checks(&found(90_000, 25, Some(40), SLOW_CHANGE_NS + 1), 4);
        assert_eq!(status(&slow_change, "timer-tc-change"), Status::Warn);
        assert_eq!(
            slow_change
                .iter()
                .filter(|c| c.status == Status::Warn)
                .count(),
            1
        );
    }

    #[test]
    fn a_change_that_could_not_be_measured_says_why_and_is_no_failure() {
        let mut none = found(90_000, 25, None, 0);
        none.tc_change = None;
        none.tc_change_error = Some("no network namespace to measure in: denied".to_owned());
        let all = checks(&none, 4);
        let change = all.iter().find(|c| c.id == "timer-tc-change").unwrap();
        assert_eq!(change.status, Status::Info);
        assert!(change.detail.contains("denied"));
        assert!(
            all.iter()
                .find(|c| c.id == "timer-load")
                .unwrap()
                .detail
                .contains("could not be read")
        );
    }

    /// The change of a qdisc is measured where namespaces can be made: the lines were done (so the numbers are of changes that
    /// `tc` answered), and the order of the summary holds. Where no namespace can be made it says so and is no failure.
    /// Canary: more changes than asked for are not counted, and the first ten that pay for cold pages are left out.
    #[test]
    fn a_change_of_a_qdisc_is_timed_through_the_batch_or_the_reason_is_given() {
        match measure_change(40) {
            Ok(cost) => {
                assert!(
                    ["netem", "pfifo"].contains(&cost.qdisc.as_str()),
                    "{cost:?}"
                );
                assert_eq!(cost.took.samples, 40);
                assert!(
                    cost.took.p50_ns <= cost.took.p99_ns && cost.took.p99_ns <= cost.took.max_ns
                );
                // A change cannot take less than a round trip to a process; a nanosecond count of zero is a clock that was not read
                assert!(cost.took.p50_ns > 0, "{cost:?}");
            }
            Err(why) => assert_ne!(why, ""),
        }
    }
}
