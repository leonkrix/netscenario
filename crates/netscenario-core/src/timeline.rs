//! The timeline of a scenario as the list of things a backend has to do, in the order it has to do them.
//!
//! The file says what happens to a link at a moment: a `set` of some values, an `outage`, `down`, `up`. A backend
//! needs something simpler: at a moment, **this direction has these impairments from now on** (the whole
//! state, not the difference), or nothing is delivered in this direction from now on, or the interface goes down
//! or up. [`steps`] makes that list, and [`states_of`] the states a direction goes through, which a backend that
//! builds queues needs before the run begins (D-059).
//!
//! An outage is **not a value**: it is a window in which nothing leaves the link, not even a packet that is on its
//! way (D-064). It is two steps, its start and its end, that do not touch the impairments, so the values of a
//! direction go on as the file says them while it lasts, with every `set` that comes in between, and the packets
//! that get to the end of the link in the window are lost. Windows of one direction do not overlap.

use netscenario_model::{Direction as Side, Duration, Impairments, Scenario};

use crate::backend::Direction;
use crate::provision::impairments_of;

/// What a backend does at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// The direction of the link has these impairments from now on. This is the whole state: what an earlier
    /// step set and this one does not change is in it. An outage is not in it (see [`Action::Blackout`]).
    Impair {
        /// The direction.
        direction: Direction,
        /// The impairments.
        impairments: Box<Impairments>,
        /// Whether an outage is going on in this direction at this moment. The `impairments` do not contain it: an
        /// outage is a window at the delivery and not a value (D-064), and a backend does it with the step that
        /// begins it.
        outage: bool,
    },
    /// The interface of the link goes down (D-039).
    Down,
    /// The interface of the link comes up again.
    Up,
    /// Nothing is delivered in the direction from now on (`on`), or deliveries are back (not `on`): an outage
    /// begins or ends (D-064). A packet that is on its way when it begins is lost as well. It comes after the others
    /// because the order of the variants is part of the API.
    Blackout {
        /// The direction.
        direction: Direction,
        /// Whether the outage begins (`true`) or ends.
        on: bool,
    },
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

    // The values of each direction of each link, as the file and the steps make them, and whether an outage is on
    let mut bases: Vec<[Impairments; 2]> = scenario
        .topology
        .links
        .iter()
        .map(|link| DIRECTIONS.map(|direction| initial(link, direction)))
        .collect();
    let mut outages: Vec<[bool; 2]> = vec![[false; 2]; bases.len()];
    let mut steps = Vec::new();
    for (at, event, link, what) in raw {
        let kind = match &what {
            Raw::Set(_) => Kind::Set,
            Raw::OutageStart(_) => Kind::OutageStart,
            Raw::OutageEnd(_) => Kind::OutageEnd,
            Raw::Down => Kind::Down,
            Raw::Up => Kind::Up,
        };
        let mut step = |action: Action| {
            steps.push(Step {
                at,
                event,
                link,
                kind,
                action,
            });
        };
        let bases = &mut bases[link];
        let outages = &mut outages[link];
        match what {
            Raw::Set(change) => {
                for (side, edit) in [
                    (Side::Forward, &change.forward),
                    (Side::Reverse, &change.reverse),
                    (Side::Both, &change.both),
                ] {
                    let Some(edit) = edit else { continue };
                    for &direction in directions(side) {
                        let base = &mut bases[way(direction)];
                        *base = edit.applied_to(base);
                    }
                    for &direction in directions(side) {
                        step(Action::Impair {
                            direction,
                            impairments: Box::new(bases[way(direction)].clone()),
                            outage: outages[way(direction)],
                        });
                    }
                }
            }
            Raw::OutageStart(side) | Raw::OutageEnd(side) => {
                let on = matches!(kind, Kind::OutageStart);
                for &direction in directions(side) {
                    outages[way(direction)] = on;
                    step(Action::Blackout { direction, on });
                }
            }
            Raw::Down => step(Action::Down),
            Raw::Up => step(Action::Up),
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
/// reordering in the impairments of a link at the start or in any state that a `set` leads to. An outage draws
/// nothing: it is a window in which the packets that reach the end of the link are dropped, whatever the numbers
/// are.
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
                Action::Impair { impairments, .. } if impairments.is_random()
            )
        })
}

/// The links that go down at some point of the timeline, by their place in the topology.
#[must_use]
pub fn links_that_go_down(scenario: &Scenario) -> Vec<usize> {
    let mut links: Vec<usize> = steps(scenario)
        .iter()
        .filter(|step| matches!(step.action, Action::Down))
        .map(|step| step.link)
        .collect();
    links.sort_unstable();
    links.dedup();
    links
}

/// The directions that have an outage at some point of the timeline, as the place of the link in the topology and
/// the direction, in the order of the links and forward before reverse. A backend that builds queues needs it before
/// the run begins, because the queue that holds the packets back is part of the tree of the direction (D-064).
#[must_use]
pub fn outage_directions(scenario: &Scenario) -> Vec<(usize, Direction)> {
    let mut found: Vec<(usize, Direction)> = steps(scenario)
        .iter()
        .filter_map(|step| match step.action {
            Action::Blackout { direction, .. } => Some((step.link, direction)),
            _ => None,
        })
        .collect();
    found.sort_by_key(|(link, direction)| (*link, way(*direction)));
    found.dedup();
    found
}

