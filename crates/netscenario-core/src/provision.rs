//! Steps 2 to 4 of the life of a run (`docs/ARCHITECTURE.md`): check the capabilities, create the nodes and
//! links, and apply the impairments of time zero.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use netscenario_model::{Impairments, IpFamily, Link, Name, Problem, Scenario, Workload};

use crate::backend::{Backend, Direction, LinkHandle, NodeHandle};
use crate::capability::{Accepted, Support, check};
use crate::error::Error;
use crate::feature::Feature;
use crate::seed::{derive_seed, draw_seed};
use crate::timeline;

/// The address of a node on a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAddress {
    /// The node.
    pub node: Name,
    /// The link it is on.
    pub link: Name,
    /// The address of its end of the link, of one family (a dual-stack link has an entry for each).
    pub address: IpAddr,
}

/// The addresses of a node on its loopback, one for each family the network has (D-078, D-081).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct NodeIps {
    /// The IPv4 address, when the network has IPv4.
    pub v4: Option<Ipv4Addr>,
    /// The IPv6 address, when the network has IPv6.
    pub v6: Option<Ipv6Addr>,
}

impl NodeIps {
    /// The address of the family, if the node has one.
    #[must_use]
    pub fn of(&self, family: IpFamily) -> Option<IpAddr> {
        match family {
            IpFamily::V4 => self.v4.map(IpAddr::V4),
            IpFamily::V6 => self.v6.map(IpAddr::V6),
        }
    }

    /// The address that stands for the node where a single one is needed: the IPv4 one if it has it.
    #[must_use]
    pub fn primary(&self) -> Option<IpAddr> {
        self.of(IpFamily::V4).or_else(|| self.of(IpFamily::V6))
    }
}

/// What was created for a scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Provisioned {
    /// The nodes in the order of the scenario.
    pub nodes: Vec<(Name, NodeHandle)>,
    /// The links in the order of the scenario.
    pub links: Vec<(Name, LinkHandle)>,
    /// The address of each end of each link. The result records them (TOP-08).
    pub addresses: Vec<NodeAddress>,
    /// The address of each node on its loopback, in the order of the scenario (D-078). Workloads and programs use it; the
    /// routes lead to it.
    pub node_addresses: Vec<(Name, NodeIps)>,
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
    let problems = checked(scenario);
    if !problems.is_empty() {
        return Err(Error::InvalidScenario(problems));
    }
    let accepted = check(scenario, &backend.capabilities())?;
    provision_accepted(backend, scenario, accepted)
}

/// The problems of a scenario before anything is built: the rules of the format, and the entries of `allow-limited` that name no
/// feature.
pub(crate) fn checked(scenario: &Scenario) -> Vec<Problem> {
    let mut problems = scenario.validate();
    problems.extend(crate::capability::unknown_allowed_features(scenario));
    problems
}

