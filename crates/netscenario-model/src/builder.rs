//! Building a scenario in code (SCN-04). The result is the same model that a file produces, so a scenario
//! built here and one read from a file are equal if they say the same.
//!
//! ```
//! use netscenario_model::{Duration, Impairments, Probability, Rate, Scenario, Size};
//!
//! let scenario = Scenario::builder("lte-like")
//!     .duration(Duration::from_secs(30).unwrap())
//!     .seed(42)
//!     .nodes("client", "server")
//!     .link(
//!         "access",
//!         Some(Impairments::new().with_delay("40ms".parse().unwrap()).with_loss_random("1%".parse().unwrap())),
//!         None,
//!     )
//!     .udp_cbr("media", "client", "server", "2mbit".parse().unwrap(), Size::from_bytes(1200))
//!     .build()?;
//! assert_eq!(scenario.workloads.len(), 1);
//! # Ok::<(), Vec<netscenario_model::Problem>>(())
//! ```

use std::collections::BTreeMap;

use crate::error::Problem;
use crate::scenario::{
    Backend, Change, Command, Direction, Event, FormatVersion, Impairments, Jitter,
    JitterDistribution, Link, Loss, Reorder, Scenario, Topology, True, UdpCbr, UdpChecksum,
    Workload,
};
use crate::units::{Duration, Name, Probability, Rate, Size};

impl Impairments {
    /// No impairment at all.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a packet draws random numbers in this direction: loss (all models), duplication, corruption
    /// and reordering do, a delay with a jitter does too but is not counted, because the pattern of lost
    /// packets does not depend on it. These are the impairments for which the seed of a run matters.
    #[must_use]
    pub fn is_random(&self) -> bool {
        self.loss.is_some()
            || self.duplicate.is_some()
            || self.corrupt.is_some()
            || self.reorder.is_some()
    }

    /// With a fixed delay.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// With a variation of the delay (needs a delay; checked by [`Scenario::validate`]).
    #[must_use]
    pub fn with_jitter(mut self, amount: Duration, distribution: JitterDistribution) -> Self {
        self.jitter = Some(Jitter {
            amount,
            distribution,
        });
        self
    }

    /// With independent packet loss.
    #[must_use]
    pub fn with_loss_random(mut self, p: Probability) -> Self {
        self.loss = Some(Loss::Random { p });
        self
    }

    /// With bursts of loss from two states (Gilbert-Elliott): `p` and `r` are the chances to go to the bad and
    /// back to the good state, `loss_bad` and `loss_good` the chances that a packet is lost in each.
    #[must_use]
    pub fn with_loss_gilbert_elliott(
        mut self,
        p: Probability,
        r: Probability,
        loss_bad: Probability,
        loss_good: Probability,
    ) -> Self {
        self.loss = Some(Loss::GilbertElliott {
            p,
            r,
            loss_bad,
            loss_good,
        });
        self
    }

    /// With bursts of loss from four states, the five transitions in the order of `netem loss state`.
    #[must_use]
    pub fn with_loss_markov4(
        mut self,
        p13: Probability,
        p31: Probability,
        p32: Probability,
        p23: Probability,
        p14: Probability,
    ) -> Self {
        self.loss = Some(Loss::Markov4 {
            p13,
            p31,
            p32,
            p23,
            p14,
        });
        self
    }

    /// With reordering (needs a delay; checked by [`Scenario::validate`]).
    #[must_use]
    pub fn with_reorder(mut self, share: Probability, gap: Option<u32>) -> Self {
        self.reorder = Some(Reorder { share, gap });
        self
    }

    /// With duplication.
    #[must_use]
    pub fn with_duplicate(mut self, p: Probability) -> Self {
        self.duplicate = Some(p);
        self
    }

    /// With corruption.
    #[must_use]
    pub fn with_corrupt(mut self, p: Probability) -> Self {
        self.corrupt = Some(p);
        self
    }

    /// With a rate limit.
    #[must_use]
    pub fn with_rate(mut self, rate: Rate) -> Self {
        self.rate = Some(rate);
        self
    }
}

impl Event {
    /// An event with no action yet; the constructors below give it one.
    fn at(at: Duration, link: Name) -> Self {
        Self {
            at,
            link,
            direction: None,
            set: None,
            outage: None,
            down: None,
            up: None,
        }
    }

    /// At `at`, change values of `link`.
    #[must_use]
    pub fn set(at: Duration, link: Name, change: Change) -> Self {
        Self {
            set: Some(change),
            ..Self::at(at, link)
        }
    }

