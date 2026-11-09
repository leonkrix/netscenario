//! Models of a link over time: periodic patterns, Markov chains and traces from a file (LMD-05, LMD-04, LMD-03), and the
//! repeating events of the timeline (TML-03). D-090, D-091.
//!
//! A model is not a new thing that a backend does. When a run begins, the models and the repeating events are replaced by
//! ordinary events of the timeline ([`Scenario::compile`]), and the steps are made from those, with everything that a step has
//! (D-038): the same seed gives the same events, and with them the same lost packets.
//!
//! A **state** is a complete overlay on the link as the file writes it: the keys it names (`set`) are what the link has while
//! the state lasts, every other key is what the link says. Entering a state, the compiled step sets the keys that differ from
//! the state before, so a state means the same whatever came before it, which a chain with a random order needs.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Problem;
use crate::generators::PhaseDistribution;
use crate::link_trace::{DirectionTrace, TracePoints, trace_events};
use crate::scenario::{
    Change, Direction, Edit, Event, Impairments, ImpairmentsChange, Link, Scenario, True,
};
use crate::units::{Duration, Name, Probability};

/// The most events that the models of one link, or the copies of one repeating event, may make.
pub const MAX_EVENTS_PER_LINK: usize = 10_000;

/// The most events that all models and repeating events of a scenario may make together.
pub const MAX_EVENTS: usize = 50_000;

/// What kind of model a link has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ModelKind {
    /// A list of states that follow each other, each for a time that the file gives, and then again (LMD-05).
    Periodic,
    /// States with a mean time in each and the chances of the next one, drawn from the seed (LMD-04).
    Markov,
    /// The rate, the delay and the loss over time from a file, for each direction (LMD-03, D-091). Last in the list because it was
    /// added last.
    Trace,
}

impl ModelKind {
    /// The word of the file.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Periodic => "periodic",
            Self::Markov => "markov",
            Self::Trace => "trace",
        }
    }
}

/// How a link changes over time: a periodic pattern, a Markov chain of states, or a trace from a file.
///
/// The keys of one kind are refused in the other, each at its own line. New keys are added to this type in later releases:
/// read a file to make one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LinkModel {
    /// `periodic` or `markov`.
    pub kind: ModelKind,
    /// When the model begins, from the start of the measured phase. Before that the link is what the file says. Default `0s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub begin: Option<Duration>,
    /// Periodic and trace: whether the list of states, or the trace, begins again when it is over (the default), or the link is what
    /// the file says after the last state (`false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<bool>,
    /// Markov only: the state the chain begins in. It is written, not drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Name>,
    /// The states. A periodic model goes through them in the order of the file; a Markov model needs at least two. A trace has
    /// none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<ModelState>,
    /// Trace only: the trace of the direction from the first node to the second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<DirectionTrace>,
    /// Trace only: the trace of the direction from the second node to the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<DirectionTrace>,
    /// Trace only: one trace for both directions. Not together with `forward` or `reverse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub both: Option<DirectionTrace>,
}

/// One state of a model.
///
/// It names what the link has while it lasts: a `set` (the keys of an impairment, as in a step of the timeline, a merge patch on
/// the link as the file writes it), an `outage`, or `down`. A state that writes none of them is the link as the file writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelState {
    /// The name of the state. Required in a Markov model, where the chances refer to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Name>,
    /// Periodic only: how long the state lasts.
    #[serde(rename = "for", default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Duration>,
    /// Markov only: how long the state lasts, or its mean (see `distribution`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stay: Option<Duration>,
    /// Markov only: whether the stay is exact (`fixed`) or drawn from an exponential distribution with `stay` as the mean
    /// (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<PhaseDistribution>,
    /// Markov only: the chances of the next state. They add up to 100 %, and none leads back to the state itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub to: BTreeMap<Name, Probability>,
    /// The values of the link while the state lasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Change>,
    /// Nothing is delivered while the state lasts (D-064).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outage: Option<True>,
    /// For an `outage`: the direction it acts on. Both by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// The interface of the link is down while the state lasts (D-039).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<True>,
}

impl Link {
    /// The impairments of the two directions (forward, reverse) as the file writes them, with `change` applied the way a `set` of
    /// the timeline applies it: what a state of a model leads to.
    #[must_use]
    pub fn with_change(&self, change: &Change) -> [Impairments; 2] {
        applied(change, &base_of(self))
    }
}

