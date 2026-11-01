//! The timeline of a scenario as the list of things a backend has to do, in the order it has to do them.
//!
//! The file says what happens to a link at a moment: a `set` of some values, an `outage`, `down`, `up`. A backend
//! needs something simpler: at a moment, **this direction has these impairments from now on** (the whole
//! state, not the difference), or the interface goes down or up. [`steps`] makes that list, and [`states_of`]
//! the states a direction goes through, which a backend that builds queues needs before the run begins
//! (D-059).
//!
//! An outage is a loss of 100 % (D-039) that lasts for its length, and then the direction has the values it had
//! before, with every `set` that came in between (D-050 refuses a `set` of the loss inside an outage, so the
//! two never disagree). Windows of one direction do not overlap, so a direction is inside an outage or not.

use netscenario_model::{Direction as Side, Duration, Impairments, Loss, Probability, Scenario};

use crate::backend::Direction;
use crate::provision::impairments_of;

/// What a backend does at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// The direction of the link has these impairments from now on. This is the whole state: what an earlier
    /// step set and this one does not change is in it, and an outage that is going on is in it as a loss of 100 %.
    Impair {
        /// The direction.
        direction: Direction,
        /// The impairments.
        impairments: Box<Impairments>,
        /// Whether an outage is going on in this direction, so that `impairments` has its loss of 100 %.
        outage: bool,
    },
    /// The interface of the link goes down (D-039).
    Down,
    /// The interface of the link comes up again.
    Up,
}

/// What the event of the file was that a step comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// A `set`.
    Set,
    /// The start of an `outage`.
    OutageStart,
    /// The end of an `outage`.
    OutageEnd,
    /// A `down`.
    Down,
    /// An `up`.
    Up,
}

impl Kind {
    /// The word for the result: `set`, `outage-start`, `outage-end`, `down`, `up`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Kind::Set => "set",
            Kind::OutageStart => "outage-start",
            Kind::OutageEnd => "outage-end",
            Kind::Down => "down",
            Kind::Up => "up",
        }
    }
}

/// One thing to do, and when and for which event of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Step {
    /// The moment, from the start of the measured phase.
    pub at: Duration,
    /// The place of the event in `timeline`. Two steps of one event (a `set` of `both`, the two ends of an
    /// outage) have the same one.
    pub event: usize,
    /// The place of the link in `topology.links`.
    pub link: usize,
    /// What the event of the file was.
    pub kind: Kind,
    /// What to do.
    pub action: Action,
}

/// The two directions of a link.
const DIRECTIONS: [Direction; 2] = [Direction::Forward, Direction::Reverse];

/// The impairments a link has at the start, for one direction: none if it has none.
fn initial(link: &netscenario_model::Link, direction: Direction) -> Impairments {
    impairments_of(link, direction).cloned().unwrap_or_default()
}

/// The loss of an outage: every packet.
fn outage_loss() -> Loss {
    Loss::Random {
        p: Probability::ALWAYS,
    }
}

/// What one direction is made of at a moment: the values from the file and the steps, and whether an outage is on.
#[derive(Clone)]
struct DirectionState {
    base: Impairments,
    outage: bool,
}

impl DirectionState {
    /// What is applied: the base, with the loss of the outage over its loss if one is going on.
    fn effective(&self) -> Impairments {
        let mut impairments = self.base.clone();
        if self.outage {
            impairments.loss = Some(outage_loss());
        }
        impairments
    }
}

/// Something that happens at a moment, before the order is fixed.
enum Raw {
    Set(Box<netscenario_model::Change>),
    OutageStart(Side),
    OutageEnd(Side),
    Down,
    Up,
}

/// The directions a side of a link means.
fn directions(side: Side) -> &'static [Direction] {
    match side {
        Side::Forward => &DIRECTIONS[..1],
        Side::Reverse => &DIRECTIONS[1..],
        Side::Both => &DIRECTIONS,
    }
}