    /// At `at`, nothing is delivered on `link` (in `direction`, or both) for `length`, not even a packet that is
    /// on its way (D-064).
    #[must_use]
    pub fn outage(
        at: Duration,
        link: Name,
        length: Duration,
        direction: Option<Direction>,
    ) -> Self {
        Self {
            outage: Some(length),
            direction,
            ..Self::at(at, link)
        }
    }

    /// At `at`, the interface of `link` goes down.
    #[must_use]
    pub fn down(at: Duration, link: Name) -> Self {
        Self {
            down: Some(True),
            ..Self::at(at, link)
        }
    }

    /// At `at`, the interface of `link` comes up.
    #[must_use]
    pub fn up(at: Duration, link: Name) -> Self {
        Self {
            up: Some(True),
            ..Self::at(at, link)
        }
    }
}

/// Builds a [`Scenario`] step by step. Names are given as text and checked when the scenario is built, so a
/// chain of calls does not need a `?` after each one.
#[derive(Debug, Clone)]
pub struct ScenarioBuilder {
    name: Option<Name>,
    seed: Option<u64>,
    duration: Option<Duration>,
    backend: Backend,
    nodes: Vec<Name>,
    links: Vec<Link>,
    workloads: Vec<Workload>,
    timeline: Vec<Event>,
    problems: Vec<Problem>,
}

impl Scenario {
    /// Starts building a scenario with the given name.
    #[must_use]
    pub fn builder(name: &str) -> ScenarioBuilder {
        let mut builder = ScenarioBuilder {
            name: None,
            seed: None,
            duration: None,
            backend: Backend::Auto,
            nodes: Vec::new(),
            links: Vec::new(),
            workloads: Vec::new(),
            timeline: Vec::new(),
            problems: Vec::new(),
        };
        builder.name = builder.name_at(name, "name");
        builder
    }
}

impl ScenarioBuilder {
    fn name_at(&mut self, text: &str, path: &str) -> Option<Name> {
        match Name::new(text) {
            Ok(name) => Some(name),
            Err(error) => {
                self.problems
                    .push(Problem::new(error.to_string()).at_path(path));
                None
            }
        }
    }

    /// The length of the measured phase.
    #[must_use]
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The seed of the scenario.
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// The backend.
    #[must_use]
    pub fn backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    /// The two nodes of release 0.1.
    #[must_use]
    pub fn nodes(mut self, first: &str, second: &str) -> Self {
        for (index, text) in [first, second].into_iter().enumerate() {
            let path = format!("topology.nodes[{index}]");
            if let Some(name) = self.name_at(text, &path) {
                self.nodes.push(name);
            }
        }
        self
    }

    /// A link between the two nodes, with impairments for the direction from the first to the second and for
    /// the way back. For the same values in both directions use [`ScenarioBuilder::link_both`].
    #[must_use]
    pub fn link(
        mut self,
        id: &str,
        forward: Option<Impairments>,
        reverse: Option<Impairments>,
    ) -> Self {
        let index = self.links.len();
        let id = self.name_at(id, &format!("topology.links[{index}].id"));
        if let (Some(id), [first, second]) = (id, self.nodes.as_slice()) {
            let between = [first.clone(), second.clone()];
            self.links.push(Link {
                id,
                between,
                forward,
                reverse,
                both: None,
            });
        } else if self.nodes.len() != 2 {
            self.problems.push(
                Problem::new("call `nodes` with two valid names before `link`")
                    .at_path(format!("topology.links[{index}]")),
            );
        }
        self
    }

    /// A link with the same impairments in both directions.
    #[must_use]
    pub fn link_both(mut self, id: &str, both: Impairments) -> Self {
        self = self.link(id, None, None);
        if let Some(link) = self.links.last_mut() {
            link.both = Some(both);
        }
        self
    }

