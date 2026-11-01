//! Steps 2 to 4 of the life of a run (`docs/ARCHITECTURE.md`): check the capabilities, create the nodes and
//! links, and apply the impairments of time zero.

use std::net::Ipv4Addr;

use netscenario_model::{Impairments, Link, Name, Problem, Scenario};

use crate::backend::{Backend, Direction, LinkHandle, NodeHandle};
use crate::capability::{Accepted, Support, check};
use crate::error::Error;
use crate::feature::Feature;
use crate::seed::{derive_seed, draw_seed};

/// The address of a node on a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAddress {
    /// The node.
    pub node: Name,
    /// The link it is on.
    pub link: Name,
    /// The address of its end of the link.
    pub address: Ipv4Addr,
}

/// What was created for a scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    /// The nodes in the order of the scenario.
    pub nodes: Vec<(Name, NodeHandle)>,
    /// The links in the order of the scenario.
    pub links: Vec<(Name, LinkHandle)>,
    /// The address of each end of each link. The result records them (TOP-08).
    pub addresses: Vec<NodeAddress>,
    /// The restrictions that the scenario accepted. The result has to record them.
    pub accepted: Vec<Accepted>,
    /// The seed of the run: the one in the scenario, or one that was drawn, or none (see `loss_reproducible`).
    /// The result records it, so that the run can be repeated.
    pub seed: Option<u64>,
    /// Whether `seed` was drawn because the scenario has none.
    pub seed_drawn: bool,
    /// Whether the same seed gives the same lost packets. It is `false` for a scenario with loss that runs on a
    /// backend that cannot apply a seed: the scenario asked for no reproducibility, so it runs, and the result
    /// has to say so (D-023). A scenario without loss is always `true`.
    pub loss_reproducible: bool,
}

/// Builds the network of `scenario` with `backend`.
///
/// The capabilities are checked before anything is created, so a refused scenario leaves nothing behind. If a
/// step fails after that, the backend is torn down before the error is returned.
///
/// A scenario without a `seed` gets a drawn one if the backend can apply a seed, so that every run can be
/// repeated; otherwise it runs without (`Provisioned::loss_reproducible` is then `false` if it has loss).
///
/// # Errors
///
/// - [`Error::InvalidScenario`] if the scenario breaks a rule of the format,
/// - [`Error::Capability`] if the backend cannot reproduce it exactly,
/// - [`Error::Backend`] if the backend fails to build it.
pub fn provision(backend: &mut dyn Backend, scenario: &Scenario) -> Result<Provisioned, Error> {
    let problems = scenario.validate();
    if !problems.is_empty() {
        return Err(Error::InvalidScenario(problems));
    }
    let accepted = check(scenario, &backend.capabilities(), false)?;
    backend.check_values(scenario)?;
    let can_seed = backend.capabilities().support(Feature::SeededLoss) == Support::Exact;
    let (seed, seed_drawn) = match scenario.seed {
        Some(seed) => (Some(seed), false),
        None if can_seed => (Some(draw_seed()), true),
        None => (None, false),
    };
    // A scenario whose result depends on random numbers (loss of any model, duplication, corruption,
    // reordering): the same seed gives the same lost packets only if the backend can apply one
    let has_random = scenario.topology.links.iter().any(|link| {
        [&link.forward, &link.reverse, &link.both]
            .into_iter()
            .flatten()
            .any(Impairments::is_random)
    });
    match build(backend, scenario, seed) {
        Ok((nodes, links, addresses)) => Ok(Provisioned {
            nodes,
            links,
            addresses,
            accepted,
            seed,
            seed_drawn,
            loss_reproducible: !has_random || seed.is_some(),
        }),
        Err(error) => {
            // The first error is the one to report; a failure of the teardown is the backend's to log
            let _ = backend.teardown();
            Err(error)
        }
    }
}

type Built = (
    Vec<(Name, NodeHandle)>,
    Vec<(Name, LinkHandle)>,
    Vec<NodeAddress>,
);

fn build(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    seed: Option<u64>,
) -> Result<Built, Error> {
    let mut nodes = Vec::new();
    for name in &scenario.topology.nodes {
        nodes.push((name.clone(), backend.create_node(name)?));
    }
    let node = |name: &Name| {
        nodes
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, handle)| *handle)
    };
    let mut links = Vec::new();
    let mut addresses = Vec::new();
    for (index, link) in scenario.topology.links.iter().enumerate() {
        let (Some(first), Some(second)) = (node(&link.between[0]), node(&link.between[1])) else {
            // `validate` has checked this, so it only happens if that check and this code disagree
            return Err(Error::InvalidScenario(vec![
                Problem::new(format!(
                    "the link '{}' refers to a node that does not exist",
                    link.id
                ))
                .at_path("topology.links"),
            ]));
        };
        let handle = backend.create_link(&link.id, [first, second])?;
        for direction in [Direction::Forward, Direction::Reverse] {
            if let Some(impairments) = impairments_of(link, direction) {
                // Only what draws random numbers uses the seed
                let seed = seed
                    .filter(|_| impairments.is_random())
                    .map(|seed| derive_seed(seed, index, direction));
                backend.apply(handle, direction, impairments, seed)?;
            }
        }
        for (end, node) in link.between.iter().enumerate() {
            addresses.push(NodeAddress {
                node: node.clone(),
                link: link.id.clone(),
                address: backend.address(handle, end)?,
            });
        }
        links.push((link.id.clone(), handle));
    }
    Ok((nodes, links, addresses))
}

