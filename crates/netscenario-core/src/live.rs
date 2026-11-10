//! Changes of a link from test code while the run goes (TML-05, D-096).
//!
//! A run is one blocking call. To talk to one that is going, make a pair with [`channel`], give the [`LiveInbox`] to the
//! options of the run ([`RunOptions::with_live`](crate::RunOptions::with_live)) and keep the [`LiveHandle`]: from any
//! thread, [`LiveHandle::change`] asks for a change and returns once the backend has done it.
//!
//! A change is a step of the timeline like the steps of the file, and the run records it in the same way. It is done
//! between two packets: the run asks the sender that sends into the direction to hold before its next packet (`hold`),
//! the sender answers with the barrier it already has, with the number of that packet, and the run does the step and
//! lets it go. So the packet that met the old state is known exactly, and the recorded scenario holds the change as an
//! event at the moment of that packet's slot, which a replay binds to the same packet again. A direction that no sender
//! of a running workload sends into is changed by the clock, and says so (`packet` is empty).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::schedule::Schedule;
use netscenario_model::{Change, Duration as Moment, Event, Name, Scenario, True};

use crate::backend::{Backend, Direction, ProgramHandle};
use crate::drive::{Begin, Tracker, do_step};
use crate::error::Error;
use crate::observe::RunEvent;
use crate::provision::Provisioned;
use crate::result::EventRecord;
use crate::run::{Control, failure};
use crate::timeline::{Action, Step};

/// What a live change does to a link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiveAction {
    /// The values of the link change as a `set` of the timeline says: the keys that are written replace the old ones, `null`
    /// takes an impairment away, the others stay.
    Set(Box<Change>),
    /// The interface of the link goes down.
    Down,
    /// The interface of the link comes up again.
    Up,
}

/// A change of a link that the code makes while the run goes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LiveChange {
    /// The link. It must be one of the links the run was declared live for.
    pub link: Name,
    /// What happens to it.
    pub action: LiveAction,
}

impl LiveChange {
    /// A `set` of `link`.
    #[must_use]
    pub fn set(link: Name, change: Change) -> Self {
        Self {
            link,
            action: LiveAction::Set(Box::new(change)),
        }
    }

    /// The interface of `link` goes down.
    #[must_use]
    pub fn down(link: Name) -> Self {
        Self {
            link,
            action: LiveAction::Down,
        }
    }

    /// The interface of `link` comes up.
    #[must_use]
    pub fn up(link: Name) -> Self {
        Self {
            link,
            action: LiveAction::Up,
        }
    }

    fn event(&self, at: Moment) -> Event {
        let mut event = Event::new(at, self.link.clone());
        match &self.action {
            LiveAction::Set(change) => event.set = Some((**change).clone()),
            LiveAction::Down => event.down = Some(True),
            LiveAction::Up => event.up = Some(True),
        }
        event
    }
}

type Reply = Result<Vec<EventRecord>, Error>;

struct Request {
    change: LiveChange,
    reply: SyncSender<Reply>,
}

/// What the handle and the run share about time: when the workloads began, and whether the run is over for the code.
#[derive(Debug, Default)]
struct Clock {
    /// The start of the measured phase on `CLOCK_MONOTONIC`, once the run knows it.
    begin_ns: Mutex<Option<u64>>,
    changed: Condvar,
    over: AtomicBool,
}

impl Clock {
    fn set_begin(&self, begin_ns: u64) {
        *self
            .begin_ns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(begin_ns);
        self.changed.notify_all();
    }

    fn end(&self) {
        self.over.store(true, Ordering::SeqCst);
        self.changed.notify_all();
    }
}

/// The side of the code that asks for changes. It can be cloned and sent to other threads.
#[derive(Debug, Clone)]
pub struct LiveHandle {
    requests: Sender<Request>,
    links: Vec<Name>,
    clock: Arc<Clock>,
}

impl LiveHandle {
    /// The links that may be changed.
    #[must_use]
    pub fn links(&self) -> &[Name] {
        &self.links
    }