    /// A workload that sends UDP at a constant bit rate from one node to the other.
    #[must_use]
    pub fn udp_cbr(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        rate: Rate,
        packet_size: Size,
    ) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let from = self.name_at(from, &format!("workloads[{index}].from"));
        let to = self.name_at(to, &format!("workloads[{index}].to"));
        if let (Some(id), Some(from), Some(to)) = (id, from, to) {
            self.workloads.push(Workload::UdpCbr(UdpCbr::new(
                id,
                from,
                to,
                rate,
                packet_size,
            )));
        }
        self
    }

    /// A workload that starts a program of your own in `node`: `run` is the program and its arguments (no
    /// shell), and `{node.addr}` in them is the address of a node. Readiness, environment and shutdown can
    /// be set on the [`Command`] with [`ScenarioBuilder::workload`].
    #[must_use]
    pub fn command(mut self, id: &str, node: &str, run: &[&str]) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let node = self.name_at(node, &format!("workloads[{index}].node"));
        if let (Some(id), Some(node)) = (id, node) {
            self.workloads.push(Workload::Command(Command {
                id,
                node,
                run: run.iter().map(|argument| (*argument).to_owned()).collect(),
                env: BTreeMap::new(),
                ready: None,
                shutdown: None,
            }));
        }
        self
    }

    /// Adds a workload that was made by hand.
    #[must_use]
    pub fn workload(mut self, workload: Workload) -> Self {
        self.workloads.push(workload);
        self
    }

    /// Adds an event to the timeline (checked when the scenario is built).
    #[must_use]
    pub fn event(mut self, event: Event) -> Self {
        self.timeline.push(event);
        self
    }

    /// Sets when the last workload starts and stops, from the start of the measured phase.
    #[must_use]
    pub fn window(mut self, start: Duration, stop: Duration) -> Self {
        if let Some(Workload::UdpCbr(workload)) = self.workloads.last_mut() {
            workload.start = Some(start);
            workload.stop = Some(stop);
        }
        self
    }

    /// Sends the packets of the last `udp-cbr` workload without the UDP checksum (`udp-checksum: false`), so that
    /// a corrupted packet arrives and is counted (D-040). The packet size must then be at least
    /// [`MIN_PACKET_SIZE_WITHOUT_CHECKSUM`](crate::MIN_PACKET_SIZE_WITHOUT_CHECKSUM).
    #[must_use]
    pub fn without_udp_checksum(mut self) -> Self {
        if let Some(Workload::UdpCbr(workload)) = self.workloads.last_mut() {
            workload.udp_checksum = UdpChecksum::Off;
        }
        self
    }

    /// Builds the scenario and checks it with [`Scenario::validate`].
    ///
    /// # Errors
    ///
    /// Returns every problem found: names that are not valid, a missing name or duration, and the rules of
    /// [`Scenario::validate`].
    pub fn build(mut self) -> Result<Scenario, Vec<Problem>> {
        if self.duration.is_none() {
            self.problems
                .push(Problem::new("the duration is missing: call `duration`").at_path("duration"));
        }
        let (Some(name), Some(duration)) = (self.name, self.duration) else {
            return Err(self.problems);
        };
        if !self.problems.is_empty() {
            return Err(self.problems);
        }
        let scenario = Scenario {
            version: FormatVersion,
            name,
            seed: self.seed,
            duration,
            backend: self.backend,
            topology: Topology {
                nodes: self.nodes,
                links: self.links,
            },
            workloads: self.workloads,
            timeline: self.timeline,
        };
        let problems = scenario.validate();
        if problems.is_empty() {
            Ok(scenario)
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_scenario_equals_the_same_scenario_read_from_a_file() {
        let built = Scenario::builder("minimal")
            .duration(Duration::from_secs(5).unwrap())
            .nodes("client", "server")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "client",
                "server",
                "1mbit".parse().unwrap(),
                Size::from_bytes(1200),
            )
            .build()
            .unwrap();
        let text = include_str!("../../../scenarios/minimal.yaml");
        assert_eq!(built, Scenario::from_yaml(text, "minimal.yaml").unwrap());
    }

    #[test]
    fn every_problem_is_reported_by_the_build() {
        let problems = Scenario::builder("Bad Name")
            .nodes("a", "b")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "a",
                "z",
                "1mbit".parse().unwrap(),
                Size::from_bytes(8),
            )
            .build()
            .unwrap_err();
        assert!(problems.iter().any(|p| p.path.as_deref() == Some("name")));
        assert!(
            problems
                .iter()
                .any(|p| p.path.as_deref() == Some("duration"))
        );
    }

    #[test]
    fn the_rules_of_the_file_apply_to_a_built_scenario() {
        let problems = Scenario::builder("t")
            .duration(Duration::from_secs(10).unwrap())
            .nodes("a", "b")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "a",
                "z",
                "1mbit".parse().unwrap(),
                Size::from_bytes(8),
            )
            .build()
            .unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("the node 'z' does not exist"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("packet size 8 bytes"))
        );
    }
}
