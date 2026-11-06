//! The rules of the models of a link and of the events that repeat (D-090). What the types cannot state: the keys of one kind
//! in the other, the chances of a chain, the states that must be valid on their own, and what the timeline of the file may not
//! touch on a link that has a model.

use std::collections::BTreeSet;

use crate::error::Problem;
use crate::link_model::{LinkModel, ModelKind, ModelState, applied_states, periodic_events};
use crate::scenario::{Event, Link, Scenario};
use crate::suggest::closest;
use crate::units::{Duration, Name, Probability};
use crate::validate::check_impairments;

/// 100 % in millionths of a percent.
const HUNDRED_PERCENT: u64 = 100_000_000;

impl Scenario {
    /// The rules of every model, and of what the timeline may do on a link that has one.
    pub(crate) fn check_models(&self, problems: &mut Vec<Problem>) {
        for (index, link) in self.topology.links.iter().enumerate() {
            let Some(model) = &link.model else { continue };
            let base = format!("topology.links[{index}].model");
            let before = problems.len();
            self.check_model(link, model, &base, problems);
            // The events of a periodic model are made from the file alone, so a bound that they break is known now. A Markov
            // model draws its stays, and the bound is checked when the run begins.
            if problems.len() == before
                && model.kind == ModelKind::Periodic
                && let Err(problem) = periodic_events(link, model, self.duration)
            {
                problems.push(problem.at_path(base.clone()));
            }
            self.check_model_and_timeline(link, model, &base, problems);
        }
    }

    fn check_model(&self, link: &Link, model: &LinkModel, base: &str, problems: &mut Vec<Problem>) {
        let kind = model.kind;
        if let Some(begin) = model.begin
            && begin >= self.duration
        {
            problems.push(
                Problem::new(format!(
                    "the model begins at {begin}, which is not before the end of the measured phase ({})",
                    self.duration
                ))
                .at_path(format!("{base}.begin")),
            );
        }
        if kind != ModelKind::Periodic && model.repeat.is_some() {
            problems.push(belongs_to(
                "repeat",
                ModelKind::Periodic,
                kind,
                format!("{base}.repeat"),
            ));
        }
        if kind != ModelKind::Markov && model.start.is_some() {
            problems.push(belongs_to(
                "start",
                ModelKind::Markov,
                kind,
                format!("{base}.start"),
            ));
        }
        let minimum = if kind == ModelKind::Markov { 2 } else { 1 };
        if model.states.len() < minimum {
            problems.push(
                Problem::new(format!(
                    "a {} model needs at least {minimum} state{}, found {}",
                    kind.word(),
                    if minimum == 1 { "" } else { "s" },
                    model.states.len()
                ))
                .at_path(format!("{base}.states")),
            );
        }
        let ids: Vec<&Name> = model.states.iter().filter_map(|s| s.id.as_ref()).collect();
        let mut seen = BTreeSet::new();
        for (index, state) in model.states.iter().enumerate() {
            let at = format!("{base}.states[{index}]");
            if let Some(id) = &state.id
                && !seen.insert(id)
            {
                problems.push(
                    Problem::new(format!("the state name '{id}' is used twice"))
                        .at_path(format!("{at}.id")),
                );
            }
            Self::check_state(link, state, kind, &at, problems);
        }
        if kind == ModelKind::Markov {
            check_chain(model, base, &ids, problems);
        }
    }

    fn check_state(
        link: &Link,
        state: &ModelState,
        kind: ModelKind,
        at: &str,
        problems: &mut Vec<Problem>,
    ) {
        match kind {
            ModelKind::Periodic => {
                for (key, present) in [
                    ("stay", state.stay.is_some()),
                    ("distribution", state.distribution.is_some()),
                    ("to", !state.to.is_empty()),
                ] {
                    if present {
                        problems.push(belongs_to(
                            key,
                            ModelKind::Markov,
                            kind,
                            format!("{at}.{key}"),
                        ));
                    }
                }
                match state.length {
                    None => problems.push(
                        Problem::new("a state of a periodic model needs `for`: how long it lasts")
                            .at_path(at.to_owned()),
                    ),
                    Some(length) if length.is_zero() => problems.push(
                        Problem::new("a state must last longer than zero")
                            .at_path(format!("{at}.for")),
                    ),
                    Some(_) => {}
                }
            }
            ModelKind::Markov => {
                if state.length.is_some() {
                    problems.push(belongs_to(
                        "for",
                        ModelKind::Periodic,
                        kind,
                        format!("{at}.for"),
                    ));
                }
                if state.id.is_none() {
                    problems.push(
                        Problem::new(
                            "a state of a Markov model needs an `id`: the chances refer to it",
                        )
                        .at_path(at.to_owned()),
                    );
                }
                match state.stay {
                    None => problems.push(
                        Problem::new("a state of a Markov model needs `stay`: how long it lasts, or the mean")
                            .at_path(at.to_owned()),
                    ),
                    Some(stay) if stay.is_zero() => problems.push(
                        Problem::new("a stay must be longer than zero").at_path(format!("{at}.stay")),
                    ),
                    Some(_) => {}
                }
            }
        }
        Self::check_state_effect(link, state, at, problems);
    }