impl LinkModel {
    /// A model of `kind` with these states, beginning at 0 s.
    #[must_use]
    pub fn new(kind: ModelKind, states: Vec<ModelState>) -> Self {
        Self {
            kind,
            begin: None,
            repeat: None,
            start: None,
            states,
            forward: None,
            reverse: None,
            both: None,
        }
    }

    /// A trace model with the traces of the directions.
    #[must_use]
    pub fn trace(
        forward: Option<DirectionTrace>,
        reverse: Option<DirectionTrace>,
        both: Option<DirectionTrace>,
    ) -> Self {
        Self {
            forward,
            reverse,
            both,
            ..Self::new(ModelKind::Trace, Vec::new())
        }
    }

    /// The traces of a trace model with the direction each one is for, in the order forward, reverse, both.
    #[must_use]
    pub fn traces(&self) -> Vec<(Direction, &DirectionTrace)> {
        [
            (Direction::Forward, &self.forward),
            (Direction::Reverse, &self.reverse),
            (Direction::Both, &self.both),
        ]
        .into_iter()
        .filter_map(|(direction, trace)| trace.as_ref().map(|trace| (direction, trace)))
        .collect()
    }

    /// Whether the list of a periodic model, or a trace, begins again when it is over.
    #[must_use]
    pub fn repeats(&self) -> bool {
        self.repeat.unwrap_or(true)
    }

    /// Whether the model draws numbers: a Markov model does.
    #[must_use]
    pub fn is_random(&self) -> bool {
        self.kind == ModelKind::Markov
    }

