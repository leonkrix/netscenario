//! The rules that the types cannot state: names that must exist, ranges, and what the Kernel backend can apply
//! exactly. Every problem names the path of the value in the scenario.

use crate::error::Problem;
use crate::generators::{Generator, Pattern};
use crate::graph::{PathError, Routes};
use crate::measure::{MAX_BINS, MIN_BIN};
use crate::scenario::{
    Change, Direction, Event, Impairments, Jitter, JitterDistribution, Link, Loss, Scenario,
    Workload,
};
use crate::suggest::closest;
use crate::topology::{
    DEFAULT_DESTINATION, DEFAULT_METRIC, MAX_LINKS, MAX_NODES, MAX_ROUTES, Role, Route, Routing,
};
use crate::units::{Duration, Name, Probability, Rate};

/// The largest cost of a link.
pub const MAX_COST: u32 = 1_000_000;
/// The largest weight of a route (the kernel's limit for a multipath next hop).
pub const MAX_WEIGHT: u32 = 256;
/// The packets per second that a workload may send: what the agent was measured to deliver (spike S8).
pub const MAX_PACKETS_PER_SECOND: u64 = 100_000;
/// The smallest payload: the header of the agent.
pub const MIN_PACKET_SIZE: u64 = 16;
/// The smallest payload with `udp-checksum: false`: the header of the agent and the CRC-32 that takes the place
/// of the checksum (D-040).
pub const MIN_PACKET_SIZE_WITHOUT_CHECKSUM: u64 = 20;
/// The largest payload that fits the MTU of 1500 without fragmentation (IPv4 and UDP headers take 28 bytes).
pub const MAX_PACKET_SIZE: u64 = 1472;

impl Scenario {
    /// Checks the rules that the types cannot state and returns every problem found (none if the scenario is
    /// valid). A scenario that was read from a file has been through this check already; one that is built in
    /// code should be checked before it is run.
    #[must_use]
    pub fn validate(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.duration.is_zero() {
            problems
                .push(Problem::new("the duration must be greater than zero").at_path("duration"));
        }
        let before = problems.len();
        let nodes = self.check_nodes(&mut problems);
        // A wrong list of nodes would make every reference to a node a second problem
        let nodes_ok = problems.len() == before;
        self.check_links(&nodes, nodes_ok, &mut problems);
        let links_ok = problems.len() == before;
        if links_ok {
            self.check_routes(&mut problems);
        }
        // Routes that are wrong would make every path a second problem
        let structure_ok = problems.len() == before;
        let routes = structure_ok.then(|| self.topology.routes());
        self.check_workloads(&nodes, nodes_ok, routes.as_ref(), &mut problems);
        self.check_timeline(&mut problems);
        self.check_measure(&mut problems);
        problems
    }

    fn check_nodes(&self, problems: &mut Vec<Problem>) -> Vec<Name> {
        let nodes: Vec<Name> = self
            .topology
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        if nodes.len() < 2 {
            problems.push(
                Problem::new(format!("a network needs at least two nodes, found {}", nodes.len()))
                    .at_path("topology.nodes")
                    .with_help("a node alone has nothing to talk to: add one, or write a star, chain or dumbbell"),
            );
        }
        if nodes.len() > MAX_NODES {
            problems.push(
                Problem::new(format!(
                    "a scenario may have at most {MAX_NODES} nodes, found {}",
                    nodes.len()
                ))
                .at_path("topology.nodes"),
            );
        }
        for (index, node) in nodes.iter().enumerate() {
            if nodes[..index].contains(node) {
                problems.push(
                    Problem::new(format!("the node name '{node}' is used twice"))
                        .at_path(format!("topology.nodes[{index}]"))
                        .with_help("every node needs its own name"),
                );
            }
            if node.as_str() == DEFAULT_DESTINATION {
                problems.push(
                    Problem::new(format!(
                        "'{DEFAULT_DESTINATION}' is the word for every destination in a route and cannot be the name of a node"
                    ))
                    .at_path(format!("topology.nodes[{index}]"))
                    .with_help("choose another name"),
                );
            }
        }
        nodes
    }

    /// The MTU of a link: in range, and at least 1280 where the network has IPv6 (D-081).
    fn check_mtu(&self, link: &Link, base: &str, problems: &mut Vec<Problem>) {
        let smallest_mtu = if self.topology.ip.has(crate::IpFamily::V6) {
            crate::MIN_MTU_V6
        } else {
            crate::MIN_MTU
        };
        if let Some(mtu) = link.mtu
            && !(smallest_mtu..=crate::MAX_MTU).contains(&mtu)
        {
            problems.push(
            Problem::new(format!(
                "the MTU {mtu} is outside {smallest_mtu} to {} bytes",
                crate::MAX_MTU
            ))
            .at_path(format!("{base}.mtu"))
            .with_help(if smallest_mtu == crate::MIN_MTU_V6 {
                "the MTU is the size of the largest IP packet the interface carries; 1500 is the default, and IPv6 needs 1280 at least"
            } else {
                "the MTU is the size of the largest IP packet the interface carries; 1500 is the default"
            }),
        );
        }
    }