/// What happens when, before the order of one moment is fixed: (moment, event, link, what). The sort is stable, so
/// the start of an outage keeps its place in the file.
fn raw_steps(scenario: &Scenario) -> Vec<(Duration, usize, usize, Raw)> {
    let mut raw: Vec<(Duration, usize, usize, Raw)> = Vec::new();
    for (index, event) in scenario.timeline.iter().enumerate() {
        let Some(link) = scenario
            .topology
            .links
            .iter()
            .position(|link| link.id == event.link)
        else {
            continue;
        };
        if let Some(change) = &event.set {
            raw.push((event.at, index, link, Raw::Set(Box::new(change.clone()))));
        }
        if let Some(length) = event.outage {
            let side = event.direction.unwrap_or(Side::Both);
            let end = Duration::from_nanos(event.at.as_nanos().saturating_add(length.as_nanos()));
            raw.push((event.at, index, link, Raw::OutageStart(side)));
            raw.push((end, index, link, Raw::OutageEnd(side)));
        }
        if event.down.is_some() {
            raw.push((event.at, index, link, Raw::Down));
        }
        if event.up.is_some() {
            raw.push((event.at, index, link, Raw::Up));
        }
    }
    raw.sort_by_key(|(at, index, _, _)| (*at, *index));

    raw
}

/// The steps of the timeline of `scenario`, in the order they are done: by moment, and for one moment by the order
/// of the events in the file. A scenario without a timeline has none.
///
/// The scenario must be valid (`Scenario::validate`): an event of a link that does not exist is skipped.
#[must_use]
pub fn steps(scenario: &Scenario) -> Vec<Step> {
    let raw = raw_steps(scenario);

    let mut states: Vec<[DirectionState; 2]> = scenario
        .topology
        .links
        .iter()
        .map(|link| {
            DIRECTIONS.map(|direction| DirectionState {
                base: initial(link, direction),
                outage: false,
            })
        })
        .collect();
    let mut steps = Vec::new();
    for (at, event, link, what) in raw {
        let kind = match &what {
            Raw::Set(_) => Kind::Set,
            Raw::OutageStart(_) => Kind::OutageStart,
            Raw::OutageEnd(_) => Kind::OutageEnd,
            Raw::Down => Kind::Down,
            Raw::Up => Kind::Up,
        };
        let mut impair = |state: &mut [DirectionState; 2], side: Side| {
            for &direction in directions(side) {
                steps.push(Step {
                    at,
                    event,
                    link,
                    kind,
                    action: Action::Impair {
                        direction,
                        impairments: Box::new(state[way(direction)].effective()),
                        outage: state[way(direction)].outage,
                    },
                });
            }
        };
        let state = &mut states[link];
        match what {
            Raw::Set(change) => {
                for (side, edit) in [
                    (Side::Forward, &change.forward),
                    (Side::Reverse, &change.reverse),
                    (Side::Both, &change.both),
                ] {
                    let Some(edit) = edit else { continue };
                    for &direction in directions(side) {
                        let way = way(direction);
                        state[way].base = edit.applied_to(&state[way].base);
                    }
                    impair(state, side);
                }
            }
            Raw::OutageStart(side) => {
                for &direction in directions(side) {
                    state[way(direction)].outage = true;
                }
                impair(state, side);
            }
            Raw::OutageEnd(side) => {
                for &direction in directions(side) {
                    state[way(direction)].outage = false;
                }
                impair(state, side);
            }
            Raw::Down => steps.push(Step {
                at,
                event,
                link,
                kind,
                action: Action::Down,
            }),
            Raw::Up => steps.push(Step {
                at,
                event,
                link,
                kind,
                action: Action::Up,
            }),
        }
    }
    steps
}

const fn way(direction: Direction) -> usize {
    match direction {
        Direction::Forward => 0,
        Direction::Reverse => 1,
    }
}

