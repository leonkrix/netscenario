//! Where the rate of a link changes during a run, and how fast (D-091).
//!
//! A backend that follows a rate that changes needs a shaper that can be changed (the Kernel backend builds such a direction with
//! an `htb` class, D-079), and how fast the rate changes decides how exactly it can follow. This is known from the file alone, with
//! no seed: the steps of the timeline, the states of a periodic model, the rows of a trace, and, for a Markov chain, the stays.

use crate::link_model::{ModelKind, periodic_events, repeated};
use crate::link_trace::trace_events;
use crate::scenario::{Edit, Event, Scenario};
use crate::units::Duration;

/// A place in a scenario where the rate of a link changes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RateChange {
    /// The path in the scenario of the timeline event or the model.
    pub path: String,
    /// The shortest time between two changes of the rate of one direction, or `None` if the rate changes only once. For a Markov
    /// model the shortest stay of a state that writes the rate (the mean, for an exponential stay).
    pub shortest: Option<Duration>,
}

/// Whether some event changes the rate, and the shortest time between two events that change the rate of the same direction of the
/// same link (`None` if there is no pair).
fn shortest_gap(events: &[Event]) -> (bool, Option<Duration>) {
    let mut times: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
    for event in events {
        let Some(change) = &event.set else { continue };
        let changes = |part: &Option<crate::scenario::ImpairmentsChange>| {
            part.as_ref()
                .is_some_and(|part| !matches!(part.rate, Edit::Keep))
        };
        if changes(&change.forward) || changes(&change.both) {
            times[0].push(event.at.as_nanos());
        }
        if changes(&change.reverse) || changes(&change.both) {
            times[1].push(event.at.as_nanos());
        }
    }
    if times.iter().all(Vec::is_empty) {
        return (false, None);
    }
    let mut shortest: Option<u64> = None;
    for direction in &mut times {
        direction.sort_unstable();
        direction.dedup();
        for pair in direction.windows(2) {
            let gap = pair[1] - pair[0];
            shortest = Some(shortest.map_or(gap, |s| s.min(gap)));
        }
    }
    (true, shortest.map(Duration::from_nanos))
}

impl Scenario {
    /// Every place where the rate of a link changes during the run, in the order of the file: the timeline first, then the models.
    #[must_use]
    pub fn rate_changes(&self) -> Vec<RateChange> {
        let mut found = Vec::new();
        // The events of the timeline of each link, a repeating event as its copies
        for (index, link) in self.topology.links.iter().enumerate() {
            let mut events = Vec::new();
            let mut first = None;
            for (place, event) in self.timeline.iter().enumerate() {
                if event.link != link.id {
                    continue;
                }
                first.get_or_insert(place);
                events.extend(repeated(event, self.duration).unwrap_or_default());
            }
            if let (Some(place), (true, shortest)) = (first, shortest_gap(&events)) {
                found.push(RateChange {
                    path: format!("timeline[{place}]"),
                    shortest,
                });
            }
            let Some(model) = &link.model else { continue };
            let path = format!("topology.links[{index}].model");
            match model.kind {
                ModelKind::Periodic | ModelKind::Trace => {
                    let made = if model.kind == ModelKind::Periodic {
                        periodic_events(link, model, self.duration)
                    } else {
                        trace_events(link, model, self.duration)
                    };
                    if let (true, shortest) =
                        made.map_or((false, None), |events| shortest_gap(&events))
                    {
                        found.push(RateChange { path, shortest });
                    }
                }
                ModelKind::Markov => {
                    let stays = model
                        .states
                        .iter()
                        .filter(|state| {
                            state.set.as_ref().is_some_and(|change| {
                                [&change.forward, &change.reverse, &change.both]
                                    .into_iter()
                                    .flatten()
                                    .any(|part| !matches!(part.rate, Edit::Keep))
                            })
                        })
                        .filter_map(|state| state.stay);
                    if let Some(shortest) = stays.min() {
                        found.push(RateChange {
                            path,
                            shortest: Some(shortest),
                        });
                    }
                }
            }
        }
        found
    }
}
