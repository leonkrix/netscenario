//! The loop of a run while the workloads go: it waits for the senders, and does the steps of the timeline
//! (D-060, D-062).
//!
//! A step is done in one of two ways.
//!
//! - **At a barrier** (D-038). The sender of the workload that sends into the direction of the step knows its
//!   packet clock, so it knows which packet is the first one at or after the moment of the step. Right after it has
//!   sent the packet before, it asks (`barrier <id>`); the loop does the step and answers (`go <id>`); and at the slot
//!   of the packet the sender goes on, waiting only if the answer is not there yet. For the packets of that workload
//!   this is the same as a step at the exact moment, and it does not depend on what the machine does: the same seed
//!   gives the same lost packets.
//! - **By the clock.** The loop asks the backend at the moment the file gives, counted from the moment the
//!   workloads start. This is for a step that no workload can wait for: no workload sends into its direction, or
//!   the moment lies after the last packet, and for the two steps of an outage, which is a window in the time of
//!   the receiver (D-064). A packet that is sent between the moment and the end of the change
//!   meets the old state, and which one differs from run to run.
//!
//! The result records both ways (`packet` is set for a barrier).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::events::{Event, SendSummary};
use netscenario_model::{Impairments, Scenario};

use crate::backend::{Backend, Direction, ProgramHandle};
use crate::error::Error;
use crate::live::{Env, LiveRun};
use crate::observe::RunEvent;
use crate::provision::Provisioned;
use crate::result::EventRecord;
use crate::run::{Control, event, failure};
use crate::timeline::{Action, Step};

/// When the workloads begin, on `CLOCK_MONOTONIC`, the clock of the records and of the senders.
///
/// A step is waited for on this clock and not on an `Instant` taken next to it: two reads one after the other differ by
/// a few microseconds, more when the machine stalls between them, and a step then began before its moment on the clock
/// of its own record (3 µs on Ubuntu 22.04 in the CI).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Begin {
    pub(crate) ns: u64,
}

impl Begin {
    /// How long until `after` nanoseconds from the beginning, on the clock of the records; zero if it has passed.
    fn left_until(self, after: u64) -> Duration {
        self.ns
            .saturating_add(after)
            .checked_sub(monotonic_ns())
            .map_or(Duration::ZERO, Duration::from_nanos)
    }
}

/// The step that a sender waits for: which sender, and before which of its packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) sender: usize,
    pub(crate) packet: u64,
}

/// The values each direction of each link has now, so that a step that comes after a change from test code (D-096) is worked out
/// from the state the direction is in and not from the state the file leads to.
pub(crate) struct Tracker {
    current: Vec<[Impairments; 2]>,
    /// Whether test code has changed the direction: from then on the steps of the file are worked out again.
    touched: Vec<[bool; 2]>,
}

const fn way(direction: Direction) -> usize {
    match direction {
        Direction::Forward => 0,
        Direction::Reverse => 1,
    }
}

impl Tracker {
    pub(crate) fn new(scenario: &Scenario) -> Self {
        Self {
            current: scenario
                .topology
                .links
                .iter()
                .map(|link| {
                    [Direction::Forward, Direction::Reverse]
                        .map(|direction| crate::timeline::initial(link, direction))
                })
                .collect(),
            touched: vec![[false; 2]; scenario.topology.links.len()],
        }
    }

    /// Notes that the step is a change from test code.
    pub(crate) fn mark_live(&mut self, step: &Step) {
        if let Action::Impair { direction, .. } = &step.action {
            self.touched[step.link][way(*direction)] = true;
        }
    }

    /// The state to write for an impairing step, and the new state of the direction.
    fn state(&mut self, step: &Step, direction: Direction, planned: &Impairments) -> Impairments {
        let place = (step.link, way(direction));
        let next = match &step.edit {
            Some(edit) if self.touched[place.0][place.1] => {
                edit.applied_to(&self.current[place.0][place.1])
            }
            _ => planned.clone(),
        };
        self.current[place.0][place.1] = next.clone();
        next
    }
}