    /// Waits until the measured phase of the run is `offset` old (the moment the workloads started is 0). The run starts the
    /// workloads half a second after it reports that they start, so a change that is to happen at a moment of the scenario
    /// waits on this clock and not on the one of the code.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] if the run ends, or its workloads are over, before that moment.
    pub fn wait_until(&self, offset: Duration) -> Result<(), Error> {
        let begin = {
            let mut guard = self
                .clock
                .begin_ns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(begin) = *guard {
                    break begin;
                }
                if self.clock.over.load(Ordering::SeqCst) {
                    return Err(over());
                }
                guard = self
                    .clock
                    .changed
                    .wait_timeout(guard, Duration::from_millis(50))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
        };
        let target = begin.saturating_add(u64::try_from(offset.as_nanos()).unwrap_or(u64::MAX));
        loop {
            if self.clock.over.load(Ordering::SeqCst) {
                return Err(over());
            }
            let now = monotonic_ns();
            if now >= target {
                return Ok(());
            }
            std::thread::sleep(Duration::from_nanos(target - now).min(Duration::from_millis(10)));
        }
    }

    /// How old the measured phase is, or `None` before the workloads started.
    #[must_use]
    pub fn elapsed(&self) -> Option<Duration> {
        let begin = (*self
            .clock
            .begin_ns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        Some(Duration::from_nanos(monotonic_ns().saturating_sub(begin)))
    }

    /// Changes a link and waits until the backend has done it. Returns the record of each step the change made (a `set` of
    /// `both` makes two), as they are in `events` of the result.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidScenario`] if the change is not valid for the link in the state it is in,
    /// - [`Error::Capability`] if the backend cannot do it faithfully, with the feature and the backend named,
    /// - [`Error::Run`] if the link was not declared live, the workloads are over, or the run has ended,
    /// - [`Error::Backend`] if the backend failed to do it (the run fails as well).
    pub fn change(&self, change: LiveChange) -> Result<Vec<EventRecord>, Error> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.requests
            .send(Request { change, reply })
            .map_err(|_| over())?;
        answer.recv().map_err(|_| over())?
    }
}

fn over() -> Error {
    failure(
        "change a link from test code",
        "the run has ended, or its workloads are over",
    )
}

/// The side of the run: it takes the requests. Give it to [`RunOptions::with_live`](crate::RunOptions::with_live).
#[derive(Debug)]
pub struct LiveInbox {
    requests: Mutex<Option<Receiver<Request>>>,
    links: Vec<Name>,
    clock: Arc<Clock>,
}

impl Drop for LiveInbox {
    fn drop(&mut self) {
        // Whoever waits for the run does not wait for ever
        self.clock.end();
    }
}

impl LiveInbox {
    /// The links the run is declared live for.
    #[must_use]
    pub fn links(&self) -> &[Name] {
        &self.links
    }

    fn take(&self) -> Option<Request> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()?
            .try_recv()
            .ok()
    }

    /// Ends the exchange for good: whoever asks after this, or waits for a moment of the run, gets the answer that the run is over. The
    /// run calls it on every way out, so a handle is never left waiting for a run that has ended.
    pub(crate) fn close(&self) {
        self.clock.end();
        self.shut();
    }

    /// Takes no more requests: the code that asks after this gets its answer at once, and one that is on its way too.
    fn shut(&self) {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("change", &self.change)
            .finish_non_exhaustive()
    }
}

/// A handle and the inbox of the run it talks to, for the links `links` (the links the code may change; their queues are built
/// so that any state fits).
#[must_use]
pub fn channel(links: Vec<Name>) -> (LiveHandle, LiveInbox) {
    let (requests, receiver) = mpsc::channel();
    let clock = Arc::new(Clock::default());
    (
        LiveHandle {
            requests,
            links: links.clone(),
            clock: Arc::clone(&clock),
        },
        LiveInbox {
            requests: Mutex::new(Some(receiver)),
            links,
            clock,
        },
    )
}

/// What the run needs to know of a sender to hold it.
pub(crate) struct LiveSender {
    pub(crate) from: Name,
    pub(crate) to: Name,
    pub(crate) schedule: Schedule,
    /// When it starts and how long it sends, in nanoseconds from the start of the measured phase.
    pub(crate) start_ns: u64,
    pub(crate) duration_ns: u64,
}