/// Every state that one direction of a link goes through, the one it starts with first: what a backend needs to know
/// to build queues that every state fits (D-059). A state that repeats is listed once.
#[must_use]
pub fn states_of(scenario: &Scenario, link: usize, direction: Direction) -> Vec<Impairments> {
    let Some(first) = scenario.topology.links.get(link) else {
        return Vec::new();
    };
    let mut states = vec![initial(first, direction)];
    for step in steps(scenario) {
        if step.link != link {
            continue;
        }
        if let Action::Impair {
            direction: moved,
            impairments,
            ..
        } = step.action
            && moved == direction
            && !states.contains(&impairments)
        {
            states.push(*impairments);
        }
    }
    states
}

/// Whether the run draws random numbers that the seed is to repeat: loss of any model, duplication, corruption or
/// reordering in the impairments of a link at the start or in any state that a `set` leads to. An outage is a loss
/// of 100 %: every packet is dropped whatever the numbers are, so it does not count.
#[must_use]
pub fn draws_random_numbers(scenario: &Scenario) -> bool {
    let at_the_start = scenario.topology.links.iter().any(|link| {
        DIRECTIONS
            .into_iter()
            .any(|direction| initial(link, direction).is_random())
    });
    at_the_start
        || steps(scenario).iter().any(|step| {
            matches!(
                &step.action,
                Action::Impair { impairments, outage: false, .. } if impairments.is_random()
            )
        })
}