/// Builds the network of `scenario`, which was checked against the capabilities of the backend as `accepted` says. The run checks
/// the scenario as the file writes it, with its models, and builds the one that the models made (D-090).
pub(crate) fn provision_accepted(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    accepted: Vec<Accepted>,
) -> Result<Provisioned, Error> {
    let problems = scenario.validate();
    if !problems.is_empty() {
        return Err(Error::InvalidScenario(problems));
    }
    backend.check_values(scenario)?;
    let can_seed = backend.capabilities().support(Feature::SeededLoss) == Support::Exact;
    // The moments of the packets of a Poisson source (and of the others with drawn phases) come from the seed too, whatever
    // the backend can apply (WRK-02, D-071): the agent is ours
    let random_traffic = scenario
        .workloads
        .iter()
        .filter_map(Workload::generator)
        .any(|generator| generator.pattern.is_random());
    let (seed, seed_drawn) = match scenario.seed {
        Some(seed) => (Some(seed), false),
        None if can_seed || random_traffic => (Some(draw_seed()), true),
        None => (None, false),
    };
    // The seed of the queues: none where it was drawn only for the traffic and the backend cannot apply one
    let queue_seed = if seed_drawn && !can_seed { None } else { seed };
    // A scenario whose result depends on random numbers (loss of any model, duplication, corruption,
    // reordering): the same seed gives the same lost packets only if the backend can apply one
    let has_random = timeline::draws_random_numbers(scenario);
    match build(backend, scenario, queue_seed) {
        Ok((nodes, links, addresses, node_addresses)) => Ok(Provisioned {
            nodes,
            links,
            addresses,
            node_addresses,
            accepted,
            seed,
            seed_drawn,
            loss_reproducible: !has_random || queue_seed.is_some(),
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
    Vec<(Name, NodeIps)>,
);

/// The address of the node number `index` (in the order of the scenario) on its loopback: `10.200.0.1`, `10.200.0.2`, and so
/// on, up to the 256 nodes of a scenario (D-078). It is not on a link, so it does not change with the path.
#[must_use]
pub fn node_address(index: usize) -> Ipv4Addr {
    let number = u32::try_from(index).unwrap_or(0) + 1;
    Ipv4Addr::new(
        10,
        200,
        u8::try_from(number >> 8).unwrap_or(0),
        (number & 0xff) as u8,
    )
}

/// The IPv6 address of the node number `index` on its loopback: `fd00:200::1`, `fd00:200::2`, and so on (D-081). A unique-local
/// address, as the IPv6 addresses of the links are (`fd00:100:<n>::/126`).
#[must_use]
pub fn node_address_v6(index: usize) -> Ipv6Addr {
    let number = u16::try_from(index + 1).unwrap_or(u16::MAX);
    Ipv6Addr::new(0xfd00, 0x200, 0, 0, 0, 0, 0, number)
}

fn build(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    seed: Option<u64>,
) -> Result<Built, Error> {
    let mode = scenario.topology.ip;
    backend.set_ip_families(mode.has(IpFamily::V4), mode.has(IpFamily::V6))?;
    let mut nodes = Vec::new();
    for node in &scenario.topology.nodes {
        nodes.push((node.id.clone(), backend.create_node(&node.id)?));
    }
    let node = |name: &Name| {
        nodes
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, handle)| *handle)
    };
    // The links first, then the addresses and the routes, then the queues: the interfaces of a network with IPv6 send packets of
    // their own while they come up, and the backend lets them pass (`finish_network`) before the first queue exists, so that no
    // queue counts a packet that no workload sent
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
        // A direction with a queue, or with a rate that changes, needs the fabric (D-079); the other links keep the direct build
        let handle = if needs_fabric(scenario, index, link) {
            backend.create_link_through_fabric(&link.id, [first, second])?
        } else {
            backend.create_link(&link.id, [first, second])?
        };
        if let Some(mtu) = link.mtu {
            backend.set_link_mtu(handle, mtu)?;
        }
        for (end, node) in link.between.iter().enumerate() {
            if mode.has(IpFamily::V4) {
                addresses.push(NodeAddress {
                    node: node.clone(),
                    link: link.id.clone(),
                    address: IpAddr::V4(backend.address(handle, end)?),
                });
            }
            if mode.has(IpFamily::V6) {
                addresses.push(NodeAddress {
                    node: node.clone(),
                    link: link.id.clone(),
                    address: IpAddr::V6(backend.address_v6(handle, end)?),
                });
            }
        }
        links.push((link.id.clone(), handle));
    }
    let node_addresses = routing(backend, scenario, &nodes, &links)?;
    backend.finish_network()?;
    let outages = timeline::outage_directions(scenario);
    for (index, (link, (_, handle))) in scenario.topology.links.iter().zip(&links).enumerate() {
        let handle = *handle;
        for direction in [Direction::Forward, Direction::Reverse] {
            // The states the timeline leads the direction through, the one it starts with first. A direction
            // that the timeline does not touch has only that one.
            let states = timeline::states_of(scenario, index, direction);
            let outage = outages.contains(&(index, direction));
            let changes = states.len() > 1 || outage;
            if changes {
                backend.plan_states(handle, direction, &states)?;
            }
            if outage {
                backend.plan_outage(handle, direction)?;
            }
            // Only what draws random numbers uses the seed, and a state that comes later counts as well: the
            // queue is seeded when it is built (a change keeps its generator)
            let seed = seed
                .filter(|_| states.iter().any(Impairments::is_random))
                .map(|seed| derive_seed(seed, index, direction));
            if let Some(impairments) = impairments_of(link, direction) {
                backend.apply(handle, direction, impairments, seed)?;
            } else if changes {
                // Nothing at the start and something later: the queue has to be there to be changed
                backend.apply(handle, direction, &Impairments::default(), seed)?;
            }
        }
    }
    Ok((nodes, links, addresses, node_addresses))
}