/// The longest the run waits for a sender to answer a hold. A sender answers before its next packet, within a gap; this is the
/// time after which something is broken.
const HOLD_TIMEOUT: Duration = Duration::from_secs(10);

struct Waiting {
    id: u32,
    sender: usize,
    step: Step,
    asked: Instant,
}

struct Active {
    reply: SyncSender<Reply>,
    steps: VecDeque<Step>,
    records: Vec<EventRecord>,
    requested_ns: u64,
    waiting: Option<Waiting>,
}

/// The state of the loop of the run that serves the live changes.
pub(crate) struct LiveRun<'a> {
    inbox: &'a LiveInbox,
    senders: Vec<LiveSender>,
    /// The scenario with the live changes that were done, in the order they were done: what a request is checked against.
    candidate: Scenario,
    /// The live changes as events of a scenario, at the moments of their packets' slots: what a replay needs.
    pub(crate) recorded: Vec<Event>,
    /// Sentences for the notes of the result.
    pub(crate) notes: Vec<String>,
    next_id: u32,
    active: Option<Active>,
}

/// What the loop of the run lends to the live changes for one call.
pub(crate) struct Env<'a> {
    pub(crate) backend: &'a mut dyn Backend,
    pub(crate) provisioned: &'a Provisioned,
    pub(crate) begin: Begin,
    pub(crate) tracker: &'a mut Tracker,
    pub(crate) programs: &'a [(ProgramHandle, String)],
    pub(crate) ended: &'a [bool],
    pub(crate) control: &'a Control<'a>,
    pub(crate) events: &'a mut Vec<EventRecord>,
    pub(crate) holds: &'a mut HashMap<u32, usize>,
}

impl<'a> LiveRun<'a> {
    pub(crate) fn new(
        inbox: &'a LiveInbox,
        senders: Vec<LiveSender>,
        scenario: &Scenario,
        first_id: u32,
        begin_ns: u64,
    ) -> Self {
        inbox.clock.set_begin(begin_ns);
        Self {
            inbox,
            senders,
            candidate: scenario.clone(),
            recorded: Vec::new(),
            notes: Vec::new(),
            next_id: first_id,
            active: None,
        }
    }