/// Whether a step changes **what a queue draws random numbers for**: the loss (or its parameters, or an outage that
/// begins or ends), the duplication, the corruption or the reordering of a direction. Such a step applied by the
/// clock lands before a different packet in every run, and from there on the lost packets of one seed differ
/// (D-038). A step that changes the rate or the delay draws nothing different, and the packets stay the same.
#[must_use]
pub fn changes_what_is_drawn(scenario: &Scenario) -> bool {
    let drawn = |impairments: &Impairments| {
        (
            impairments.loss,
            impairments.duplicate,
            impairments.corrupt,
            impairments.reorder,
        )
    };
    let mut last: Vec<[_; 2]> = scenario
        .topology
        .links
        .iter()
        .map(|link| DIRECTIONS.map(|direction| drawn(&initial(link, direction))))
        .collect();
    steps(scenario).iter().any(|step| {
        let Action::Impair {
            direction,
            impairments,
            ..
        } = &step.action
        else {
            return false;
        };
        let now = drawn(impairments);
        let changed = last[step.link][way(*direction)] != now;
        last[step.link][way(*direction)] = now;
        changed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(link: &str, timeline: &str) -> Scenario {
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: t\nduration: 20s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\ntimeline:\n{timeline}"
            ),
            "t.yaml",
        )
        .unwrap()
    }

    fn describe(step: &Step) -> String {
        let what = match &step.action {
            Action::Impair {
                direction,
                impairments,
                ..
            } => format!(
                "{direction:?} delay={:?} rate={:?} loss={}",
                impairments.delay.map(|d| d.to_string()),
                impairments.rate.map(|r| r.to_string()),
                impairments
                    .loss
                    .as_ref()
                    .map_or("none".to_owned(), |loss| match loss {
                        Loss::Random { p } => format!("random {}", p.micro_percent()),
                        _ => "model".to_owned(),
                    })
            ),
            Action::Down => "down".to_owned(),
            Action::Up => "up".to_owned(),
        };
        format!("{} #{} {what}", step.at, step.event)
    }

    /// A `set` becomes the whole state, and `both` becomes two steps of one event; what a step does not
    /// mention stays, and `null` takes a value away.
    #[test]
    fn a_set_becomes_the_whole_state_of_each_direction() {
        let s = scenario(
            "      both: { delay: 20ms, rate: 4mbit }\n",
            "  - { at: 5s, link: l, set: { forward: { rate: 2mbit } } }\n  - { at: 8s, link: l, set: { both: { delay: 40ms, rate: null } } }\n",
        );
        let steps: Vec<String> = steps(&s).iter().map(describe).collect();
        assert_eq!(
            steps,
            [
                "5s #0 Forward delay=Some(\"20ms\") rate=Some(\"2mbit\") loss=none",
                "8s #1 Forward delay=Some(\"40ms\") rate=None loss=none",
                "8s #1 Reverse delay=Some(\"40ms\") rate=None loss=none",
            ]
        );
    }

    /// An outage is a loss of 100 % for its length, and then the values of the time: here the delay was
    /// changed in between, and the end of the outage brings back the new one with the loss it had (none).
    #[test]
    fn an_outage_is_a_loss_of_all_packets_and_then_the_values_of_that_moment() {
        let s = scenario(
            "      forward: { delay: 20ms, loss: { model: random, p: 1% } }\n",
            "  - { at: 5s, link: l, outage: 4s, direction: forward }\n  - { at: 7s, link: l, set: { forward: { delay: 50ms } } }\n",
        );
        let steps: Vec<String> = steps(&s).iter().map(describe).collect();
        assert_eq!(
            steps,
            [
                "5s #0 Forward delay=Some(\"20ms\") rate=None loss=random 100000000",
                "7s #1 Forward delay=Some(\"50ms\") rate=None loss=random 100000000",
                "9s #0 Forward delay=Some(\"50ms\") rate=None loss=random 1000000",
            ]
        );
        // Canary: the end of the outage is not a restore of the state at its start (the delay would be 20 ms)
        assert!(!steps[2].contains("20ms"));
    }

    #[test]
    fn down_and_up_are_steps_of_the_link_in_the_order_of_the_file_for_one_moment() {
        let s = scenario(
            "",
            "  - { at: 3s, link: l, down: true }\n  - { at: 6s, link: l, up: true }\n  - { at: 6s, link: l, set: { forward: { delay: 5ms } } }\n",
        );
        let steps: Vec<String> = steps(&s).iter().map(describe).collect();
        assert_eq!(
            steps,
            [
                "3s #0 down",
                "6s #1 up",
                "6s #2 Forward delay=Some(\"5ms\") rate=None loss=none"
            ]
        );
        // No timeline, no step
        assert_eq!(super::steps(&scenario("", "  []\n")).len(), 0);
    }

    /// A step that changes what is drawn (a loss, an outage) is told from one that does not (a rate, a delay).
    #[test]
    fn only_a_step_that_changes_what_is_drawn_is_one() {
        let lossy = "      forward: { delay: 20ms, loss: { model: random, p: 5% } }\n";
        let timeline = |text: &str| scenario(lossy, text);
        // Only the rate and the delay change: the same numbers are drawn
        assert!(!changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { rate: 2mbit, delay: 40ms } } }\n"
        )));
        // The loss changes, or goes away, or an outage begins
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { loss: { model: random, p: 10% } } } }\n"
        )));
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { loss: null } } }\n"
        )));
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, outage: 1s }\n"
        )));
        // The other direction has nothing drawn, and a step that adds a duplication does change it
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { reverse: { duplicate: 5% } } }\n"
        )));
        // Canary: no timeline, nothing changes
        assert!(!changes_what_is_drawn(&timeline("  []\n")));
    }

    /// The states of a direction: where it starts, and every state that a step leads to, once.
    #[test]
    fn the_states_of_a_direction_are_its_start_and_every_state_a_step_leads_to() {
        let s = scenario(
            "      forward: { delay: 20ms }\n",
            "  - { at: 2s, link: l, set: { forward: { rate: 2mbit } } }\n  - { at: 4s, link: l, set: { forward: { rate: null } } }\n  - { at: 6s, link: l, set: { reverse: { delay: 1ms } } }\n",
        );
        let forward = states_of(&s, 0, Direction::Forward);
        // delay; delay and rate; delay again (the start, listed once)
        assert_eq!(forward.len(), 2, "{forward:?}");
        assert!(forward[0].rate.is_none() && forward[1].rate.is_some());
        let reverse = states_of(&s, 0, Direction::Reverse);
        assert_eq!(reverse.len(), 2);
        assert!(reverse[0].delay.is_none() && reverse[1].delay.is_some());
        // A link without events has its start only
        let quiet = scenario("      both: { delay: 5ms }\n", "  []\n");
        assert_eq!(states_of(&quiet, 0, Direction::Forward).len(), 1);
    }
}