    fn check_links(&self, nodes: &[Name], nodes_ok: bool, problems: &mut Vec<Problem>) {
        let links = &self.topology.links;
        let before = problems.len();
        if links.is_empty() {
            problems.push(
                Problem::new("a network needs at least one link, found none")
                    .at_path("topology.links")
                    .with_help("add a link between two of the nodes"),
            );
        }
        if links.len() > MAX_LINKS {
            problems.push(
                Problem::new(format!(
                    "a scenario may have at most {MAX_LINKS} links, found {}",
                    links.len()
                ))
                .at_path("topology.links"),
            );
        }
        for (index, link) in links.iter().enumerate() {
            let base = format!("topology.links[{index}]");
            if links[..index].iter().any(|other| other.id == link.id) {
                problems.push(
                    Problem::new(format!("the link name '{}' is used twice", link.id))
                        .at_path(format!("{base}.id")),
                );
            }
            if link.id.as_str() == DEFAULT_DESTINATION {
                problems.push(
                    Problem::new(format!(
                        "'{DEFAULT_DESTINATION}' is a word of the routes and cannot be the name of a link"
                    ))
                    .at_path(format!("{base}.id"))
                    .with_help("choose another name"),
                );
            }
            if let Some(cost) = link.cost
                && !(1..=MAX_COST).contains(&cost)
            {
                problems.push(
                    Problem::new(format!("the cost {cost} is outside 1 to {MAX_COST}"))
                        .at_path(format!("{base}.cost"))
                        .with_help("the cost is a whole number; 1 is the default"),
                );
            }
            self.check_mtu(link, &base, problems);
            let [first, second] = &link.between;
            if first == second {
                problems.push(
                    Problem::new(format!(
                        "a link needs two different nodes, both are '{first}'"
                    ))
                    .at_path(format!("{base}.between[1]")),
                );
            }
            for (side, node) in link.between.iter().enumerate() {
                if nodes_ok && !nodes.contains(node) {
                    problems.push(unknown_node(node, nodes, format!("{base}.between[{side}]")));
                }
            }
            if link.both.is_some() && (link.forward.is_some() || link.reverse.is_some()) {
                problems.push(
                    Problem::new("`both` cannot be combined with `forward` or `reverse`")
                        .at_path(format!("{base}.both"))
                        .with_help("use `both` for the same values in both directions, or `forward` and `reverse` for different ones"),
                );
            }
            for (name, impairments) in [
                ("forward", &link.forward),
                ("reverse", &link.reverse),
                ("both", &link.both),
            ] {
                if let Some(impairments) = impairments {
                    check_impairments(impairments, &format!("{base}.{name}"), problems);
                }
            }
        }
        // A link that is wrong would make the nodes without a link a second problem
        if nodes_ok && problems.len() == before {
            for (index, node) in nodes.iter().enumerate() {
                if !links.iter().any(|link| link.between.contains(node)) {
                    problems.push(
                        Problem::new(format!("the node '{node}' has no link"))
                            .at_path(format!("topology.nodes[{index}]"))
                            .with_help("add a link to it, or remove the node"),
                    );
                }
            }
        }
    }

    /// The rules of the routes and of the routing, with the nodes and links known to exist.
    fn check_routes(&self, problems: &mut Vec<Problem>) {
        let topology = &self.topology;
        if topology.routes.len() > MAX_ROUTES {
            problems.push(
                Problem::new(format!(
                    "a scenario may list at most {MAX_ROUTES} routes, found {}",
                    topology.routes.len()
                ))
                .at_path("topology.routes"),
            );
        }
        if topology.ecmp && topology.routing == Routing::Manual {
            problems.push(
                Problem::new("`ecmp` needs `routing: auto`: with `manual` the routes are only the listed ones")
                    .at_path("topology.ecmp")
                    .with_help("list several routes with the same metric to share the packets of a node"),
            );
        }
        for (index, route) in topology.routes.iter().enumerate() {
            self.check_route(index, route, problems);
        }
    }