    /// What a state does to the link: an outage or `down`, and the values it sets, which must be a valid link on their own.
    fn check_state_effect(link: &Link, state: &ModelState, at: &str, problems: &mut Vec<Problem>) {
        if state.outage.is_some() && state.down.is_some() {
            problems.push(
                Problem::new("a state is an `outage` or `down`, not both")
                    .at_path(at.to_owned())
                    .with_help("`down` takes the interface away and the sender gets errors; an `outage` loses what leaves and the sender notices nothing"),
            );
        }
        if state.direction.is_some() && state.outage.is_none() {
            problems.push(
                Problem::new("`direction` belongs to `outage`").at_path(format!("{at}.direction")),
            );
        }
        let Some(change) = &state.set else { return };
        let set = format!("{at}.set");
        if change.forward.is_none() && change.reverse.is_none() && change.both.is_none() {
            problems.push(
                Problem::new("this state writes no key, so it changes nothing")
                    .at_path(set.clone())
                    .with_help("write `forward`, `reverse` or `both` with the keys that change, or take `set` away"),
            );
        }
        for (key, part) in [
            ("forward", &change.forward),
            ("reverse", &change.reverse),
            ("both", &change.both),
        ] {
            let Some(part) = part else { continue };
            if part.is_empty() {
                problems.push(
                    Problem::new("this state writes no key, so it changes nothing")
                        .at_path(format!("{set}.{key}")),
                );
            }
            if let crate::scenario::Edit::Set(jitter) = &part.jitter
                && jitter.table.is_some()
            {
                problems.push(
                    Problem::new("a state cannot bring a `table`: `netem` keeps the table that was loaded first")
                        .at_path(format!("{set}.{key}.jitter.table"))
                        .with_help("write `distribution: table` and the new `amount`; the table is the one of the link"),
                );
            }
        }
        // The state on its own: the link as the file writes it, with this overlay. What the link itself gets wrong is reported
        // with the link
        let own = {
            let mut found = Vec::new();
            for side in &crate::link_model::base_of(link) {
                check_impairments(side, &set, &mut found);
            }
            found
        };
        let mut found = Vec::new();
        for side in &applied_states(change, link) {
            check_impairments(side, &set, &mut found);
        }
        let mut reported = BTreeSet::new();
        for problem in found {
            if !own.iter().any(|o| o.message == problem.message)
                && reported.insert(problem.message.clone())
            {
                let message = format!("{} while this state lasts", problem.message);
                problems.push(Problem { message, ..problem });
            }
        }
    }

    /// What the timeline of the file may not do on a link that has a model: the model would undo it at its next step.
    fn check_model_and_timeline(
        &self,
        link: &Link,
        model: &LinkModel,
        base: &str,
        problems: &mut Vec<Problem>,
    ) {
        let written = model.keys_written();
        for (index, event) in self.timeline.iter().enumerate() {
            if event.link != link.id {
                continue;
            }
            let at = format!("timeline[{index}]");
            if let Some(change) = &event.set {
                for (side, part) in [
                    ("forward", &change.forward),
                    ("reverse", &change.reverse),
                    ("both", &change.both),
                ] {
                    let Some(part) = part else { continue };
                    for (key, present) in [
                        ("delay", !part.delay.is_keep()),
                        ("jitter", !part.jitter.is_keep()),
                        ("loss", !part.loss.is_keep()),
                        ("reorder", !part.reorder.is_keep()),
                        ("duplicate", !part.duplicate.is_keep()),
                        ("corrupt", !part.corrupt.is_keep()),
                        ("rate", !part.rate.is_keep()),
                    ] {
                        if present && written.contains(&key) {
                            problems.push(
                                Problem::new(format!(
                                    "the model of the link '{}' writes `{key}` in its states ({base}), and would take this change back at its next step",
                                    link.id
                                ))
                                .at_path(format!("{at}.set.{side}.{key}"))
                                .with_help("change a key that no state writes, or put the change into the states of the model"),
                            );
                        }
                    }
                }
            }
            for (key, present, why) in [
                ("outage", event.outage.is_some(), model.has_outages()),
                ("down", event.down.is_some(), model.has_downs()),
                ("up", event.up.is_some(), model.has_downs()),
            ] {
                if present && why {
                    let what = if key == "outage" {
                        "an outage"
                    } else {
                        "`down`"
                    };
                    problems.push(
                        Problem::new(format!(
                            "the model of the link '{}' has states with {what} ({base}), and the file would not say what happens when they meet",
                            link.id
                        ))
                        .at_path(format!("{at}.{key}"))
                        .with_help("put it into the states of the model, or take the state away"),
                    );
                }
            }
        }
    }
}