/// The impairments that apply to one direction of a link: `both` if it is written, else the direction's own.
#[must_use]
pub fn impairments_of(link: &Link, direction: Direction) -> Option<&Impairments> {
    link.both.as_ref().or(match direction {
        Direction::Forward => link.forward.as_ref(),
        Direction::Reverse => link.reverse.as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Program;
    use crate::capability::{Capabilities, Support};
    use crate::feature::Feature;

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        fail_on: Option<&'static str>,
        support_everything: bool,
        /// Everything but the seed of the loss is supported.
        cannot_seed: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                support_everything: true,
                ..Self::default()
            }
        }

        fn step(&mut self, call: String, kind: &'static str) -> Result<(), Error> {
            self.calls.push(call);
            if self.fail_on == Some(kind) {
                return Err(Error::Backend {
                    backend: "fake".to_owned(),
                    action: kind.to_owned(),
                    message: "refused".to_owned(),
                });
            }
            Ok(())
        }
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> Capabilities {
            let support = if self.support_everything {
                Support::Exact
            } else {
                Support::Unsupported {
                    reason: "no".to_owned(),
                    fix: "none".to_owned(),
                }
            };
            let caps = Feature::ALL
                .iter()
                .fold(Capabilities::new("fake"), |caps, f| {
                    caps.with(*f, support.clone())
                });
            if self.cannot_seed {
                caps.with(
                    Feature::SeededLoss,
                    Support::Unsupported {
                        reason: "no".to_owned(),
                        fix: "none".to_owned(),
                    },
                )
            } else {
                caps
            }
        }

        fn create_node(&mut self, name: &Name) -> Result<NodeHandle, Error> {
            self.step(format!("node {name}"), "create_node")?;
            Ok(NodeHandle(u32::try_from(self.calls.len()).unwrap()))
        }

        fn create_link(
            &mut self,
            name: &Name,
            between: [NodeHandle; 2],
        ) -> Result<LinkHandle, Error> {
            self.step(
                format!("link {name} {}-{}", between[0].0, between[1].0),
                "create_link",
            )?;
            Ok(LinkHandle(u32::try_from(self.calls.len()).unwrap()))
        }

        fn apply(
            &mut self,
            link: LinkHandle,
            direction: Direction,
            impairments: &Impairments,
            seed: Option<u64>,
        ) -> Result<(), Error> {
            let delay = impairments.delay.map(|d| d.to_string()).unwrap_or_default();
            let seed = seed.map(|s| format!(" seed {s}")).unwrap_or_default();
            self.step(
                format!("apply {} {direction:?} {delay}{seed}", link.0),
                "apply",
            )
        }

        fn start_program(&mut self, _: NodeHandle, _: &Program) -> Result<ProgramHandle, Error> {
            unreachable!("not part of provisioning")
        }

        fn address(&self, link: LinkHandle, end: usize) -> Result<std::net::Ipv4Addr, Error> {
            Ok(std::net::Ipv4Addr::new(
                10,
                100,
                u8::try_from(link.0).unwrap(),
                1 + u8::try_from(end).unwrap(),
            ))
        }

        fn describe(&self) -> crate::backend::BackendInfo {
            crate::backend::BackendInfo::default()
        }

        fn poll_program(
            &mut self,
            _: ProgramHandle,
            _: std::time::Duration,
        ) -> Result<crate::backend::ProgramState, Error> {
            unreachable!("not part of provisioning")
        }

        fn send_line(&mut self, _: ProgramHandle, _: &str) -> Result<(), Error> {
            unreachable!("not part of provisioning")
        }

        fn teardown(&mut self) -> Result<(), Error> {
            self.calls.push("teardown".to_owned());
            Ok(())
        }
    }

    use crate::backend::ProgramHandle;

    fn scenario(link_body: &str) -> Scenario {
        scenario_with("", link_body)
    }

    fn scenario_with(head: &str, link_body: &str) -> Scenario {
        Scenario::from_yaml(
            &format!("version: 0\nname: t\nduration: 10s\n{head}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link_body}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"),
            "t.yaml",
        )
        .expect("valid")
    }

    #[test]
    fn every_end_of_a_link_gets_its_address() {
        let mut backend = Fake::new();
        let done = provision(&mut backend, &scenario("")).unwrap();
        let shown: Vec<String> = done
            .addresses
            .iter()
            .map(|a| format!("{} on {}: {}", a.node, a.link, a.address))
            .collect();
        // The fake counts every object, so the number of the link is its handle; what matters is that the
        // first node of `between` gets the first address and the second node the second one
        let link = done.links[0].1.0;
        assert_eq!(
            shown,
            [
                format!("a on l: 10.100.{link}.1"),
                format!("b on l: 10.100.{link}.2")
            ]
        );
    }

    #[test]
    fn nodes_then_links_then_impairments() {
        let mut backend = Fake::new();
        let done = provision(&mut backend, &scenario("      forward: { delay: 20ms }\n")).unwrap();
        assert_eq!(done.nodes.len(), 2);
        assert_eq!(done.links.len(), 1);
        assert_eq!(
            backend.calls,
            ["node a", "node b", "link l 1-2", "apply 3 Forward 20ms"]
        );
    }

    #[test]
    fn both_applies_the_same_values_in_each_direction() {
        let mut backend = Fake::new();
        provision(&mut backend, &scenario("      both: { delay: 5ms }\n")).unwrap();
        assert_eq!(
            &backend.calls[3..],
            ["apply 3 Forward 5ms", "apply 3 Reverse 5ms"]
        );
    }

    #[test]
    fn a_link_without_impairments_applies_nothing() {
        let mut backend = Fake::new();
        provision(&mut backend, &scenario("")).unwrap();
        assert_eq!(backend.calls.len(), 3);
    }

    /// Canary: a refused scenario must leave nothing behind, so nothing may be created before the check.
    #[test]
    fn a_refused_scenario_creates_nothing() {
        let mut backend = Fake::new();
        backend.support_everything = false;
        let error = provision(&mut backend, &scenario("      both: { delay: 5ms }\n")).unwrap_err();
        assert!(matches!(error, Error::Capability(_)));
        assert!(backend.calls.is_empty(), "{:?}", backend.calls);
    }

    #[test]
    fn a_failure_tears_down_and_reports_the_first_error() {
        let mut backend = Fake::new();
        backend.fail_on = Some("create_link");
        let error = provision(&mut backend, &scenario("      both: { delay: 5ms }\n")).unwrap_err();
        assert!(matches!(&error, Error::Backend { action, .. } if action == "create_link"));
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
    }

    #[test]
    fn the_impairments_of_a_direction_prefer_both() {
        let s = scenario("      forward: { delay: 1ms }\n");
        let link = &s.topology.links[0];
        assert!(impairments_of(link, Direction::Forward).is_some());
        assert!(impairments_of(link, Direction::Reverse).is_none());
    }

    const LOSS: &str = "      both: { loss: { model: random, p: 10% } }\n";

    #[test]
    fn a_seed_in_the_scenario_is_used_and_not_called_drawn() {
        let mut backend = Fake::new();
        let done = provision(&mut backend, &scenario_with("seed: 77\n", LOSS)).unwrap();
        assert_eq!(done.seed, Some(77));
        assert!(!done.seed_drawn && done.loss_reproducible);
        let seeds: Vec<&String> = backend
            .calls
            .iter()
            .filter(|c| c.contains("seed"))
            .collect();
        assert_eq!(
            seeds.len(),
            2,
            "one seed for each direction: {:?}",
            backend.calls
        );
        assert_ne!(seeds[0], seeds[1]);
    }

    #[test]
    fn a_scenario_without_a_seed_gets_a_drawn_one_that_is_recorded() {
        let mut backend = Fake::new();
        let done = provision(&mut backend, &scenario(LOSS)).unwrap();
        assert!(done.seed.is_some() && done.seed_drawn && done.loss_reproducible);
        assert!(backend.calls.iter().any(|c| c.contains("seed")));
    }

    /// The scenario asked for no reproducibility, so it runs on a backend that cannot seed, and says so.
    #[test]
    fn without_a_seed_on_a_backend_that_cannot_seed_the_loss_is_not_reproducible() {
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let done = provision(&mut backend, &scenario(LOSS)).unwrap();
        assert_eq!(done.seed, None);
        assert!(!done.loss_reproducible);
        assert!(
            !backend.calls.iter().any(|c| c.contains("seed")),
            "{:?}",
            backend.calls
        );
    }

    /// Canary: with a written seed the same backend refuses, as the capability check says.
    #[test]
    fn with_a_written_seed_on_a_backend_that_cannot_seed_the_scenario_is_refused() {
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let error = provision(&mut backend, &scenario_with("seed: 1\n", LOSS)).unwrap_err();
        assert!(matches!(error, Error::Capability(_)));
        assert_eq!(backend.calls, Vec::<String>::new());
    }

    #[test]
    fn a_scenario_without_loss_is_reproducible_whatever_the_backend_can_seed() {
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let done = provision(&mut backend, &scenario("      both: { delay: 5ms }\n")).unwrap();
        assert!(done.loss_reproducible);
    }
}