    fn check_route(&self, index: usize, route: &Route, problems: &mut Vec<Problem>) {
        let topology = &self.topology;
        let node_names: Vec<&str> = topology.nodes.iter().map(|node| node.id.as_str()).collect();
        let link_names: Vec<&str> = topology.links.iter().map(|link| link.id.as_str()).collect();
        {
            let base = format!("topology.routes[{index}]");
            if topology.node(&route.node).is_none() {
                problems.push(unknown_node(
                    &route.node,
                    &topology
                        .nodes
                        .iter()
                        .map(|n| n.id.clone())
                        .collect::<Vec<_>>(),
                    format!("{base}.node"),
                ));
            }
            if !route.is_default() && topology.node(&route.to).is_none() {
                let mut known = node_names.clone();
                known.push(DEFAULT_DESTINATION);
                let list = format!(
                    "the destinations are the nodes ({}) and `{DEFAULT_DESTINATION}`",
                    node_names.join(", ")
                );
                let help = match closest(route.to.as_str(), &known) {
                    Some(near) => format!("did you mean '{near}'? {list}"),
                    None => list,
                };
                problems.push(
                    Problem::new(format!(
                        "the destination '{}' is neither a node nor `{DEFAULT_DESTINATION}`",
                        route.to
                    ))
                    .at_path(format!("{base}.to"))
                    .with_help(help),
                );
            }
            match topology.link(&route.via) {
                None => problems.push(unknown_link(&route.via, &link_names, format!("{base}.via"))),
                Some(link) if !link.between.contains(&route.node) => {
                    if topology.node(&route.node).is_some() {
                        let own: Vec<&str> = topology
                            .links
                            .iter()
                            .filter(|other| other.between.contains(&route.node))
                            .map(|other| other.id.as_str())
                            .collect();
                        problems.push(
                            Problem::new(format!(
                                "the link '{}' does not touch the node '{}'",
                                route.via, route.node
                            ))
                            .at_path(format!("{base}.via"))
                            .with_help(format!(
                                "the links of '{}' are: {}",
                                route.node,
                                own.join(", ")
                            )),
                        );
                    }
                }
                Some(_) => {}
            }
            if route.to == route.node {
                problems.push(
                    Problem::new(format!("a route of '{}' to itself", route.node))
                        .at_path(format!("{base}.to"))
                        .with_help("a node reaches its own address without a route"),
                );
            }
            if route.metric == Some(0) {
                problems.push(
                    Problem::new("the metric 0 is not allowed")
                        .at_path(format!("{base}.metric"))
                        .with_help(format!("use 1 or more; the default is {DEFAULT_METRIC}")),
                );
            }
            if let Some(weight) = route.weight
                && !(1..=MAX_WEIGHT).contains(&weight)
            {
                problems.push(
                    Problem::new(format!("the weight {weight} is outside 1 to {MAX_WEIGHT}"))
                        .at_path(format!("{base}.weight")),
                );
            }
            if topology.routes[..index].iter().any(|other| {
                other.node == route.node && other.to == route.to && other.via == route.via
            }) {
                problems.push(
                    Problem::new(format!(
                        "the route of '{}' to '{}' out of '{}' is listed twice",
                        route.node, route.to, route.via
                    ))
                    .at_path(base),
                );
            }
        }
    }

    fn check_workloads(
        &self,
        nodes: &[Name],
        nodes_ok: bool,
        routes: Option<&Routes>,
        problems: &mut Vec<Problem>,
    ) {
        if self.workloads.is_empty() {
            problems.push(
                Problem::new("a scenario needs at least one workload, found none")
                    .at_path("workloads")
                    .with_help("add a workload of kind udp-cbr, udp-poisson, udp-onoff or udp-bursty, or a command"),
            );
        }
        for (index, workload) in self.workloads.iter().enumerate() {
            let base = format!("workloads[{index}]");
            if self.workloads[..index]
                .iter()
                .any(|other| other.id() == workload.id())
            {
                problems.push(
                    Problem::new(format!(
                        "the workload name '{}' is used twice",
                        workload.id()
                    ))
                    .at_path(format!("{base}.id")),
                );
            }
            match workload {
                Workload::Command(command) => {
                    crate::command::check(command, &base, nodes, nodes_ok, problems);
                }
                other => {
                    if let Some(generator) = other.generator() {
                        self.check_generator(&generator, &base, nodes, nodes_ok, routes, problems);
                    }
                }
            }
        }
    }

    /// A packet of the workload must get from the sender to the receiver over the routes.
    fn check_path(
        &self,
        routes: &Routes,
        from: &Name,
        to: &Name,
        base: &str,
        problems: &mut Vec<Problem>,
    ) {
        let Err(error) = self.topology.path(routes, from, to) else {
            return;
        };
        let manual = self.topology.routing == Routing::Manual;
        let (message, help) = match error {
            PathError::NoRoute { at } => {
                let links: Vec<&str> = self
                    .topology
                    .links
                    .iter()
                    .filter(|link| link.between.contains(&at))
                    .map(|link| link.id.as_str())
                    .collect();
                let message = if at == *from {
                    format!("'{from}' has no route to '{to}'")
                } else {
                    format!(
                        "the packets from '{from}' to '{to}' are lost at '{at}': it has no route to '{to}'"
                    )
                };
                let help = if manual {
                    format!(
                        "the routing is manual, so list the route: `routes: [{{ node: {at}, to: {to}, via: <link> }}]`; the links of '{at}' are: {}",
                        links.join(", ")
                    )
                } else {
                    let hosts = self.hosts_between(from, to);
                    if hosts.is_empty() {
                        format!(
                            "no chain of links joins '{at}' and '{to}': add a link, or list a route"
                        )
                    } else {
                        let list: Vec<String> =
                            hosts.iter().map(|name| format!("'{name}'")).collect();
                        format!(
                            "the links lead through {}, which do not forward because they are hosts: write them as `{{ id: <name>, role: router }}`",
                            list.join(", ")
                        )
                    }
                };
                (message, help)
            }
            PathError::NotARouter { at } => (
                format!(
                    "the route from '{from}' to '{to}' leads through '{at}', which is a host and does not forward"
                ),
                format!("write the node as `{{ id: {at}, role: router }}`, or route around it"),
            ),
            PathError::Loop { at } => (
                format!(
                    "the routes from '{from}' to '{to}' lead in a circle and come back to '{at}'"
                ),
                format!("check the routes of '{at}' to '{to}'"),
            ),
        };
        problems.push(
            Problem::new(message)
                .at_path(format!("{base}.to"))
                .with_help(help),
        );
    }