/// Gives the nodes their addresses, makes the routers forward and installs the routes (D-078), for each family the network has
/// (D-081). Returns the addresses.
fn routing(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    nodes: &[(Name, NodeHandle)],
    links: &[(Name, LinkHandle)],
) -> Result<Vec<(Name, NodeIps)>, Error> {
    let topology = &scenario.topology;
    let mode = topology.ip;
    let addresses: Vec<(Name, NodeIps)> = topology
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                node.id.clone(),
                NodeIps {
                    v4: mode.has(IpFamily::V4).then(|| node_address(index)),
                    v6: mode.has(IpFamily::V6).then(|| node_address_v6(index)),
                },
            )
        })
        .collect();
    let address_of = |name: &Name, family: IpFamily| {
        addresses
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, ips)| ips.of(family))
    };
    let handle_of = |name: &Name| nodes.iter().find(|(n, _)| n == name).map(|(_, h)| *h);
    let missing = |what: &str, name: &Name| {
        Error::InvalidScenario(vec![Problem::new(format!(
            "the {what} '{name}' was not built, so a route cannot lead to it"
        ))])
    };
    for (node, (_, ips)) in topology.nodes.iter().zip(&addresses) {
        let handle = handle_of(&node.id).ok_or_else(|| missing("node", &node.id))?;
        for family in [IpFamily::V4, IpFamily::V6] {
            if let Some(address) = ips.of(family) {
                backend.set_node_address(handle, address)?;
            }
        }
        // Set on every node and not only on routers: a new network namespace inherits `ip_forward` from the machine, so
        // on a machine that forwards (Docker sets it) a host in the middle of a chain would carry packets that only a router
        // may carry
        backend.set_forwarding(handle, node.role == netscenario_model::Role::Router)?;
    }
    // The routes of a node with the same destination and metric are one route with several ways out
    let routes = topology.routes();
    let mut done: Vec<(Name, netscenario_model::Destination, u32)> = Vec::new();
    for entry in &routes.entries {
        let key = (entry.node.clone(), entry.to.clone(), entry.metric);
        if done.contains(&key) {
            continue;
        }
        done.push(key);
        let handle = handle_of(&entry.node).ok_or_else(|| missing("node", &entry.node))?;
        let mut next_hops = Vec::new();
        for same in routes.entries.iter().filter(|other| {
            other.node == entry.node && other.to == entry.to && other.metric == entry.metric
        }) {
            let link = topology
                .link(&same.link)
                .ok_or_else(|| missing("link", &same.link))?;
            let link_handle = links
                .iter()
                .find(|(n, _)| *n == same.link)
                .map(|(_, h)| *h)
                .ok_or_else(|| missing("link", &same.link))?;
            next_hops.push(crate::backend::NextHop {
                link: link_handle,
                end: usize::from(link.between[1] == entry.node),
                weight: same.weight,
            });
        }
        // The same route in each family the network has: the next hops are the other ends of the same links
        for family in [IpFamily::V4, IpFamily::V6] {
            if !mode.has(family) {
                continue;
            }
            let destination = match &entry.to {
                netscenario_model::Destination::Node(name) => {
                    Some(address_of(name, family).ok_or_else(|| missing("node", name))?)
                }
                netscenario_model::Destination::Default => None,
            };
            backend.add_route(
                handle,
                &crate::backend::NodeRoute {
                    family,
                    destination,
                    metric: entry.metric,
                    next_hops: next_hops.clone(),
                },
            )?;
        }
    }
    Ok(addresses)
}