    /// The keys that the states write, by name: what the timeline of the file may not change on this link, because the model
    /// would take it back at its next step. A delay, a jitter and a reorder depend on each other, so one of them takes all three.
    #[must_use]
    pub fn keys_written(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = Vec::new();
        for (_, trace) in self.traces() {
            let Some(points) = &trace.points else {
                continue;
            };
            for (name, written) in [
                ("rate", points.writes_rate()),
                ("delay", points.writes_delay()),
                ("loss", points.writes_loss()),
            ] {
                if written && !keys.contains(&name) {
                    keys.push(name);
                }
            }
        }
        for state in &self.states {
            let Some(change) = &state.set else { continue };
            for part in [&change.forward, &change.reverse, &change.both]
                .into_iter()
                .flatten()
            {
                for (name, written) in [
                    ("delay", !part.delay.is_keep()),
                    ("jitter", !part.jitter.is_keep()),
                    ("loss", !part.loss.is_keep()),
                    ("reorder", !part.reorder.is_keep()),
                    ("duplicate", !part.duplicate.is_keep()),
                    ("corrupt", !part.corrupt.is_keep()),
                    ("rate", !part.rate.is_keep()),
                ] {
                    if written && !keys.contains(&name) {
                        keys.push(name);
                    }
                }
            }
        }
        if keys
            .iter()
            .any(|key| ["delay", "jitter", "reorder"].contains(key))
        {
            for key in ["delay", "jitter", "reorder"] {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// Whether the model may give the part of the link written as `side` (`forward`, `reverse` or `both`) a rate: a state sets
    /// one for it, or a trace writes one (a trace that is not read yet may). A `queue` waits for a rate, and the rate may come from
    /// the model and not from the link.
    #[must_use]
    pub fn may_write_rate(&self, side: &str) -> bool {
        let sets_rate = |part: &Option<ImpairmentsChange>| {
            part.as_ref().is_some_and(|part| !part.rate.is_keep())
        };
        let from_states = self.states.iter().any(|state| {
            state.set.as_ref().is_some_and(|change| match side {
                "forward" => sets_rate(&change.forward) || sets_rate(&change.both),
                "reverse" => sets_rate(&change.reverse) || sets_rate(&change.both),
                _ => {
                    sets_rate(&change.both)
                        || (sets_rate(&change.forward) && sets_rate(&change.reverse))
                }
            })
        });
        let writes = |trace: &Option<DirectionTrace>| {
            trace
                .as_ref()
                .is_some_and(|trace| trace.points.as_ref().is_none_or(TracePoints::writes_rate))
        };
        let from_traces = match side {
            "forward" => writes(&self.forward) || writes(&self.both),
            "reverse" => writes(&self.reverse) || writes(&self.both),
            _ => writes(&self.both) || (writes(&self.forward) && writes(&self.reverse)),
        };
        from_states || from_traces
    }

    /// Whether some state is an outage.
    #[must_use]
    pub fn has_outages(&self) -> bool {
        self.states.iter().any(|state| state.outage.is_some())
    }

    /// Whether some state takes the interface down.
    #[must_use]
    pub fn has_downs(&self) -> bool {
        self.states.iter().any(|state| state.down.is_some())
    }
}

impl ModelState {
    /// A periodic state that lasts `length` and changes nothing yet; add what it does with the other methods.
    #[must_use]
    pub fn periodic(length: Duration) -> Self {
        Self {
            id: None,
            length: Some(length),
            stay: None,
            distribution: None,
            to: BTreeMap::new(),
            set: None,
            outage: None,
            direction: None,
            down: None,
        }
    }

    /// A Markov state with a mean stay, and the chances of the next state.
    #[must_use]
    pub fn markov(id: Name, stay: Duration, to: BTreeMap<Name, Probability>) -> Self {
        Self {
            id: Some(id),
            length: None,
            stay: Some(stay),
            distribution: None,
            to,
            set: None,
            outage: None,
            direction: None,
            down: None,
        }
    }

    /// With the name.
    #[must_use]
    pub fn with_id(mut self, id: Name) -> Self {
        self.id = Some(id);
        self
    }

    /// With the values of the link while the state lasts.
    #[must_use]
    pub fn with_set(mut self, set: Change) -> Self {
        self.set = Some(set);
        self
    }

    /// With an outage for the whole stay, in `direction` (both if `None`).
    #[must_use]
    pub fn with_outage(mut self, direction: Option<Direction>) -> Self {
        self.outage = Some(True);
        self.direction = direction;
        self
    }

    /// With the interface down for the whole stay.
    #[must_use]
    pub fn with_down(mut self) -> Self {
        self.down = Some(True);
        self
    }

    /// With exact stays (Markov).
    #[must_use]
    pub fn with_distribution(mut self, distribution: PhaseDistribution) -> Self {
        self.distribution = Some(distribution);
        self
    }
}

/// A source of numbers in `(0, 1]`, one for each link that has a Markov model (D-090).
pub type Uniform = Box<dyn FnMut() -> f64>;

/// What to do about a model of states that makes too many events.
const STATES_HELP: &str = "make the states longer (or the mean stay, for a Markov model), or the measured phase shorter: every event is a change of the link, which takes a few milliseconds";

/// The two directions of a link as the file writes them: forward and reverse.
pub(crate) fn base_of(link: &Link) -> [Impairments; 2] {
    let start = |own: &Option<Impairments>| {
        own.clone()
            .or_else(|| link.both.clone())
            .unwrap_or_default()
    };
    [start(&link.forward), start(&link.reverse)]
}

/// The two directions after a change, in the order the steps of the timeline apply it (`forward`, `reverse`, `both`).
fn applied(change: &Change, base: &[Impairments; 2]) -> [Impairments; 2] {
    let mut result = base.clone();
    for (side, edit) in [
        (Direction::Forward, &change.forward),
        (Direction::Reverse, &change.reverse),
        (Direction::Both, &change.both),
    ] {
        let Some(edit) = edit else { continue };
        if matches!(side, Direction::Forward | Direction::Both) {
            result[0] = edit.applied_to(&result[0]);
        }
        if matches!(side, Direction::Reverse | Direction::Both) {
            result[1] = edit.applied_to(&result[1]);
        }
    }
    result
}

/// The keys that differ between two sets of impairments, written as the edit that leads from the first to the second.
fn difference(from: &Impairments, to: &Impairments) -> ImpairmentsChange {
    fn edit<T: Clone + PartialEq>(from: Option<&T>, to: Option<&T>) -> Edit<T> {
        if from == to {
            Edit::Keep
        } else {
            to.cloned().map_or(Edit::Clear, Edit::Set)
        }
    }
    ImpairmentsChange {
        delay: edit(from.delay.as_ref(), to.delay.as_ref()),
        jitter: edit(from.jitter.as_ref(), to.jitter.as_ref()),
        loss: edit(from.loss.as_ref(), to.loss.as_ref()),
        reorder: edit(from.reorder.as_ref(), to.reorder.as_ref()),
        duplicate: edit(from.duplicate.as_ref(), to.duplicate.as_ref()),
        corrupt: edit(from.corrupt.as_ref(), to.corrupt.as_ref()),
        rate: edit(from.rate.as_ref(), to.rate.as_ref()),
    }
}

/// The change that leads a link from `from` to `to`, or `None` if they are the same.
fn step_between(from: &[Impairments; 2], to: &[Impairments; 2]) -> Option<Change> {
    let forward = difference(&from[0], &to[0]);
    let reverse = difference(&from[1], &to[1]);
    match (forward.is_empty(), reverse.is_empty()) {
        (true, true) => None,
        _ if forward == reverse => Some(Change {
            both: Some(forward),
            ..Change::default()
        }),
        (false, true) => Some(Change {
            forward: Some(forward),
            ..Change::default()
        }),
        (true, false) => Some(Change {
            reverse: Some(reverse),
            ..Change::default()
        }),
        (false, false) => Some(Change {
            forward: Some(forward),
            reverse: Some(reverse),
            ..Change::default()
        }),
    }
}

fn plain(at: Duration, link: &Name) -> Event {
    Event {
        at,
        link: link.clone(),
        direction: None,
        set: None,
        outage: None,
        down: None,
        up: None,
        every: None,
        until: None,
        count: None,
    }
}

/// The events of one link, with the state it is in.
pub(crate) struct Writer<'a> {
    link: &'a Link,
    base: [Impairments; 2],
    now: [Impairments; 2],
    is_down: bool,
    end: Duration,
    /// What to do about a model that makes too many events.
    help: &'static str,
    pub(crate) events: Vec<Event>,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(link: &'a Link, end: Duration, help: &'static str) -> Self {
        let base = base_of(link);
        Self {
            link,
            now: base.clone(),
            base,
            is_down: false,
            end,
            help,
            events: Vec::new(),
        }
    }

    /// The error of a model that would make more events than a link may have.
    pub(crate) fn too_many(&self) -> Problem {
        Problem::new(format!(
            "the model of the link '{}' makes more than {MAX_EVENTS_PER_LINK} events in the measured phase",
            self.link.id
        ))
        .at_path("topology.links")
        .with_help(self.help)
    }

    fn push(&mut self, event: Event) -> Result<(), Problem> {
        if self.events.len() >= MAX_EVENTS_PER_LINK {
            return Err(self.too_many());
        }
        self.events.push(event);
        Ok(())
    }

    /// Moves the direction `side` of the link to the link as the file writes it with `edit` applied (or, with `None`, to the link
    /// as the file writes it), at `at`; the other direction stays as it is. The step sets the keys that differ from the state
    /// before.
    pub(crate) fn move_side(
        &mut self,
        at: Duration,
        side: Direction,
        edit: Option<&ImpairmentsChange>,
    ) -> Result<(), Problem> {
        if at >= self.end {
            return Ok(());
        }
        let mut target = self.now.clone();
        for (index, own) in [(0, Direction::Forward), (1, Direction::Reverse)] {
            if side == own || side == Direction::Both {
                target[index] = edit.map_or_else(
                    || self.base[index].clone(),
                    |edit| edit.applied_to(&self.base[index]),
                );
            }
        }
        if let Some(change) = step_between(&self.now, &target) {
            let set = Event {
                set: Some(change),
                ..plain(at, &self.link.id)
            };
            self.push(set)?;
        }
        self.now = target;
        Ok(())
    }

    /// Enters `state` at `at`, which lasts `length`: the end of the stay before, the change of the values, an outage or a `down`.
    fn enter(&mut self, at: Duration, state: &ModelState, length: Duration) -> Result<(), Problem> {
        if at >= self.end {
            return Ok(());
        }
        if self.is_down {
            let up = Event {
                up: Some(True),
                ..plain(at, &self.link.id)
            };
            self.push(up)?;
            self.is_down = false;
        }
        let target = match &state.set {
            Some(change) => applied(change, &self.base),
            None => self.base.clone(),
        };
        if let Some(change) = step_between(&self.now, &target) {
            let set = Event {
                set: Some(change),
                ..plain(at, &self.link.id)
            };
            self.push(set)?;
        }
        self.now = target;
        if state.outage.is_some() {
            // A window that reaches past the end is cut: nothing that is measured lies behind it
            let room = self.end.as_nanos().saturating_sub(at.as_nanos());
            let outage = Event {
                outage: Some(Duration::from_nanos(length.as_nanos().min(room))),
                direction: state.direction,
                ..plain(at, &self.link.id)
            };
            self.push(outage)?;
        }
        if state.down.is_some() {
            let down = Event {
                down: Some(True),
                ..plain(at, &self.link.id)
            };
            self.push(down)?;
            self.is_down = true;
        }
        Ok(())
    }

    /// The link is what the file says from `at` on: the model is over.
    fn finish(&mut self, at: Duration) -> Result<(), Problem> {
        let rest = ModelState::periodic(Duration::from_nanos(0));
        self.enter(at, &rest, Duration::from_nanos(0))
    }
}

/// The two directions of `link` with the change of a state applied.
pub(crate) fn applied_states(change: &Change, link: &Link) -> [Impairments; 2] {
    applied(change, &base_of(link))
}

/// The events of a periodic model.
pub(crate) fn periodic_events(
    link: &Link,
    model: &LinkModel,
    end: Duration,
) -> Result<Vec<Event>, Problem> {
    periodic(link, model, end)
}

/// The events of a periodic model.
fn periodic(link: &Link, model: &LinkModel, end: Duration) -> Result<Vec<Event>, Problem> {
    let mut writer = Writer::new(link, end, STATES_HELP);
    let mut at = model.begin.unwrap_or(Duration::from_nanos(0));
    let lengths: Vec<Duration> = model
        .states
        .iter()
        .map(|state| state.length.unwrap_or(Duration::from_nanos(0)))
        .collect();
    let period: u64 = lengths.iter().map(|length| length.as_nanos()).sum();
    if model.states.is_empty() || period == 0 {
        return Ok(Vec::new());
    }
    'cycles: loop {
        for (state, length) in model.states.iter().zip(&lengths) {
            if at >= end {
                break 'cycles;
            }
            writer.enter(at, state, *length)?;
            at = Duration::from_nanos(at.as_nanos().saturating_add(length.as_nanos()));
        }
        if !model.repeats() {
            writer.finish(at)?;
            break;
        }
    }
    Ok(writer.events)
}

/// The events of a Markov model, with the numbers of `uniform`.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a stay is a few nanoseconds to some days: far below 2^53, and the drawn number is not negative"
)]
fn markov(
    link: &Link,
    model: &LinkModel,
    end: Duration,
    uniform: &mut dyn FnMut() -> f64,
) -> Result<Vec<Event>, Problem> {
    let mut writer = Writer::new(link, end, STATES_HELP);
    let Some(start) = &model.start else {
        return Ok(Vec::new());
    };
    let Some(mut state) = model
        .states
        .iter()
        .position(|candidate| candidate.id.as_ref() == Some(start))
    else {
        return Ok(Vec::new());
    };
    let mut at = model.begin.unwrap_or(Duration::from_nanos(0));
    while at < end {
        let current = &model.states[state];
        let mean = current.stay.map_or(0.0, |stay| stay.as_nanos() as f64);
        let nanos = match current
            .distribution
            .unwrap_or(PhaseDistribution::Exponential)
        {
            PhaseDistribution::Fixed => mean,
            PhaseDistribution::Exponential => -uniform().ln() * mean,
        };
        // A stay is at least a nanosecond: an outage of no length is refused, and two states at one moment are no state
        let length = Duration::from_nanos((nanos.round() as u64).max(1));
        writer.enter(at, current, length)?;
        at = Duration::from_nanos(at.as_nanos().saturating_add(length.as_nanos()));
        // The next state: one number chooses among the chances in the order of the names
        let draw = uniform();
        let mut sum = 0.0;
        let mut chosen = None;
        for (name, chance) in &current.to {
            sum += chance.as_fraction();
            chosen = Some(name);
            if draw <= sum {
                break;
            }
        }
        let Some(next) = chosen.and_then(|name| {
            model
                .states
                .iter()
                .position(|candidate| candidate.id.as_ref() == Some(name))
        }) else {
            break;
        };
        state = next;
    }
    Ok(writer.events)
}