/// What the loop found out.
pub(crate) struct Driven {
    /// What each sender said at its end, in the order of the senders.
    pub(crate) sent: Vec<SendSummary>,
    /// The steps that were done, in the order they were done.
    pub(crate) events: Vec<EventRecord>,
    /// If the run was cancelled and keeps what it has (D-066): when the senders were told to stop, in nanoseconds from
    /// the start of the workloads.
    pub(crate) interrupted_ns: Option<u64>,
    /// The changes from test code, as events of a scenario at the slots of the packets they met (D-096).
    pub(crate) live_events: Vec<netscenario_model::Event>,
    /// What to say about the live changes in the notes of the result.
    pub(crate) live_notes: Vec<String>,
}

/// Below this a wait for a step is a sleep to the exact moment and not a wait for news of a sender: the wait for
/// news counts whole milliseconds, and a step must not be due a millisecond late because of that.
const SLEEP_BELOW: Duration = Duration::from_millis(3);

/// The longest the loop waits for news of a sender when nothing is due, and so the longest between two looks at the
/// programs of your own (50 ms), which is the resolution of the time of a metric.
const LONGEST_WAIT: Duration = Duration::from_millis(50);

/// How long the loop waits for news of a sender while a barrier is still to come: a sender asks right after the packet
/// before the barrier, and the packet waits for the answer, so the loop looks often.
const BARRIER_WAIT: Duration = Duration::from_millis(1);

/// What the loop also does while it goes (D-072): looks at the programs of your own, and does not end before `hold_until`,
/// the end of the scenario, so that a program runs to the end also when no sender does.
pub(crate) struct Watch<'a> {
    pub(crate) programs: &'a mut crate::programs::Programs,
    pub(crate) hold_until: Instant,
    /// Looks at the queues once a second (PRB-10), while no sender may wait for a barrier.
    pub(crate) queues: &'a mut crate::queues::Sampler,
    /// The changes from test code, if the run takes them (D-096).
    pub(crate) live: Option<LiveRun<'a>>,
}

