//! The loop of a run while the workloads go: it waits for the senders and does the steps of the timeline when
//! they are due (step 4.3b).
//!
//! A step is done by the **clock**: the run asks the backend at the moment the file gives, counted from the
//! moment the workloads start. A packet that is sent between the moment and the end of the change meets the old
//! state, and which packet that is differs from run to run, so for a link that draws random numbers the lost
//! packets of one seed differ too. The result says so (`seed.not_reproducible_because`); the barrier of D-038, which
//! makes the sender wait for the change at the packet it belongs to, is the next step. For a link whose
//! changes draw no numbers (a rate, a delay) the clock is exact enough: the result records how late each step was.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::events::{Event, SendSummary};
use netscenario_model::Scenario;

use crate::backend::{Backend, Direction, ProgramHandle};
use crate::error::Error;
use crate::observe::RunEvent;
use crate::provision::Provisioned;
use crate::result::EventRecord;
use crate::run::{Control, event, failure};
use crate::timeline::{Action, Step};

/// When the workloads begin: on the clock the run waits with, and on `CLOCK_MONOTONIC` that the records use.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Begin {
    pub(crate) at: Instant,
    pub(crate) ns: u64,
}

/// What the loop found out.
pub(crate) struct Driven {
    /// What each sender said at its end, in the order of the senders.
    pub(crate) sent: Vec<SendSummary>,
    /// The steps that were done, in the order they were done.
    pub(crate) events: Vec<EventRecord>,
}

/// Below this a wait for a step is a sleep to the exact moment and not a wait for news of a sender: the wait for
/// news counts whole milliseconds, and a step must not be due a millisecond late because of that.
const SLEEP_BELOW: Duration = Duration::from_millis(3);

/// The longest the loop waits for news of a sender when no step is due.
const LONGEST_WAIT: Duration = Duration::from_millis(200);

/// Waits for the senders to end and does the steps of the timeline at their moments.
///
/// # Errors
///
/// - [`Error::Cancelled`] if the run is cancelled (looked at before every wait),
/// - [`Error::Backend`] if a step cannot be done,
/// - [`Error::Run`] if a sender fails, says something else than `sent` at its end, or does not end in time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    senders: &[(ProgramHandle, String)],
    steps: &[Step],
    begin: Begin,
    deadline: Instant,
    control: &Control<'_>,
) -> Result<Driven, Error> {
    let mut pending: VecDeque<&Step> = steps.iter().collect();
    let mut lines: Vec<Vec<String>> = vec![Vec::new(); senders.len()];
    let mut ended = vec![false; senders.len()];
    let mut events = Vec::new();
    loop {
        control.checkpoint()?;
        // The steps that are due, in order. A step that is a moment away is waited for to the microsecond.
        while let Some(step) = pending.front() {
            let due = begin.at + Duration::from_nanos(step.at.as_nanos());
            let left = due.saturating_duration_since(Instant::now());
            if left > SLEEP_BELOW {
                break;
            }
            if !left.is_zero() {
                std::thread::sleep(left);
            }
            let record = do_step(backend, scenario, provisioned, step, begin)?;
            control.emit(&RunEvent::StepDone {
                event: record.event,
                lateness: Duration::from_nanos(
                    record.applied_ns.saturating_sub(record.scheduled_ns),
                ),
            });
            events.push(record);
            pending.pop_front();
        }
        // Until the next step is due, or for news: the first sender that is still going is asked to wait for
        // news, the others are only looked at
        let wait = pending.front().map_or(LONGEST_WAIT, |step| {
            let due = begin.at + Duration::from_nanos(step.at.as_nanos());
            due.saturating_duration_since(Instant::now())
                .saturating_sub(SLEEP_BELOW)
                .min(LONGEST_WAIT)
        });
        let mut waited = false;
        for (index, (program, what)) in senders.iter().enumerate() {
            if ended[index] {
                continue;
            }
            let patience = if waited { Duration::ZERO } else { wait };
            waited = true;
            let state = backend.poll_program(*program, patience)?;
            lines[index].extend(state.lines);
            if let Some(exit) = state.exit {
                if !exit.success() {
                    return Err(failure(
                        what.clone(),
                        format!("the agent ended with {exit}: {}", state.stderr.trim()),
                    ));
                }
                ended[index] = true;
            }
        }
        if ended.iter().all(|done| *done) && pending.is_empty() {
            break;
        }
        if !waited {
            // Every sender has ended and a step is still to come: wait for it
            std::thread::sleep(wait.max(Duration::from_millis(1)));
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "run the workloads",
                "the agents did not end in time",
            ));
        }
    }
    let mut sent = Vec::new();
    for ((_, what), lines) in senders.iter().zip(&lines) {
        match lines.last().map(|line| event(line, what)).transpose()? {
            Some(Event::Sent(summary)) => sent.push(summary),
            other => {
                return Err(failure(
                    what.clone(),
                    format!("the sender should end with 'sent', it said {other:?}"),
                ));
            }
        }
    }
    Ok(Driven { sent, events })
}

/// Does one step and records it.
fn do_step(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    step: &Step,
    begin: Begin,
) -> Result<EventRecord, Error> {
    let since = |ns: u64| ns.saturating_sub(begin.ns);
    let started = since(monotonic_ns());
    let link = &scenario.topology.links[step.link];
    let handle = provisioned.links[step.link].1;
    let direction = match &step.action {
        Action::Impair {
            direction,
            impairments,
            ..
        } => {
            backend.change(handle, *direction, impairments)?;
            Some(match direction {
                Direction::Forward => "forward".to_owned(),
                Direction::Reverse => "reverse".to_owned(),
            })
        }
        Action::Down | Action::Up => {
            // A backend that declares `Link down and up` has to say how; none does yet
            return Err(failure(
                format!("do the step '{}' of the timeline", step.kind.word()),
                "this build of the run cannot take a link down or up",
            ));
        }
    };
    Ok(EventRecord {
        event: step.event,
        kind: step.kind.word().to_owned(),
        link: link.id.to_string(),
        direction,
        scheduled_ns: step.at.as_nanos(),
        started_ns: started,
        applied_ns: since(monotonic_ns()),
    })
}