/// Whether the link number `index` is built through the fabric: a direction has a `queue`, or the rate of a direction changes
/// during the run, which a queue of the kernel follows and a `netem` does not (D-079, D-091).
#[must_use]
pub fn needs_fabric(scenario: &Scenario, index: usize, link: &Link) -> bool {
    has_queue(link)
        || [Direction::Forward, Direction::Reverse]
            .into_iter()
            .any(|direction| {
                let states = timeline::states_of(scenario, index, direction);
                states.iter().any(|state| state.rate != states[0].rate)
            })
}

/// Whether a direction of the link has a `queue`, which makes the Kernel backend build it through the fabric (D-079).
#[must_use]
pub fn has_queue(link: &Link) -> bool {
    [&link.forward, &link.reverse, &link.both]
        .into_iter()
        .flatten()
        .any(|impairments| impairments.queue.is_some())
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
        /// The addresses, the forwarding and the routes that were set, apart from the steps above.
        routing: Vec<String>,
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

        fn create_link_through_fabric(
            &mut self,
            name: &Name,
            between: [NodeHandle; 2],
        ) -> Result<LinkHandle, Error> {
            self.step(
                format!("fabric link {name} {}-{}", between[0].0, between[1].0),
                "create_link",
            )?;
            Ok(LinkHandle(u32::try_from(self.calls.len()).unwrap()))
        }

        fn set_ip_families(&mut self, v4: bool, v6: bool) -> Result<(), Error> {
            self.routing.push(format!("families {v4} {v6}"));
            Ok(())
        }

        fn finish_network(&mut self) -> Result<(), Error> {
            self.routing.push("finish".to_owned());
            Ok(())
        }

        fn address_v6(&self, link: LinkHandle, end: usize) -> Result<Ipv6Addr, Error> {
            Ok(Ipv6Addr::new(
                0xfd00,
                0x100,
                u16::try_from(link.0).unwrap(),
                0,
                0,
                0,
                0,
                1 + u16::try_from(end).unwrap(),
            ))
        }

        fn set_node_address(&mut self, node: NodeHandle, address: IpAddr) -> Result<(), Error> {
            self.routing.push(format!("address {} {address}", node.0));
            Ok(())
        }

        fn set_link_mtu(&mut self, link: LinkHandle, mtu: u32) -> Result<(), Error> {
            self.routing.push(format!("mtu {} {mtu}", link.0));
            Ok(())
        }

        fn set_forwarding(&mut self, node: NodeHandle, on: bool) -> Result<(), Error> {
            self.routing.push(format!("forwarding {} {on}", node.0));
            Ok(())
        }

        fn add_route(
            &mut self,
            node: NodeHandle,
            route: &crate::backend::NodeRoute,
        ) -> Result<(), Error> {
            let ways: Vec<String> = route
                .next_hops
                .iter()
                .map(|hop| format!("{}/{}x{}", hop.link.0, hop.end, hop.weight))
                .collect();
            self.routing.push(format!(
                "route {} {} metric {} via {}",
                node.0,
                route
                    .destination
                    .map_or("default".to_owned(), |a| a.to_string()),
                route.metric,
                ways.join(",")
            ));
            Ok(())
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

        fn plan_states(
            &mut self,
            link: LinkHandle,
            direction: Direction,
            states: &[Impairments],
        ) -> Result<(), Error> {
            self.step(
                format!("plan {} {direction:?} {} states", link.0, states.len()),
                "plan_states",
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

    fn network(topology: &str) -> Scenario {
        Scenario::from_yaml(
            &format!("version: 0\nname: t\nduration: 10s\ntopology:\n{topology}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: c, rate: 1mbit, packet-size: 1200b }}\n"),
            "t.yaml",
        )
        .expect("valid")
    }

    /// Every node gets an address of its own, a router forwards and the others do not, and every node has a route to every
    /// other node out of the link that leads to it (D-078).
    #[test]
    fn nodes_get_addresses_routers_forward_and_the_routes_are_installed() {
        let mut backend = Fake::new();
        let done = provision(
            &mut backend,
            &network("  nodes: [a, { id: r, role: router }, c]\n  links:\n    - { id: x, between: [a, r] }\n    - { id: z, between: [r, c] }\n"),
        )
        .unwrap();
        let shown: Vec<String> = done
            .node_addresses
            .iter()
            .map(|(name, ips)| format!("{name} {}", ips.primary().unwrap()))
            .collect();
        assert_eq!(shown, ["a 10.200.0.1", "r 10.200.0.2", "c 10.200.0.3"]);
        let routing = backend.routing.join("\n");
        // Every node is told, and only the router forwards (a host would inherit the setting of the machine otherwise)
        let forwarding: Vec<&str> = routing
            .lines()
            .filter(|line| line.starts_with("forwarding"))
            .collect();
        assert_eq!(forwarding.len(), 3, "{routing}");
        assert_eq!(
            forwarding
                .iter()
                .filter(|line| line.ends_with("true"))
                .count(),
            1,
            "{routing}"
        );
        assert!(forwarding[1].ends_with("true"), "{routing}");
        // Six routes: each of the three nodes to each of the other two
        assert_eq!(routing.matches("route ").count(), 6, "{routing}");
        // `a` reaches `c` (10.200.0.3) out of the link x at its first end (`a` is the first node of the link)
        assert!(
            routing.contains(&format!(
                "route {} 10.200.0.3 metric 100 via {}/0x1",
                done.nodes[0].1.0, done.links[0].1.0
            )),
            "{routing}"
        );
        // `c` reaches `a` out of the link z at its second end
        assert!(
            routing.contains(&format!(
                "route {} 10.200.0.1 metric 100 via {}/1x1",
                done.nodes[2].1.0, done.links[1].1.0
            )),
            "{routing}"
        );
    }

    /// Equally short paths with `ecmp` are one route with two ways, weighted; routes of different metrics are two routes.
    #[test]
    fn a_multipath_is_one_route_with_several_ways_and_a_failover_is_two_routes() {
        let diamond = "  nodes: [a, { id: r1, role: router }, { id: r2, role: router }, c]\n  links:\n    - { id: ar1, between: [a, r1] }\n    - { id: r1c, between: [r1, c] }\n    - { id: ar2, between: [a, r2] }\n    - { id: r2c, between: [r2, c] }\n";
        let mut backend = Fake::new();
        provision(&mut backend, &network(&format!("{diamond}  ecmp: true\n"))).unwrap();
        let routing = backend.routing.join("\n");
        let to_c: Vec<&str> = routing
            .lines()
            .filter(|line| line.contains("10.200.0.4 metric 100") && line.contains(','))
            .collect();
        assert_eq!(to_c.len(), 1, "one route from a with two ways:\n{routing}");
        // Failover: two listed routes with different metrics
        let mut backend = Fake::new();
        provision(
            &mut backend,
            &network(&format!("{diamond}  routes:\n    - {{ node: a, to: c, via: ar1, metric: 10 }}\n    - {{ node: a, to: c, via: ar2, metric: 20 }}\n")),
        )
        .unwrap();
        let routing = backend.routing.join("\n");
        assert!(routing.contains("10.200.0.4 metric 10 via"), "{routing}");
        assert!(routing.contains("10.200.0.4 metric 20 via"), "{routing}");
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

    /// A network with both families (D-081): the backend is told before anything is made, every link and node gets an address
    /// of each family, every route is there twice, and the network is finished (the wait for the quiet) before the first
    /// queue is built.
    #[test]
    fn a_dual_stack_network_has_both_families_and_waits_before_the_first_queue() {
        let mut backend = Fake::new();
        let done = provision(
            &mut backend,
            &network("  ip: dual\n  nodes: [a, { id: r, role: router }, c]\n  links:\n    - { id: x, between: [a, r], mtu: 1400, forward: { delay: 5ms } }\n    - { id: z, between: [r, c] }\n"),
        )
        .unwrap();
        let routing = backend.routing.join("\n");
        assert!(routing.starts_with("families true true"), "{routing}");
        // The link x is 0 + 1 = first link: its MTU is set, and the queue comes after the routes and the wait
        assert!(routing.contains("mtu"), "{routing}");
        let finish = routing.find("finish").unwrap();
        assert!(routing.rfind("route ").unwrap() < finish, "{routing}");
        let apply = backend
            .calls
            .iter()
            .position(|call| call.starts_with("apply"))
            .unwrap();
        assert_eq!(backend.calls[..apply].len(), apply);
        // Four addresses for each link: an end of each family
        assert_eq!(done.addresses.len(), 8);
        assert_eq!(
            done.addresses
                .iter()
                .filter(|a| a.address.is_ipv6())
                .count(),
            4
        );
        // The node addresses of both families, and six routes in each
        let (name, ips) = &done.node_addresses[2];
        assert_eq!(name.as_str(), "c");
        assert_eq!(ips.v4.unwrap().to_string(), "10.200.0.3");
        assert_eq!(ips.v6.unwrap().to_string(), "fd00:200::3");
        assert_eq!(routing.matches("route ").count(), 12, "{routing}");
        assert!(routing.contains("fd00:200::3 metric 100"), "{routing}");
        // The address of a node where only one is needed is the IPv4 one
        assert_eq!(ips.primary().unwrap().to_string(), "10.200.0.3");
    }

    /// An IPv6 network has no IPv4 address anywhere, and an IPv4 network has no IPv6 address anywhere.
    #[test]
    fn a_network_has_the_addresses_of_its_family_only() {
        let topology = "  nodes: [a, { id: r, role: router }, c]\n  links:\n    - { id: x, between: [a, r] }\n    - { id: z, between: [r, c] }\n";
        let mut v6 = Fake::new();
        let done = provision(&mut v6, &network(&format!("  ip: v6\n{topology}"))).unwrap();
        assert!(done.addresses.iter().all(|a| a.address.is_ipv6()));
        assert!(done.node_addresses.iter().all(|(_, ips)| ips.v4.is_none()));
        let routing = v6.routing.join("\n");
        assert!(routing.starts_with("families false true"), "{routing}");
        assert!(!routing.contains("10.200"), "{routing}");
        let mut v4 = Fake::new();
        let done = provision(&mut v4, &network(topology)).unwrap();
        assert!(done.addresses.iter().all(|a| a.address.is_ipv4()));
        assert!(done.node_addresses.iter().all(|(_, ips)| ips.v6.is_none()));
        assert!(v4.routing.join("\n").starts_with("families true false"));
    }

    /// A link with a `queue` in a direction is built through the fabric (D-079); a link without one is built as before.
    #[test]
    fn a_link_with_a_queue_is_made_through_the_fabric_and_the_others_directly() {
        let mut backend = Fake::new();
        provision(
            &mut backend,
            &network("  nodes: [a, { id: r, role: router }, c]\n  links:\n    - { id: x, between: [a, r] }\n    - { id: z, between: [r, c], forward: { rate: 2mbit, queue: { limit: 50p } } }\n"),
        )
        .unwrap();
        assert!(
            backend
                .calls
                .iter()
                .any(|call| call.starts_with("fabric link z")),
            "{:?}",
            backend.calls
        );
        assert!(
            backend.calls.iter().any(|call| call.starts_with("link x")),
            "{:?}",
            backend.calls
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

    /// A direction that the timeline changes is told its states before it is built, and is built also if it has
    /// nothing at the start; a direction that the timeline does not touch is built as before.
    #[test]
    fn a_direction_that_the_timeline_changes_is_planned_before_it_is_built() {
        let mut backend = Fake::new();
        let scenario = Scenario::from_yaml(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 20ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\ntimeline:\n  - { at: 2s, link: l, set: { forward: { delay: 40ms } } }\n  - { at: 4s, link: l, set: { reverse: { delay: 5ms } } }\n",
            "t.yaml",
        )
        .unwrap();
        provision(&mut backend, &scenario).unwrap();
        assert_eq!(
            &backend.calls[3..],
            [
                "plan 3 Forward 2 states",
                "apply 3 Forward 20ms",
                "plan 3 Reverse 2 states",
                // Nothing at the start, and the queue is there to be changed
                "apply 3 Reverse "
            ]
        );
        // Canary: the same scenario without its timeline plans nothing and builds only what it has
        let mut plain = Fake::new();
        provision(
            &mut plain,
            &self::scenario("      forward: { delay: 20ms }\n"),
        )
        .unwrap();
        assert!(plain.calls.iter().all(|call| !call.starts_with("plan")));
        assert_eq!(plain.calls.len(), 4);
    }

    /// A loss that a step switches on later needs a seed from the start: the queue is built once, and a change keeps
    /// its generator.
    #[test]
    fn a_loss_that_comes_later_is_seeded_when_the_queue_is_built() {
        let mut backend = Fake::new();
        let scenario = Scenario::from_yaml(
            "version: 0\nname: t\nseed: 9\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 20ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\ntimeline:\n  - { at: 2s, link: l, set: { forward: { loss: { model: random, p: 5% } } } }\n",
            "t.yaml",
        )
        .unwrap();
        let built = provision(&mut backend, &scenario).unwrap();
        assert!(built.loss_reproducible);
        let apply = backend
            .calls
            .iter()
            .find(|call| call.starts_with("apply"))
            .unwrap();
        assert!(apply.contains(" seed "), "{apply}");
        // Canary: without a loss in any state there is no seed
        let mut quiet = Fake::new();
        provision(
            &mut quiet,
            &self::scenario("      forward: { delay: 20ms }\n"),
        )
        .unwrap();
        assert!(!quiet.calls[3].contains("seed"));
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

    /// The moments of a Poisson source come from the seed of the run (D-071), and the agent is ours, so the run has a seed for
    /// them also where the backend cannot apply one to the queues; the queues are then built without it and the loss is
    /// not reproducible, as it was.
    #[test]
    fn a_random_source_draws_a_seed_also_where_the_queues_cannot_use_it() {
        let poisson = |link: &str| {
            Scenario::from_yaml(
                &format!("version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}workloads:\n  - {{ id: w, kind: udp-poisson, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"),
                "t.yaml",
            )
            .expect("valid")
        };
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let done = provision(&mut backend, &poisson("      both: { delay: 5ms }\n")).unwrap();
        assert!(done.seed.is_some() && done.seed_drawn);
        assert!(done.loss_reproducible, "there is no loss");
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let lossy = poisson("      both: { loss: { model: random, p: 1% } }\n");
        let done = provision(&mut backend, &lossy).unwrap();
        assert!(done.seed.is_some() && done.seed_drawn);
        assert!(!done.loss_reproducible, "the queues were not seeded");
        assert!(
            !backend.calls.iter().any(|c| c.contains("seed")),
            "{:?}",
            backend.calls
        );
        // Canary: a constant bit rate draws nothing, so it still runs without a seed on this backend
        let mut backend = Fake::new();
        backend.cannot_seed = true;
        let done = provision(&mut backend, &scenario("      both: { delay: 5ms }\n")).unwrap();
        assert_eq!(done.seed, None);
    }
}
