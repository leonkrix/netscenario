//! Steps 2 to 4 of the life of a run (`docs/ARCHITECTURE.md`): check the capabilities, create the nodes and
//! links, and apply the impairments of time zero.

use netscenario_model::{Impairments, Link, Name, Problem, Scenario};

use crate::backend::{Backend, Direction, LinkHandle, NodeHandle};
use crate::capability::{Accepted, check};
use crate::error::Error;
use crate::seed::derive_seed;

/// What was created for a scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    /// The nodes in the order of the scenario.
    pub nodes: Vec<(Name, NodeHandle)>,
    /// The links in the order of the scenario.
    pub links: Vec<(Name, LinkHandle)>,
    /// The restrictions that the scenario accepted. The result has to record them.
    pub accepted: Vec<Accepted>,
}

/// Builds the network of `scenario` with `backend`.
///
/// The capabilities are checked before anything is created, so a refused scenario leaves nothing behind. If a
/// step fails after that, the backend is torn down before the error is returned.
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
    match build(backend, scenario) {
        Ok((nodes, links)) => Ok(Provisioned {
            nodes,
            links,
            accepted,
        }),
        Err(error) => {
            // The first error is the one to report; a failure of the teardown is the backend's to log
            let _ = backend.teardown();
            Err(error)
        }
    }
}

type Built = (Vec<(Name, NodeHandle)>, Vec<(Name, LinkHandle)>);

fn build(backend: &mut dyn Backend, scenario: &Scenario) -> Result<Built, Error> {
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
                let seed = scenario
                    .seed
                    .map(|seed| derive_seed(seed, index, direction));
                backend.apply(handle, direction, impairments, seed)?;
            }
        }
        links.push((link.id.clone(), handle));
    }
    Ok((nodes, links))
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
            Feature::ALL
                .iter()
                .fold(Capabilities::new("fake"), |caps, f| {
                    caps.with(*f, support.clone())
                })
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

        fn teardown(&mut self) -> Result<(), Error> {
            self.calls.push("teardown".to_owned());
            Ok(())
        }
    }

    use crate::backend::ProgramHandle;

    fn scenario(link_body: &str) -> Scenario {
        Scenario::from_yaml(
            &format!("version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link_body}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"),
            "t.yaml",
        )
        .expect("valid")
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
}