/// The copies of an event that repeats, the first one included.
pub(crate) fn repeated(event: &Event, end: Duration) -> Result<Vec<Event>, Problem> {
    let Some(every) = event.every.filter(|every| !every.is_zero()) else {
        return Ok(vec![Event {
            every: None,
            until: None,
            count: None,
            ..event.clone()
        }]);
    };
    let stop = event.until.map_or(end, |until| until.min(end));
    let mut copies = Vec::new();
    let mut at = event.at;
    while at < stop
        && event
            .count
            .is_none_or(|count| (copies.len() as u64) < u64::from(count))
    {
        if copies.len() >= MAX_EVENTS_PER_LINK {
            return Err(Problem::new(format!(
                "the event repeats more than {MAX_EVENTS_PER_LINK} times in the measured phase"
            ))
            .with_help("make `every` longer, or end it sooner with `until` or `count`"));
        }
        copies.push(Event {
            at,
            every: None,
            until: None,
            count: None,
            ..event.clone()
        });
        at = Duration::from_nanos(at.as_nanos().saturating_add(every.as_nanos()));
    }
    Ok(copies)
}

/// A scenario with its models and repeating events replaced by the events they make.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Compiled {
    /// The scenario: no model, no event that repeats.
    pub scenario: Scenario,
    /// The place in `scenario.timeline` of the first event that a model made. The ones before come from the file (each event that
    /// repeats as its copies).
    pub first_generated: usize,
}