fn belongs_to(key: &str, owner: ModelKind, kind: ModelKind, path: String) -> Problem {
    Problem::new(format!(
        "`{key}` belongs to a {} model, and this one is {}",
        owner.word(),
        kind.word()
    ))
    .at_path(path)
}

/// The chances of a Markov chain: they add up to 100 %, lead to other states that exist, and the start exists.
fn check_chain(model: &LinkModel, base: &str, ids: &[&Name], problems: &mut Vec<Problem>) {
    let known: Vec<&str> = ids.iter().map(|id| id.as_str()).collect();
    let list = format!("the states are: {}", known.join(", "));
    match &model.start {
        None => problems.push(
            Problem::new("a Markov model needs `start`: the state it begins in")
                .at_path(base.to_owned())
                .with_help(list.clone()),
        ),
        Some(start) if !ids.contains(&start) => {
            let help = match closest(start.as_str(), &known) {
                Some(near) => format!("did you mean '{near}'? {list}"),
                None => list.clone(),
            };
            problems.push(
                Problem::new(format!("the start state '{start}' does not exist"))
                    .at_path(format!("{base}.start"))
                    .with_help(help),
            );
        }
        Some(_) => {}
    }
    for (index, state) in model.states.iter().enumerate() {
        let at = format!("{base}.states[{index}].to");
        if state.to.is_empty() {
            problems.push(
                Problem::new("a state of a Markov model needs `to`: the chances of the next state")
                    .at_path(format!("{base}.states[{index}]")),
            );
            continue;
        }
        let mut sum: u64 = 0;
        for (target, chance) in &state.to {
            sum += u64::from(Probability::micro_percent(*chance));
            if Some(target) == state.id.as_ref() {
                problems.push(
                    Problem::new("a state does not lead to itself: the stay is the time it lasts")
                        .at_path(format!("{at}.{target}"))
                        .with_help("take the chance away and give the other states the 100 %; a longer `stay` keeps the chain in the state"),
                );
            } else if !ids.contains(&target) {
                let help = match closest(target.as_str(), &known) {
                    Some(near) => format!("did you mean '{near}'? {list}"),
                    None => list.clone(),
                };
                problems.push(
                    Problem::new(format!("the state '{target}' does not exist"))
                        .at_path(format!("{at}.{target}"))
                        .with_help(help),
                );
            }
        }
        if sum != HUNDRED_PERCENT {
            problems.push(
                Problem::new(format!(
                    "the chances add up to {}%, not 100%",
                    Probability::from_micro_percent(u32::try_from(sum).unwrap_or(u32::MAX))
                        .map_or_else(
                            || ">100".to_owned(),
                            |p| p.to_string().trim_end_matches('%').to_owned()
                        )
                ))
                .at_path(at)
                .with_help("the chances of the next state are all the ways out of this one"),
            );
        }
    }
}

/// The rules of an event that repeats: `every` is longer than zero, `until` and `count` need it and exclude each other.
pub(crate) fn check_repeats(
    event: &Event,
    base: &str,
    duration: Duration,
    problems: &mut Vec<Problem>,
) {
    if event.every.is_none() {
        for (key, present) in [
            ("until", event.until.is_some()),
            ("count", event.count.is_some()),
        ] {
            if present {
                problems.push(
                    Problem::new(format!(
                        "`{key}` belongs to an event that repeats, and this one has no `every`"
                    ))
                    .at_path(format!("{base}.{key}"))
                    .with_help("write `every` with the time between the events"),
                );
            }
        }
        return;
    }
    if event.every.is_some_and(Duration::is_zero) {
        problems.push(
            Problem::new("`every` must be longer than zero").at_path(format!("{base}.every")),
        );
    }
    if event.until.is_some() && event.count.is_some() {
        problems.push(
            Problem::new("`until` and `count` both say when the repeating ends")
                .at_path(format!("{base}.count"))
                .with_help("write one of them"),
        );
    }
    if let Some(until) = event.until
        && (until <= event.at || (!duration.is_zero() && until > duration))
    {
        problems.push(
            Problem::new(format!(
                "`until` ({until}) must be after the first event ({}) and not after the end of the measured phase ({duration})",
                event.at
            ))
            .at_path(format!("{base}.until")),
        );
    }
    if event.count == Some(0) {
        problems.push(Problem::new("`count` must be at least 1").at_path(format!("{base}.count")));
    }
}