/// Whether a step changes **what a queue draws random numbers for**: the loss (or its parameters), the
/// duplication, the corruption or the reordering of a direction, or a link that goes down or comes up while a
/// queue of it draws numbers. Such a step applied by the clock lands before a different packet in every run, and
/// from there on the lost packets of one seed differ (D-038). A step that changes the rate or the delay draws
/// nothing different, and the packets stay the same. An outage begins and ends in the time of the receiver and
/// acts after the queues that draw, so it is not one of these steps (D-064).
#[must_use]
pub fn changes_what_is_drawn(scenario: &Scenario) -> bool {
    !steps_that_change_what_is_drawn(scenario).is_empty()
}

/// The places in [`steps`] of the steps that change what a queue draws random numbers for (see
/// [`changes_what_is_drawn`]).
#[must_use]
pub fn steps_that_change_what_is_drawn(scenario: &Scenario) -> Vec<usize> {
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
    steps(scenario)
        .iter()
        .enumerate()
        .filter(|(_, step)| {
            let (direction, impairments) = match &step.action {
                Action::Impair {
                    direction,
                    impairments,
                    ..
                } => (direction, impairments),
                // The window of an outage acts after the queues that draw
                Action::Blackout { .. } => return false,
                _ => {
                    // A link that goes down or comes up: the packets that a down interface refuses never reach
                    // the queue and draw no number, so from there on every packet meets a different number if
                    // the queue draws any
                    return last[step.link]
                        .iter()
                        .any(|(loss, duplicate, corrupt, reorder)| {
                            loss.is_some()
                                || duplicate.is_some()
                                || corrupt.is_some()
                                || reorder.is_some()
                        });
                }
            };
            let now = drawn(impairments);
            let changed = last[step.link][way(*direction)] != now;
            last[step.link][way(*direction)] = now;
            changed
        })
        .map(|(place, _)| place)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::Loss;

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
            Action::Blackout { direction, on } => {
                format!("{direction:?} blackout {}", if *on { "on" } else { "off" })
            }
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

    /// An outage is two steps that touch no value: the values go on as the file and the steps say them, also while
    /// the window lasts, and a `set` inside it is a step like any other (D-064).
    #[test]
    fn an_outage_is_a_window_that_touches_no_value() {
        let s = scenario(
            "      forward: { delay: 20ms, loss: { model: random, p: 1% } }\n",
            "  - { at: 5s, link: l, outage: 4s, direction: forward }\n  - { at: 7s, link: l, set: { forward: { delay: 50ms, loss: null } } }\n",
        );
        let steps: Vec<String> = steps(&s).iter().map(describe).collect();
        assert_eq!(
            steps,
            [
                "5s #0 Forward blackout on",
                "7s #1 Forward delay=Some(\"50ms\") rate=None loss=none",
                "9s #0 Forward blackout off",
            ]
        );
        // Canary: no step carries a loss of 100 % (what an outage was before D-064)
        assert!(!steps.iter().any(|step| step.contains("100000000")));
        let kinds: Vec<Kind> = super::steps(&s).iter().map(|step| step.kind).collect();
        assert_eq!(kinds, [Kind::OutageStart, Kind::Set, Kind::OutageEnd]);
    }

    /// A `set` says whether an outage is going on at its moment; the flag does not change the impairments.
    #[test]
    fn a_set_says_whether_an_outage_is_going_on() {
        let s = scenario(
            "      forward: { delay: 20ms }\n",
            "  - { at: 2s, link: l, set: { forward: { delay: 30ms } } }\n  - { at: 4s, link: l, outage: 4s, direction: forward }\n  - { at: 6s, link: l, set: { forward: { delay: 40ms } } }\n  - { at: 10s, link: l, set: { forward: { delay: 50ms } } }\n",
        );
        let flags: Vec<bool> = steps(&s)
            .iter()
            .filter_map(|step| match step.action {
                Action::Impair { outage, .. } => Some(outage),
                _ => None,
            })
            .collect();
        assert_eq!(flags, [false, true, false]);
    }

    /// `both` is an outage in each direction, and the directions that have one are listed once, in order.
    #[test]
    fn the_directions_with_an_outage_are_listed_once_in_order() {
        let s = scenario(
            "",
            "  - { at: 2s, link: l, outage: 1s }\n  - { at: 6s, link: l, outage: 1s, direction: reverse }\n",
        );
        let found = outage_directions(&s);
        assert_eq!(found, [(0, Direction::Forward), (0, Direction::Reverse)]);
        let only = scenario(
            "",
            "  - { at: 2s, link: l, outage: 1s, direction: reverse }\n",
        );
        assert_eq!(outage_directions(&only), [(0, Direction::Reverse)]);
        assert_eq!(outage_directions(&scenario("", "  []\n")), []);
        // The states of the direction have no loss of an outage in them
        assert_eq!(states_of(&s, 0, Direction::Forward).len(), 1);
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

    /// A step that changes what is drawn (a loss) is told from one that does not (a rate, a delay, an outage).
    #[test]
    fn only_a_step_that_changes_what_is_drawn_is_one() {
        let lossy = "      forward: { delay: 20ms, loss: { model: random, p: 5% } }\n";
        let timeline = |text: &str| scenario(lossy, text);
        // Only the rate and the delay change: the same numbers are drawn
        assert!(!changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { rate: 2mbit, delay: 40ms } } }\n"
        )));
        // The loss changes, or goes away
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { loss: { model: random, p: 10% } } } }\n"
        )));
        assert!(changes_what_is_drawn(&timeline(
            "  - { at: 2s, link: l, set: { forward: { loss: null } } }\n"
        )));
        // An outage acts after the queues that draw: the same numbers are drawn with it
        assert!(!changes_what_is_drawn(&timeline(
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