impl Scenario {
    /// Whether the scenario has a model, or an event that repeats: [`Scenario::compile`] has something to do.
    #[must_use]
    pub fn has_models(&self) -> bool {
        self.topology.links.iter().any(|link| link.model.is_some())
            || self.timeline.iter().any(|event| event.every.is_some())
    }

    /// Whether the scenario has a model that draws numbers from the seed.
    #[must_use]
    pub fn has_random_models(&self) -> bool {
        self.topology
            .links
            .iter()
            .any(|link| link.model.as_ref().is_some_and(LinkModel::is_random))
    }

    /// The scenario with the models and the repeating events replaced by the events they make (D-090): the events of the file
    /// first, each repeating one as its copies, then the events of the models, link by link. The result has no model and no
    /// event that repeats, and is what a run does. `uniform` gives, for a link that has a Markov model, the numbers in
    /// `(0, 1]` to draw from.
    ///
    /// # Errors
    ///
    /// A model or an event that would make more events than the bounds allow ([`MAX_EVENTS_PER_LINK`], [`MAX_EVENTS`]); a
    /// trace whose file has not been read (D-083): without its rows the model would make no events and the link would
    /// silently keep what the file says outside the trace.
    pub fn compile(&self, uniform: &mut dyn FnMut(&Name) -> Uniform) -> Result<Compiled, Problem> {
        if let Some(file) = self.unresolved_link_trace_files().first() {
            return Err(Problem::new(format!(
                "the trace of a link has not been read from its file '{file}'"
            ))
            .with_help(
                "read the files of the scenario before the models are compiled (`resolve_files`)",
            ));
        }
        let mut compiled = self.clone();
        let mut timeline = Vec::new();
        for event in &self.timeline {
            timeline.extend(repeated(event, self.duration)?);
        }
        let first_generated = timeline.len();
        let mut generated = 0;
        for link in &mut compiled.topology.links {
            let Some(model) = link.model.take() else {
                continue;
            };
            let events = match model.kind {
                ModelKind::Periodic => periodic(link, &model, self.duration)?,
                ModelKind::Markov => {
                    let mut numbers = uniform(&link.id);
                    markov(link, &model, self.duration, &mut numbers)?
                }
                ModelKind::Trace => trace_events(link, &model, self.duration)?,
            };
            generated += events.len();
            if generated + timeline.len() > MAX_EVENTS {
                return Err(Problem::new(format!(
                    "the models and the repeating events make more than {MAX_EVENTS} events in the measured phase"
                ))
                .with_help("make the states longer, or the measured phase shorter"));
            }
            timeline.extend(events);
        }
        compiled.timeline = timeline;
        Ok(Compiled {
            scenario: compiled,
            first_generated,
        })
    }
}