/// Waits for the senders to end and does the steps of the timeline: at the barriers where `bindings` has one, else by the
/// clock.
///
/// # Errors
///
/// - [`Error::Cancelled`] if the run is cancelled (looked at before every wait),
/// - [`Error::Backend`] if a step cannot be done,
/// - [`Error::Run`] if a sender fails, says something else than `sent` at its end, does not end in time, ends
///   without having asked a barrier, or went on without an answer.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn drive(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    senders: &[(ProgramHandle, String)],
    steps: &[Step],
    bindings: &[Option<Binding>],
    begin: Begin,
    deadline: Instant,
    watch: Watch<'_>,
    control: &Control<'_>,
) -> Result<Driven, Error> {
    let Watch {
        programs,
        hold_until,
        queues,
        mut live,
    } = watch;
    let mut tracker = Tracker::new(scenario);
    // The steps that are done by the clock, in order; the others are done when their sender asks
    let mut pending: VecDeque<(usize, &Step)> = steps
        .iter()
        .enumerate()
        .filter(|(place, _)| bindings[*place].is_none())
        .collect();
    let mut at_barriers = bindings.iter().filter(|binding| binding.is_some()).count();
    let mut lines: Vec<Vec<String>> = vec![Vec::new(); senders.len()];
    let mut ended = vec![false; senders.len()];
    let mut events: Vec<EventRecord> = Vec::new();
    // Where in `events` the record of a step is, by the id of its barrier
    let mut recorded: HashMap<u32, usize> = HashMap::new();
    let mut interrupted_ns: Option<u64> = None;
    loop {
        control.checkpoint()?;
        programs.watch(backend)?;
        if let Some(live) = live.as_mut() {
            live.work(&mut Env {
                backend,
                provisioned,
                begin,
                tracker: &mut tracker,
                programs: senders,
                ended: &ended,
                control,
                events: &mut events,
                holds: &mut recorded,
            })?;
        }
        // A look at the queues costs a `tc` process or more, a few milliseconds: not while a sender may ask for a barrier and wait
        // for the answer
        if at_barriers == 0 && !live.as_ref().is_some_and(LiveRun::is_waiting) {
            queues.look(backend, begin.ns)?;
        }
        // A cancel that keeps the result (D-066): every sender is told to stop and no step is done any more. The
        // senders end at their next packet and say what they sent; the loop waits for that like for their end.
        if interrupted_ns.is_none() && control.take_interruption() {
            interrupted_ns = Some(monotonic_ns().saturating_sub(begin.ns));
            for (index, (program, _)) in senders.iter().enumerate() {
                if !ended[index] {
                    // A sender that has ended in the meantime has nobody to read it: that is not an error
                    let _ = backend.send_line(*program, "stop");
                }
            }
            pending.clear();
            at_barriers = 0;
            if let Some(live) = live.as_mut() {
                live.close();
            }
        }
        // The steps by the clock that are due, in order. A step that is a moment away is waited for to the
        // microsecond.
        while let Some((_, step)) = pending.front() {
            let left = begin.left_until(step.at.as_nanos());
            if !left.is_zero() {
                // While a sender may ask for a barrier the loop does not sleep: a sleep of up to 3 ms would hold the
                // sender's packet back and let this step overtake the barrier that came before it. It waits for news
                // for a millisecond at a time below instead, and does the step when the moment has come.
                if left > SLEEP_BELOW || at_barriers > 0 {
                    break;
                }
                std::thread::sleep(left);
            }
            let record = do_step(
                backend,
                scenario,
                provisioned,
                step,
                None,
                begin,
                &mut tracker,
            )?;
            tell(control, &record);
            events.push(record);
            pending.pop_front();
        }
        // Until the next step is due, or for news: the first sender that is still going is asked to wait for
        // news, the others are only looked at
        let wait = if at_barriers > 0 {
            // A millisecond at a time, and not beyond the next step by the clock
            pending.front().map_or(BARRIER_WAIT, |(_, step)| {
                begin.left_until(step.at.as_nanos()).min(BARRIER_WAIT)
            })
        } else {
            pending.front().map_or(LONGEST_WAIT, |(_, step)| {
                begin
                    .left_until(step.at.as_nanos())
                    .saturating_sub(SLEEP_BELOW)
                    .min(LONGEST_WAIT)
            })
        };
        // A request from test code is taken within a millisecond
        let wait = if live.is_some() {
            wait.min(BARRIER_WAIT)
        } else {
            wait
        };
        let mut waited = false;
        for (index, (program, what)) in senders.iter().enumerate() {
            if ended[index] {
                continue;
            }
            let patience = if waited { Duration::ZERO } else { wait };
            waited = true;
            let state = backend.poll_program(*program, patience)?;
            let before = lines[index].len();
            lines[index].extend(state.lines);
            // A sender that asks is answered at once, in the order it asked
            for line in &lines[index][before..] {
                let Event::Barrier { id, packet, .. } = event(line, what)? else {
                    continue;
                };
                // A sender that asked just before it was told to stop is not answered: it ends
                if interrupted_ns.is_some() {
                    continue;
                }
                // The barrier of a hold that a change from test code asked for (D-096)
                if let Some(live) = live.as_mut().filter(|live| live.owns(id)) {
                    live.barrier(
                        &mut Env {
                            backend,
                            provisioned,
                            begin,
                            tracker: &mut tracker,
                            programs: senders,
                            ended: &ended,
                            control,
                            events: &mut events,
                            holds: &mut recorded,
                        },
                        index,
                        id,
                        packet,
                    )?;
                    continue;
                }
                let place = usize::try_from(id).unwrap_or(usize::MAX);
                let Some(binding) = bindings.get(place).copied().flatten() else {
                    return Err(failure(
                        what.clone(),
                        format!("the sender asked for a barrier ({id}) that the run did not plan"),
                    ));
                };
                if binding.sender != index || recorded.contains_key(&id) {
                    return Err(failure(
                        what.clone(),
                        format!("the barrier {id} was asked by the wrong sender, or twice"),
                    ));
                }
                let record = do_step(
                    backend,
                    scenario,
                    provisioned,
                    &steps[place],
                    Some(binding.packet),
                    begin,
                    &mut tracker,
                )?;
                backend.send_line(*program, &format!("go {id}"))?;
                tell(control, &record);
                recorded.insert(id, events.len());
                events.push(record);
                at_barriers -= 1;
            }
            if let Some(exit) = state.exit {
                if !exit.success() {
                    return Err(failure(
                        what.clone(),
                        format!("the agent ended with {exit}: {}", state.stderr.trim()),
                    ));
                }
                ended[index] = true;
                // A change that waited for this sender is done by the clock
                if let Some(live) = live.as_mut() {
                    live.sender_ended(
                        &mut Env {
                            backend,
                            provisioned,
                            begin,
                            tracker: &mut tracker,
                            programs: senders,
                            ended: &ended,
                            control,
                            events: &mut events,
                            holds: &mut recorded,
                        },
                        index,
                    )?;
                }
            }
        }
        if ended.iter().all(|done| *done) {
            if at_barriers > 0 {
                return Err(failure(
                    "run the workloads",
                    format!(
                        "{at_barriers} barrier(s) of the timeline were never asked by a sender that has ended"
                    ),
                ));
            }
            // A cancel that keeps the result also ends the wait for the programs: they are stopped next
            if pending.is_empty()
                && !live.as_ref().is_some_and(LiveRun::is_waiting)
                && (Instant::now() >= hold_until || interrupted_ns.is_some())
            {
                break;
            }
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
    let (live_events, live_notes) = match live.as_mut() {
        Some(live) => {
            live.close();
            (
                std::mem::take(&mut live.recorded),
                std::mem::take(&mut live.notes),
            )
        }
        None => (Vec::new(), Vec::new()),
    };
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
    // What each sender lived through at its barriers: how long the packet waited, and whether it went on without
    // an answer, which the guarantee of D-038 does not allow
    for (summary, (_, what)) in sent.iter().zip(senders) {
        if summary.refused_unrecorded > 0 {
            return Err(failure(
                what.clone(),
                format!(
                    "the kernel refused packets in more than {} separate runs ({} more packets are not recorded), so the run cannot say which packets were sent and its statistics would be wrong",
                    netscenario_agent::sender::MAX_REFUSED_RUNS,
                    summary.refused_unrecorded
                ),
            ));
        }
        if summary.barrier_timeouts > 0 {
            return Err(failure(
                what.clone(),
                format!(
                    "the sender went on without an answer to {} barrier(s); the run was too slow to answer",
                    summary.barrier_timeouts
                ),
            ));
        }
        for barrier in &summary.barriers {
            if let Some(&at) = recorded.get(&barrier.id) {
                events[at].held_ns = Some(barrier.held_ns);
            }
        }
    }
    Ok(Driven {
        sent,
        events,
        interrupted_ns,
        live_events,
        live_notes,
    })
}

/// Tells the observer that a step was done.
fn tell(control: &Control<'_>, record: &EventRecord) {
    control.emit(&RunEvent::StepDone {
        event: record.event,
        lateness: Duration::from_nanos(record.applied_ns.saturating_sub(record.scheduled_ns)),
    });
}

/// Does one step and records it. `packet` is set for a step done at a barrier.
pub(crate) fn do_step(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    step: &Step,
    packet: Option<u64>,
    begin: Begin,
    tracker: &mut Tracker,
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
            let state = tracker.state(step, *direction, impairments);
            backend.change(handle, *direction, &state)?;
            Some(match direction {
                Direction::Forward => "forward".to_owned(),
                Direction::Reverse => "reverse".to_owned(),
            })
        }
        Action::Blackout { direction, on } => {
            backend.set_outage(handle, *direction, *on)?;
            Some(match direction {
                Direction::Forward => "forward".to_owned(),
                Direction::Reverse => "reverse".to_owned(),
            })
        }
        Action::Down | Action::Up => {
            backend.set_link(handle, matches!(step.action, Action::Up))?;
            // A link has both directions, and a step of the whole link names none
            None
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
        packet,
        held_ns: None,
        from_model: false,
        live: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_counted_from_the_beginning_on_the_clock_of_the_records() {
        let ns = monotonic_ns() + 2_000_000_000;
        let begin = Begin { ns };
        let left = begin.left_until(1_000_000_000);
        assert!(left > Duration::from_millis(2_900), "{left:?}");
        assert!(left <= Duration::from_secs(3), "{left:?}");
        // Nothing is left for a moment that has passed, and a moment far away does not overflow
        assert!(begin.left_until(0) > Duration::ZERO);
        let past = Begin {
            ns: monotonic_ns().saturating_sub(5_000_000_000),
        };
        assert_eq!(past.left_until(1_000_000_000), Duration::ZERO);
        assert!(begin.left_until(u64::MAX) > Duration::ZERO);
    }

    fn two_steps() -> (Scenario, Vec<Step>) {
        let scenario = Scenario::from_yaml(
            "version: 0\nname: t\nduration: 20s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\ntimeline:\n  - { at: 2s, link: l, set: { forward: { delay: 20ms } } }\n  - { at: 4s, link: l, set: { forward: { rate: 2mbit } } }\n",
            "t.yaml",
        )
        .unwrap();
        let steps = crate::timeline::steps(&scenario);
        (scenario, steps)
    }

    fn state_of(step: &Step) -> Impairments {
        match &step.action {
            Action::Impair { impairments, .. } => (**impairments).clone(),
            other => panic!("{other:?}"),
        }
    }

    /// A step of the file that comes after a change from code is worked out from the state the direction is in (D-096): the loss
    /// the code set stays when the step sets a rate. Canary: while the direction is not marked as changed by code, the step is the
    /// state the file leads to, so the tracker changes nothing for a run that is not changed from code.
    #[test]
    fn a_step_after_a_live_change_is_worked_out_from_the_state_the_direction_is_in() {
        let (scenario, steps) = two_steps();
        let mut tracker = Tracker::new(&scenario);
        let first = tracker.state(&steps[0], Direction::Forward, &state_of(&steps[0]));
        assert_eq!(first, state_of(&steps[0]));
        // The code switches a loss on
        let mut live = steps[1].clone();
        live.edit = Some(serde_json::from_str(r#"{"loss":{"model":"random","p":"10%"}}"#).unwrap());
        tracker.mark_live(&live);
        let with_loss = tracker.state(&live, Direction::Forward, &state_of(&live));
        assert!(with_loss.loss.is_some() && with_loss.delay.is_some());
        // The next step of the file sets the rate and keeps the delay and the loss
        let after = tracker.state(&steps[1], Direction::Forward, &state_of(&steps[1]));
        assert!(
            after.rate.is_some() && after.delay.is_some() && after.loss.is_some(),
            "{after:?}"
        );
        // Canary: a tracker that was never told of a change by code writes the state of the file
        let mut plain = Tracker::new(&scenario);
        plain.state(&steps[0], Direction::Forward, &state_of(&steps[0]));
        let untouched = plain.state(&steps[1], Direction::Forward, &state_of(&steps[1]));
        assert!(untouched.loss.is_none(), "{untouched:?}");
    }
}