    /// Whether a sender is held for a change: the loop looks at the senders often then, and does not take the time for the
    /// queues.
    pub(crate) fn is_waiting(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.waiting.is_some())
    }

    /// Whether the number is one of a hold.
    pub(crate) fn owns(&self, id: u32) -> bool {
        self.active
            .as_ref()
            .and_then(|active| active.waiting.as_ref())
            .is_some_and(|waiting| waiting.id == id)
    }

    /// Takes the requests that are there and goes on with the one that is going.
    pub(crate) fn work(&mut self, env: &mut Env<'_>) -> Result<(), Error> {
        loop {
            if self.active.is_none() {
                let Some(request) = self.inbox.take() else {
                    return Ok(());
                };
                self.accept(request, env);
            }
            let Some(active) = self.active.as_mut() else {
                continue;
            };
            if let Some(waiting) = &active.waiting {
                if waiting.asked.elapsed() > HOLD_TIMEOUT {
                    return self.fail(failure(
                        "hold the sender for a change from test code",
                        "the sender did not answer",
                    ));
                }
                return Ok(());
            }
            let Some(step) = active.steps.pop_front() else {
                // Every step is done
                if let Some(active) = self.active.take() {
                    let _ = active.reply.send(Ok(active.records));
                }
                continue;
            };
            match self.pick(&step, env.ended) {
                Some(sender) => {
                    let id = self.next_id;
                    self.next_id += 1;
                    let program = env.programs[sender].0;
                    // A sender that has ended in the meantime has nobody to read it: the change is done by the clock
                    if env
                        .backend
                        .send_line(program, &format!("hold {id}"))
                        .is_ok()
                    {
                        if let Some(active) = self.active.as_mut() {
                            active.waiting = Some(Waiting {
                                id,
                                sender,
                                step,
                                asked: Instant::now(),
                            });
                        }
                        return Ok(());
                    }
                    self.done(env, &step, None)?;
                }
                None => {
                    self.done(env, &step, None)?;
                }
            }
        }
    }

    /// The sender asked the barrier of a hold: the packet is held, the step is done and the sender goes on.
    pub(crate) fn barrier(
        &mut self,
        env: &mut Env<'_>,
        sender: usize,
        id: u32,
        packet: u64,
    ) -> Result<(), Error> {
        let Some(waiting) = self
            .active
            .as_mut()
            .and_then(|active| active.waiting.take())
            .filter(|waiting| waiting.id == id && waiting.sender == sender)
        else {
            return Err(failure(
                "hold the sender for a change from test code",
                format!("the sender asked a barrier ({id}) that the run did not ask for"),
            ));
        };
        let at = self.done(env, &waiting.step, Some((sender, packet)))?;
        env.backend
            .send_line(env.programs[sender].0, &format!("go {id}"))?;
        env.holds.insert(id, at);
        self.work(env)
    }

    /// A sender has ended. If a change waited for it, the change is done by the clock now.
    pub(crate) fn sender_ended(&mut self, env: &mut Env<'_>, sender: usize) -> Result<(), Error> {
        let waits = self
            .active
            .as_ref()
            .and_then(|active| active.waiting.as_ref())
            .is_some_and(|waiting| waiting.sender == sender);
        if waits {
            if let Some(waiting) = self
                .active
                .as_mut()
                .and_then(|active| active.waiting.take())
            {
                self.done(env, &waiting.step, None)?;
            }
            self.work(env)?;
        }
        Ok(())
    }

    /// The workloads are over, or the run is cancelled: nobody is served any more.
    pub(crate) fn close(&mut self) {
        self.inbox.clock.end();
        if let Some(active) = self.active.take() {
            let _ = active.reply.send(Err(over()));
        }
        while let Some(request) = self.inbox.take() {
            let _ = request.reply.send(Err(over()));
        }
        self.inbox.shut();
    }

    fn fail(&mut self, error: Error) -> Result<(), Error> {
        let message = error.to_string();
        if let Some(active) = self.active.take() {
            let _ = active
                .reply
                .send(Err(failure("change a link from test code", message)));
        }
        Err(error)
    }

    /// Checks a request and makes its steps the work in hand; a request that is refused is answered at once.
    fn accept(&mut self, request: Request, env: &Env<'_>) {
        let Request { change, reply } = request;
        let requested_ns = monotonic_ns().saturating_sub(env.begin.ns);
        match self.prepare(&change, requested_ns, env.backend) {
            Ok(steps) => {
                self.active = Some(Active {
                    reply,
                    steps,
                    records: Vec::new(),
                    requested_ns,
                    waiting: None,
                });
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
        }
    }

    /// The steps of a change, if it is allowed: the link is live, the measured phase is not over, the scenario with the
    /// change is valid, and the backend can do all of it.
    fn prepare(
        &self,
        change: &LiveChange,
        requested_ns: u64,
        backend: &dyn Backend,
    ) -> Result<VecDeque<Step>, Error> {
        if !self.candidate.live_links.contains(&change.link) {
            return Err(failure(
                "change a link from test code",
                format!(
                    "link '{}' was not declared live; name it when the run is started (live=[...])",
                    change.link
                ),
            ));
        }
        let at = Moment::from_nanos(requested_ns);
        if Duration::from(at) >= Duration::from(self.candidate.duration) {
            return Err(failure(
                "change a link from test code",
                "the measured phase of the scenario is over",
            ));
        }
        let mut probe = self.candidate.clone();
        probe.timeline.push(change.event(at));
        let problems = crate::provision::checked(&probe);
        if !problems.is_empty() {
            return Err(Error::InvalidScenario(problems));
        }
        crate::capability::check(&probe, &backend.capabilities())?;
        backend.check_values(&probe)?;
        // The steps the event makes in a scenario of its own: the directions and the edits are all that is taken from them
        let mut alone = self.candidate.clone();
        alone.timeline = vec![change.event(at)];
        Ok(crate::timeline::steps(&alone).into())
    }

    /// The first sender that has not ended and sends into the direction of the step (a step of the whole link: either way).
    fn pick(&self, step: &Step, ended: &[bool]) -> Option<usize> {
        let link = self.candidate.topology.links.get(step.link)?;
        let ends: Vec<(&Name, &Name)> = match &step.action {
            Action::Impair {
                direction: Direction::Forward,
                ..
            } => vec![(&link.between[0], &link.between[1])],
            Action::Impair {
                direction: Direction::Reverse,
                ..
            } => vec![(&link.between[1], &link.between[0])],
            Action::Down | Action::Up => vec![
                (&link.between[0], &link.between[1]),
                (&link.between[1], &link.between[0]),
            ],
            Action::Blackout { .. } => return None,
        };
        self.senders
            .iter()
            .enumerate()
            .find(|(index, sender)| {
                !ended.get(*index).copied().unwrap_or(true)
                    && ends.contains(&(&sender.from, &sender.to))
            })
            .map(|(index, _)| index)
    }

    /// Does a step, records it and the event that replays it. `at_barrier` is the sender that was held and the packet it
    /// was held before. Returns the place of the record in `events`.
    fn done(
        &mut self,
        env: &mut Env<'_>,
        step: &Step,
        at_barrier: Option<(usize, u64)>,
    ) -> Result<usize, Error> {
        let requested_ns = self.active.as_ref().map_or(0, |active| active.requested_ns);
        env.tracker.mark_live(step);
        let mut record = match do_step(
            env.backend,
            &self.candidate,
            env.provisioned,
            step,
            at_barrier.map(|(_, packet)| packet),
            env.begin,
            env.tracker,
        ) {
            Ok(record) => record,
            Err(error) => {
                self.fail(failure("change a link from test code", error.to_string()))
                    .ok();
                return Err(error);
            }
        };
        record.live = true;
        record.event = self.recorded.len();
        record.scheduled_ns = requested_ns;
        // The moment a replay binds to the same packet: the slot of the packet in the schedule of the sender; for a step by the
        // clock, the moment it was done
        let at = at_barrier
            .and_then(|(sender, packet)| self.slot_of(sender, packet))
            .unwrap_or(record.applied_ns);
        let event = event_of(&self.candidate, step, Moment::from_nanos(at));
        self.candidate.timeline.push(event.clone());
        self.recorded.push(event);
        if let Some((sender, packet)) = at_barrier {
            let one = &self.senders[sender];
            if one.schedule.packets_in(at.saturating_sub(one.start_ns)) != packet {
                self.notes.push(format!(
                    "the change before the packet {packet} of the workload from '{}' to '{}' is not recorded at a moment that binds to that packet again (two packets leave at one moment), so a replay may change the link one packet off",
                    one.from, one.to
                ));
            }
        }
        env.control.emit(&RunEvent::StepDone {
            event: record.event,
            lateness: Duration::from_nanos(record.applied_ns.saturating_sub(record.scheduled_ns)),
        });
        let place = env.events.len();
        env.events.push(record.clone());
        if let Some(active) = self.active.as_mut() {
            active.records.push(record);
        }
        Ok(place)
    }

    /// The moment, in nanoseconds from the start of the measured phase, at which packet `packet` of the sender leaves.
    fn slot_of(&self, sender: usize, packet: u64) -> Option<u64> {
        let one = &self.senders[sender];
        let offset = one
            .schedule
            .offsets(one.duration_ns)
            .nth(usize::try_from(packet).ok()?)?;
        Some(one.start_ns.saturating_add(offset))
    }
}

/// The event of a scenario that does what a step did, at `at`.
fn event_of(scenario: &Scenario, step: &Step, at: Moment) -> Event {
    let link = scenario.topology.links[step.link].id.clone();
    let mut event = Event::new(at, link);
    match &step.action {
        Action::Impair { direction, .. } => {
            let mut change = Change::default();
            match direction {
                Direction::Forward => change.forward.clone_from(&step.edit),
                Direction::Reverse => change.reverse.clone_from(&step.edit),
            }
            event.set = Some(change);
        }
        Action::Down => event.down = Some(True),
        Action::Up => event.up = Some(True),
        Action::Blackout { .. } => {}
    }
    event
}