    /// The hosts on the shortest path between two nodes if every node were a router (empty if no chain of links
    /// joins them).
    fn hosts_between(&self, from: &Name, to: &Name) -> Vec<Name> {
        let mut open = self.topology.clone();
        for node in &mut open.nodes {
            node.role = Role::Router;
        }
        open.routing = Routing::Auto;
        open.routes.clear();
        let routes = open.routes();
        open.path(&routes, from, to)
            .map(|path| {
                path.nodes
                    .into_iter()
                    .skip(1)
                    .filter(|name| name != to)
                    .filter(|name| {
                        self.topology
                            .node(name)
                            .is_some_and(|node| node.role == Role::Host)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The family of the flow: its own `ip`, or the only one of the network (D-081). The family must be one the network has,
    /// and over IPv6 the checksum of UDP cannot be off.
    fn check_family(
        &self,
        workload: &Generator<'_>,
        base: &str,
        problems: &mut Vec<Problem>,
    ) -> crate::IpFamily {
        // The family of the flow: its own `ip`, or the only one of the network (D-081)
        let mode = self.topology.ip;
        let family = workload.ip.unwrap_or_else(|| mode.default_family());
        if !mode.has(family) {
            problems.push(
                Problem::new(format!(
                    "the workload asks for {} but the network has {} only",
                    if family == crate::IpFamily::V6 { "IPv6" } else { "IPv4" },
                    if family == crate::IpFamily::V6 { "IPv4" } else { "IPv6" },
                ))
                .at_path(format!("{base}.ip"))
                .with_help(if family == crate::IpFamily::V6 {
                    "write `ip: v6` or `ip: dual` under `topology`, so that the network gets IPv6 addresses"
                } else {
                    "write `ip: dual` under `topology` to have both families, or leave `ip` out of the workload"
                }),
            );
        }
        if family == crate::IpFamily::V6 && !workload.udp_checksum.is_on() {
            problems.push(
                Problem::new("a stream without the UDP checksum is not possible over IPv6: the protocol requires the checksum")
                    .at_path(format!("{base}.udp-checksum"))
                    .with_help("leave `udp-checksum` out, or use `ip: v4` for a test of corruption without the checksum"),
            );
        }
        family
    }

    /// The packet size: in range for the family, and carried by every link of the path (D-081).
    fn check_packet_size(
        workload: &Generator<'_>,
        family: crate::IpFamily,
        path_links: Option<&[Name]>,
        (mtus, sets_mtu): (&[u32], bool),
        base: &str,
        problems: &mut Vec<Problem>,
    ) {
        let size = workload.packet_size.bytes();
        // Without a link that says its MTU a packet that does not fit 1500 is a mistake; with one, a larger packet is
        // the way to ask for fragmentation
        let largest = match (sets_mtu, family) {
            (true, crate::IpFamily::V4) => crate::MAX_UDP_PAYLOAD_V4,
            (true, crate::IpFamily::V6) => crate::MAX_UDP_PAYLOAD_V6,
            (false, crate::IpFamily::V4) => MAX_PACKET_SIZE,
            (false, crate::IpFamily::V6) => crate::MAX_PACKET_SIZE_V6,
        };
        if !(MIN_PACKET_SIZE..=largest).contains(&size) {
            problems.push(
                Problem::new(format!(
                    "the packet size {size} bytes is outside {MIN_PACKET_SIZE} to {largest} bytes"
                ))
                .at_path(format!("{base}.packet-size"))
                .with_help(if sets_mtu {
                    format!(
                        "the smallest payload is the header of the agent ({MIN_PACKET_SIZE} bytes); the largest is the biggest UDP datagram ({largest} bytes), which the sender cuts into fragments where it does not fit the MTU of the first link"
                    )
                } else {
                    format!(
                        "the smallest payload is the header of the agent ({MIN_PACKET_SIZE} bytes); the largest fits the MTU of 1500 without fragmentation ({largest} bytes); a link with an `mtu` allows a larger packet, which is then cut into fragments"
                    )
                }),
            );
        } else if let Some((at, why)) = crate::path_problem(size, mtus, family)
            // A sender that must not cut refuses a packet that does not fit the first link: nothing leaves, and the result
            // counts the refused packets, so there is no router to cut anything (D-081)
            .filter(|_| {
                !(workload.dont_fragment
                    && mtus
                        .first()
                        .is_some_and(|first| family.packet_length(size) > u64::from(*first)))
            })
            && let Some(links) = &path_links
        {
            problems.push(
                Problem::new(format!(
                    "a packet of {size} bytes of payload would be cut by a router: {why} ('{}')",
                    links[at]
                ))
                .at_path(format!("{base}.packet-size"))
                .with_help("a router that cuts a packet makes frames in sizes the run cannot account for; make the packet smaller, raise the MTU of that link, or put the smallest MTU first on the path (the sender cuts the packet to the MTU of the first link, and the later links carry the fragments whole)"),
            );
        } else if !workload.udp_checksum.is_on() && size < MIN_PACKET_SIZE_WITHOUT_CHECKSUM {
            problems.push(
                Problem::new(format!(
                    "the packet size {size} bytes is too small for `udp-checksum: false`: the packet then carries a CRC-32, and the smallest payload is {MIN_PACKET_SIZE_WITHOUT_CHECKSUM} bytes"
                ))
                .at_path(format!("{base}.packet-size"))
                .with_help(format!(
                    "use a payload of {MIN_PACKET_SIZE_WITHOUT_CHECKSUM} bytes or more, or leave `udp-checksum` out (the checksum of UDP is then on, and the smallest payload is {MIN_PACKET_SIZE} bytes)"
                )),
            );
        }
    }

    fn check_generator(
        &self,
        workload: &Generator<'_>,
        base: &str,
        nodes: &[Name],
        nodes_ok: bool,
        routes: Option<&Routes>,
        problems: &mut Vec<Problem>,
    ) {
        {
            for (key, node) in [("from", &workload.from), ("to", &workload.to)] {
                if nodes_ok && !nodes.contains(node) {
                    problems.push(unknown_node(node, nodes, format!("{base}.{key}")));
                }
            }
            if workload.from == workload.to {
                problems.push(
                    Problem::new(format!(
                        "a workload needs two different nodes, both are '{}'",
                        workload.from
                    ))
                    .at_path(format!("{base}.to")),
                );
            }
            if let Some(routes) = routes
                && nodes_ok
                && workload.from != workload.to
                && nodes.contains(workload.from)
                && nodes.contains(workload.to)
            {
                self.check_path(routes, workload.from, workload.to, base, problems);
            }
            let size = workload.packet_size.bytes();
            // The links of the path, when there is one: their MTUs decide how large a packet may be (D-081)
            let path_links = routes
                .filter(|_| nodes_ok && workload.from != workload.to)
                .and_then(|routes| self.topology.path(routes, workload.from, workload.to).ok())
                .map(|path| path.links);
            let mtus: Vec<u32> = path_links
                .as_ref()
                .map(|links| crate::mtus_of_topology(&self.topology, links))
                .unwrap_or_default();
            let sets_mtu = path_links.as_ref().is_some_and(|links| {
                links.iter().any(|name| {
                    self.topology
                        .links
                        .iter()
                        .any(|link| link.id == *name && link.mtu.is_some())
                })
            });
            let family = self.check_family(workload, base, problems);
            Self::check_packet_size(
                workload,
                family,
                path_links.as_deref(),
                (&mtus, sets_mtu),
                base,
                problems,
            );
            check_pattern(workload, size, base, problems);
            if !self.duration.is_zero() {
                check_window(self.duration, workload.start, workload.stop, base, problems);
            }
        }
    }
}

/// The longest burst: far more than a link holds, and far below what the 32 bit sequence number of a packet allows.
pub const MAX_BURST_PACKETS: u32 = 100_000;

/// The rules of a pattern: the rate is one that the agent was measured at, and the times make a source that sends.
fn check_pattern(workload: &Generator<'_>, size: u64, base: &str, problems: &mut Vec<Problem>) {
    let too_fast = |rate: Rate| {
        u128::from(rate.bits_per_second())
            > u128::from(MAX_PACKETS_PER_SECOND) * u128::from(size) * 8
    };
    let rate_problem = |rate: Rate, problems: &mut Vec<Problem>| {
        let per_second = u128::from(rate.bits_per_second()) / (u128::from(size) * 8);
        problems.push(
            Problem::new(format!(
                "the rate {rate} with packets of {} is {per_second} packets per second; at most {MAX_PACKETS_PER_SECOND} are supported",
                workload.packet_size
            ))
            .at_path(format!("{base}.rate"))
            .with_help("use bigger packets or a lower rate: the agent was measured up to that many packets per second"),
        );
    };
    let must_be_more_than_zero = |key: &str, time: Duration, problems: &mut Vec<Problem>| {
        if time.is_zero() {
            problems.push(
                Problem::new(format!("`{key}` must be more than zero"))
                    .at_path(format!("{base}.{key}")),
            );
        }
    };
    match workload.pattern {
        Pattern::Cbr { rate } | Pattern::Poisson { rate } => {
            if too_fast(rate) {
                rate_problem(rate, problems);
            }
        }
        Pattern::OnOff { rate, on, off, .. } => {
            if too_fast(rate) {
                rate_problem(rate, problems);
            }
            must_be_more_than_zero("on", on, problems);
            must_be_more_than_zero("off", off, problems);
        }
        Pattern::Bursty {
            packets,
            gap,
            pause,
            ..
        } => {
            if !(1..=MAX_BURST_PACKETS).contains(&packets) {
                problems.push(
                    Problem::new(format!(
                        "a burst of {packets} packets is outside 1 to {MAX_BURST_PACKETS} packets"
                    ))
                    .at_path(format!("{base}.burst-packets")),
                );
            }
            // The packets of a burst are as many a second as the agent was measured at, at most
            let shortest = Duration::from_nanos(1_000_000_000_u64.div_ceil(MAX_PACKETS_PER_SECOND));
            if packets > 1 && gap < shortest {
                problems.push(
                    Problem::new(format!(
                        "the gap {gap} between the packets of a burst is less than {shortest}, which is more than {MAX_PACKETS_PER_SECOND} packets per second"
                    ))
                    .at_path(format!("{base}.gap"))
                    .with_help("the agent was measured up to that many packets per second"),
                );
            }
            must_be_more_than_zero("pause", pause, problems);
        }
    }
}

impl Scenario {
    /// The section `measure`: a bin that can be worked with, a definition of the recovery that means something, and links
    /// that exist (D-073).
    fn check_measure(&self, problems: &mut Vec<Problem>) {
        let measure = &self.measure;
        if let Some(bin) = measure.series.as_ref().and_then(|series| series.bin) {
            let path = "measure.series.bin";
            if bin < MIN_BIN {
                problems.push(
                    Problem::new(format!("a bin of {bin} is shorter than {MIN_BIN}")).at_path(path),
                );
            } else if !self.duration.is_zero()
                && self.duration.as_nanos() / bin.as_nanos() > MAX_BINS
            {
                problems.push(
                    Problem::new(format!(
                        "a bin of {bin} makes more than {MAX_BINS} bins for a run of {}",
                        self.duration
                    ))
                    .at_path(path)
                    .with_help("use a longer bin or a shorter duration"),
                );
            }
        }
        if let Some(recovery) = &measure.recovery {
            for (key, value) in [
                ("baseline-bins", recovery.baseline_bins),
                ("window-bins", recovery.window_bins),
            ] {
                if value == Some(0) {
                    problems.push(
                        Problem::new(format!("`{key}` must be at least 1"))
                            .at_path(format!("measure.recovery.{key}")),
                    );
                }
            }
            if recovery
                .fraction
                .is_some_and(|fraction| fraction.micro_percent() == 0)
            {
                problems.push(
                    Problem::new("`fraction` must be above 0 %")
                        .at_path("measure.recovery.fraction")
                        .with_help("it is the share of the level before the event that has to be reached again, for example 90%"),
                );
            }
        }
        let ids: Vec<&str> = self
            .topology
            .links
            .iter()
            .map(|link| link.id.as_str())
            .collect();
        for (index, link) in measure.capture.iter().enumerate() {
            let path = format!("measure.capture[{index}]");
            if !ids.contains(&link.as_str()) {
                problems.push(unknown_link(link, &ids, path));
            } else if measure.capture[..index].contains(link) {
                problems.push(
                    Problem::new(format!("the link '{link}' is captured twice")).at_path(path),
                );
            }
        }
    }

    /// The events: each one inside the measured phase, on a link that exists, doing exactly one thing, and
    /// every state that the steps lead to a valid set of impairments (D-050).
    fn check_timeline(&self, problems: &mut Vec<Problem>) {
        let ids: Vec<&str> = self
            .topology
            .links
            .iter()
            .map(|link| link.id.as_str())
            .collect();
        for (index, event) in self.timeline.iter().enumerate() {
            let base = format!("timeline[{index}]");
            if !self.duration.is_zero() && event.at >= self.duration {
                problems.push(
                    Problem::new(format!(
                        "the moment {} is not before the end of the measured phase ({})",
                        event.at, self.duration
                    ))
                    .at_path(format!("{base}.at"))
                    .with_help(
                        "an event at or after the end would change nothing that is measured",
                    ),
                );
            }
            if !ids.contains(&event.link.as_str()) {
                problems.push(unknown_link(&event.link, &ids, format!("{base}.link")));
            }
            check_actions(event, &base, problems);
        }
        // The steps in time order (events at one moment in the order of the file) lead through states
        let mut order: Vec<usize> = (0..self.timeline.len()).collect();
        order.sort_by_key(|index| (self.timeline[*index].at, *index));
        for link in &self.topology.links {
            self.walk_link(link, &order, problems);
        }
    }

    /// The states of one link after each step: the impairments must stay valid, an outage must not overlap
    /// another, and a link goes down only if it is up.
    fn walk_link(&self, link: &Link, order: &[usize], problems: &mut Vec<Problem>) {
        let start = |own: &Option<Impairments>| {
            own.clone()
                .or_else(|| link.both.clone())
                .unwrap_or_default()
        };
        let mut state = LinkState {
            forward: start(&link.forward),
            reverse: start(&link.reverse),
            is_down: false,
            windows: Vec::new(),
        };
        for &index in order {
            let event = &self.timeline[index];
            if event.link != link.id {
                continue;
            }
            let base = format!("timeline[{index}]");
            if let Some(change) = &event.set {
                state.step_set(change, &base, problems);
            }
            if let Some(length) = event.outage {
                state.step_outage(event, length, self.duration, index, problems);
            }
            if event.down.is_some() {
                if state.is_down {
                    problems.push(
                        Problem::new("the link is down already")
                            .at_path(format!("{base}.down"))
                            .with_help("bring it up with `up: true` first"),
                    );
                }
                state.is_down = true;
            }
            if event.up.is_some() {
                if !state.is_down {
                    problems.push(
                        Problem::new("the link is up already")
                            .at_path(format!("{base}.up"))
                            .with_help("`up: true` follows a `down: true` of the same link"),
                    );
                }
                state.is_down = false;
            }
        }
    }
}

/// An outage so far: the direction, from, to, and the index of the event.
type Window = (Direction, Duration, Duration, usize);

/// What the timeline has made of one link at a moment.
struct LinkState {
    forward: Impairments,
    reverse: Impairments,
    is_down: bool,
    windows: Vec<Window>,
}

impl LinkState {
    /// A `set`: the impairments valid afterwards, and the step writes something.
    fn step_set(&mut self, change: &Change, base: &str, problems: &mut Vec<Problem>) {
        let mut earlier = Vec::new();
        check_impairments(&self.forward, &format!("{base}.set"), &mut earlier);
        check_impairments(&self.reverse, &format!("{base}.set"), &mut earlier);
        for (direction, change) in [
            (Direction::Forward, &change.forward),
            (Direction::Reverse, &change.reverse),
            (Direction::Both, &change.both),
        ] {
            let Some(change) = change else { continue };
            let key = direction_key(direction);
            if change.is_empty() {
                problems.push(
                    Problem::new("this step writes no key, so it changes nothing")
                        .at_path(format!("{base}.set.{key}"))
                        .with_help("write the keys that change, or take the step away"),
                );
            }
            if matches!(direction, Direction::Forward | Direction::Both) {
                self.forward = change.applied_to(&self.forward);
            }
            if matches!(direction, Direction::Reverse | Direction::Both) {
                self.reverse = change.applied_to(&self.reverse);
            }
        }
        // Only what this step breaks: a problem of the link itself is reported with the link
        let mut found = Vec::new();
        check_impairments(&self.forward, &format!("{base}.set"), &mut found);
        if self.reverse != self.forward {
            check_impairments(&self.reverse, &format!("{base}.set"), &mut found);
        }
        for problem in found {
            if !earlier.iter().any(|e| e.message == problem.message) {
                let message = format!("{} after this step", problem.message);
                problems.push(Problem { message, ..problem });
            }
        }
    }

    /// An `outage`: longer than zero, over before the end, and not overlapping another.
    fn step_outage(
        &mut self,
        event: &Event,
        length: Duration,
        duration: Duration,
        index: usize,
        problems: &mut Vec<Problem>,
    ) {
        let base = format!("timeline[{index}]");
        let direction = event.direction.unwrap_or(Direction::Both);
        let end = Duration::from_nanos(event.at.as_nanos().saturating_add(length.as_nanos()));
        if length.is_zero() {
            problems.push(
                Problem::new("an outage must last longer than zero")
                    .at_path(format!("{base}.outage")),
            );
        }
        if end > duration {
            problems.push(
                Problem::new(format!(
                    "the outage ends at {end}, after the end of the measured phase ({duration})"
                ))
                .at_path(format!("{base}.outage"))
                .with_help("make it shorter, or start it earlier"),
            );
        }
        for (other, from, to, at) in &self.windows {
            if overlaps_direction(*other, direction) && event.at < *to && *from < end {
                problems.push(
                    Problem::new(format!(
                        "the outage overlaps the outage of timeline[{at}] ({from} to {to})"
                    ))
                    .at_path(format!("{base}.at"))
                    .with_help("two outages at once have no clear end of the first; make one outage of them"),
                );
            }
        }
        self.windows.push((direction, event.at, end, index));
    }
}

/// An event does exactly one thing, and `direction` belongs to the outage.
fn check_actions(event: &Event, base: &str, problems: &mut Vec<Problem>) {
    let named: Vec<&str> = [
        ("set", event.set.is_some()),
        ("outage", event.outage.is_some()),
        ("down", event.down.is_some()),
        ("up", event.up.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect();
    if named.len() != 1 {
        let found = if named.is_empty() {
            "none".to_owned()
        } else {
            named.join(", ")
        };
        problems.push(
            Problem::new(format!(
                "an event does exactly one of `set`, `outage`, `down` and `up`, found {found}"
            ))
            .at_path(base.to_owned())
            .with_help("write one event for each thing that happens"),
        );
    }
    if event.direction.is_some() && event.outage.is_none() {
        problems.push(
            Problem::new("`direction` belongs to `outage`")
                .at_path(format!("{base}.direction"))
                .with_help("in `set` write `forward`, `reverse` or `both`; `down` and `up` act on the interface, which has both"),
        );
    }
}

fn direction_key(direction: Direction) -> &'static str {
    match direction {
        Direction::Forward => "forward",
        Direction::Reverse => "reverse",
        Direction::Both => "both",
    }
}

/// Whether two directions have a side in common.
fn overlaps_direction(first: Direction, second: Direction) -> bool {
    first == second || first == Direction::Both || second == Direction::Both
}

fn unknown_link(link: &Name, ids: &[&str], path: String) -> Problem {
    let list = format!("the links are: {}", ids.join(", "));
    let help = match closest(link.as_str(), ids) {
        Some(near) => format!("did you mean '{near}'? {list}"),
        None => list,
    };
    Problem::new(format!("the link '{link}' does not exist"))
        .at_path(path)
        .with_help(help)
}

pub(crate) fn unknown_node(node: &Name, nodes: &[Name], path: String) -> Problem {
    let known: Vec<&str> = nodes.iter().map(Name::as_str).collect();
    let list = format!("the nodes are: {}", known.join(", "));
    let help = match closest(node.as_str(), &known) {
        Some(near) => format!("did you mean '{near}'? {list}"),
        None => list,
    };
    Problem::new(format!("the node '{node}' does not exist"))
        .at_path(path)
        .with_help(help)
}

fn check_window(
    duration: Duration,
    start: Option<Duration>,
    stop: Option<Duration>,
    base: &str,
    problems: &mut Vec<Problem>,
) {
    let start_value = start.unwrap_or(Duration::from_nanos(0));
    let stop_value = stop.unwrap_or(duration);
    if stop_value > duration {
        problems.push(
            Problem::new(format!(
                "`stop` ({stop_value}) is after the end of the measured phase ({duration})"
            ))
            .at_path(format!("{base}.stop")),
        );
    }
    if start_value >= stop_value.min(duration) {
        let path = if stop.is_some() {
            format!("{base}.stop")
        } else {
            format!("{base}.start")
        };
        problems.push(
            Problem::new(format!(
                "`start` ({start_value}) must be before `stop` ({})",
                stop_value.min(duration)
            ))
            .at_path(path),
        );
    }
}

fn check_impairments(impairments: &Impairments, base: &str, problems: &mut Vec<Problem>) {
    if let Some(Loss::Markov4 { p13, p14, .. }) = &impairments.loss {
        check_sum("p13", *p13, "p14", *p14, &format!("{base}.loss"), problems);
    }
    if let Some(Loss::Markov4 { p31, p32, .. }) = &impairments.loss {
        check_sum("p31", *p31, "p32", *p32, &format!("{base}.loss"), problems);
    }
    if let Some(reorder) = &impairments.reorder {
        if reorder.gap == Some(0) {
            problems.push(
                Problem::new("the `gap` of `reorder` must be at least 1")
                    .at_path(format!("{base}.reorder.gap"))
                    .with_help("leave `gap` out for the default of 1"),
            );
        }
        if impairments.delay.is_none() {
            problems.push(
                Problem::new(
                    "`reorder` needs a `delay`: packets are reordered by holding some back",
                )
                .at_path(format!("{base}.reorder"))
                .with_help("add `delay` next to `reorder`; without it `tc` refuses the setting"),
            );
        }
    }
    let Some(jitter) = &impairments.jitter else {
        return;
    };
    let path = format!("{base}.jitter");
    let Some(delay) = impairments.delay else {
        problems.push(
            Problem::new("`jitter` needs a `delay` to vary")
                .at_path(path)
                .with_help("add `delay` next to `jitter`"),
        );
        return;
    };
    problems.extend(check_jitter(jitter, delay, &path));
}

/// Two transitions out of the same state of the four-state model are chances of one draw, so they cannot add
/// up to more than 100 %.
fn check_sum(
    first_name: &str,
    first: Probability,
    second_name: &str,
    second: Probability,
    path: &str,
    problems: &mut Vec<Problem>,
) {
    let sum = u64::from(first.micro_percent()) + u64::from(second.micro_percent());
    if sum > 100_000_000 {
        problems.push(
            Problem::new(format!(
                "`{first_name}` ({first}) and `{second_name}` ({second}) add up to more than 100%"
            ))
            .at_path(format!("{path}.{second_name}"))
            .with_help("they are the two ways out of one state, so together they are at most 100%"),
        );
    }
}

/// A delay below zero cannot exist, and `netem` would silently clamp it. The format refuses a jitter that
/// reaches below zero instead.
fn check_jitter(jitter: &Jitter, delay: Duration, path: &str) -> Option<Problem> {
    match jitter.distribution {
        JitterDistribution::Uniform if jitter.amount > delay => Some(
            Problem::new(format!(
                "the jitter ({}) is longer than the delay ({delay}): some delays would be below zero",
                jitter.amount
            ))
            .at_path(format!("{path}.amount"))
            .with_help("use a jitter that is at most the delay"),
        ),
        JitterDistribution::Normal if delay.as_nanos() / 4 < jitter.amount.as_nanos() => Some(
            Problem::new(format!(
                "with a normal distribution the delay ({delay}) must be at least four times the jitter ({}): some delays would be below zero",
                jitter.amount
            ))
            .at_path(format!("{path}.amount"))
            .with_help("the normal distribution of netem reaches about four standard deviations: use a smaller jitter or a longer delay"),
        ),
        _ => None,
    }
}
