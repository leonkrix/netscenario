//! The features of a backend that a scenario can ask for, as the rows of the coverage matrix
//! (`docs/COVERAGE.md`), and which of them a scenario needs.

use netscenario_model::{Impairments, Loss, Scenario, WorkloadKind};

/// Something a backend reproduces faithfully, in part, or not at all. Each feature is a row of the coverage
/// matrix; [`Feature::label`] is the text of that row, and a test keeps the two in step.
///
/// The list grows with the format: a release adds the features that its scenarios can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Feature {
    /// Two nodes and one link between them.
    PointToPoint,
    /// IPv4 addresses on the links.
    Ipv4,
    /// `forward` and `reverse` with their own values.
    DifferentValuesPerDirection,
    /// A constant delay.
    FixedDelay,
    /// A delay that varies (uniform or normal).
    Jitter,
    /// Each packet is lost with the same probability.
    IndependentLoss,
    /// Losses in bursts: the Gilbert-Elliott model and the four-state model.
    BurstLoss,
    /// Packets that arrive after later ones (`reorder`).
    Reordering,
    /// Packets that arrive twice (`duplicate`).
    Duplication,
    /// Packets with a flipped bit (`corrupt`).
    Corruption,
    /// A limit on the rate.
    RateLimit,
    /// Values of a link that change at moments of the run (`timeline` with `set`).
    ScheduledSteps,
    /// Windows in which nothing is delivered, not even a packet that is on its way (`timeline` with `outage`, D-064).
    Outages,
    /// The interface of a link goes down and comes up (`timeline` with `down` and `up`).
    LinkDownUp,
    /// The workloads that send UDP packets: `udp-cbr`, and with `TrafficPatterns` the three others.
    UdpGenerators,
    /// A `udp-cbr` workload without the checksum of UDP (`udp-checksum: false`), so that a corrupted packet
    /// arrives and is counted.
    UdpWithoutChecksum,
    /// A program of your own as a node (`kind: command`).
    Program,
    /// A `seed` is written in the file and the scenario has loss: the same seed must give the same lost packets.
    SeededLoss,
    /// The workloads `udp-poisson`, `udp-onoff` and `udp-bursty`: the moments of the packets come from a seeded generator.
    /// Last in the list because it was added last.
    TrafficPatterns,
    /// The capture of the interfaces of a link as pcap files (`measure.capture`, PRB-09). Last in the list because it was added
    /// last.
    PacketCapture,
    /// A network of more than two nodes or more than one link: routers, several senders and receivers, several paths
    /// (D-078). Last in the list because it was added last.
    MultipleNodes,
    /// Routes the scenario takes a hand in: a node that is a router, listed routes, manual routing, a link `cost` or
    /// `ecmp` (D-078). Last in the list because it was added last.
    StaticRoutes,
    /// The `mtu` of a link, and the fragments a datagram that does not fit the first link makes (D-081). Last in the list
    /// because it was added last.
    LinkMtu,
    /// IPv6 addresses on the links and the nodes (`topology.ip`, `ip` on a workload, D-081). Last in the list because it was
    /// added last.
    Ipv6,
    /// The `queue` of a direction with a `fifo` and a limit (LNK-09, D-082). Last in the list because it was added last.
    QueueLimit,
    /// The `queue` of a direction with `CoDel`, `FQ-CoDel` or `PIE` (LNK-09, D-082). Last in the list because it was added last.
    QueueDisciplines,
    /// The workload `trace-replay`: packets at the moments and with the sizes of a recorded trace (WRK-03, D-085). Last in the
    /// list because it was added last.
    TraceReplay,
    /// The workloads `tcp-bulk` and `request-response`: data and requests over the TCP of the kernel (WRK-04, WRK-05, D-086).
    /// Last in the list because it was added last.
    TcpWorkloads,
    /// The key `echo` of the UDP workloads: the far node sends every packet back and the result has the round trip (PRB-12, D-087).
    /// Last in the list because it was added last.
    UdpEcho,
}

impl Feature {
    /// Every feature, in the order of the coverage matrix.
    pub const ALL: &'static [Feature] = &[
        Feature::PointToPoint,
        Feature::Ipv4,
        Feature::DifferentValuesPerDirection,
        Feature::FixedDelay,
        Feature::Jitter,
        Feature::IndependentLoss,
        Feature::BurstLoss,
        Feature::Reordering,
        Feature::Duplication,
        Feature::Corruption,
        Feature::RateLimit,
        Feature::ScheduledSteps,
        Feature::Outages,
        Feature::LinkDownUp,
        Feature::UdpGenerators,
        Feature::UdpWithoutChecksum,
        Feature::Program,
        Feature::SeededLoss,
        Feature::TrafficPatterns,
        Feature::PacketCapture,
        Feature::MultipleNodes,
        Feature::StaticRoutes,
        Feature::LinkMtu,
        Feature::Ipv6,
        Feature::QueueLimit,
        Feature::QueueDisciplines,
        Feature::TraceReplay,
        Feature::TcpWorkloads,
        Feature::UdpEcho,
    ];

    /// A short stable name, for results and logs.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Feature::PointToPoint => "point-to-point",
            Feature::Ipv4 => "ipv4",
            Feature::DifferentValuesPerDirection => "different-values-per-direction",
            Feature::FixedDelay => "fixed-delay",
            Feature::Jitter => "jitter",
            Feature::IndependentLoss => "independent-loss",
            Feature::BurstLoss => "burst-loss",
            Feature::Reordering => "reordering",
            Feature::Duplication => "duplication",
            Feature::Corruption => "corruption",
            Feature::RateLimit => "rate-limit",
            Feature::ScheduledSteps => "scheduled-steps",
            Feature::Outages => "outages",
            Feature::LinkDownUp => "link-down-up",
            Feature::UdpGenerators => "udp-generators",
            Feature::UdpWithoutChecksum => "udp-without-checksum",
            Feature::Program => "program",
            Feature::SeededLoss => "seeded-loss",
            Feature::TrafficPatterns => "traffic-patterns",
            Feature::PacketCapture => "packet-capture",
            Feature::MultipleNodes => "multiple-nodes",
            Feature::StaticRoutes => "static-routes",
            Feature::LinkMtu => "link-mtu",
            Feature::Ipv6 => "ipv6",
            Feature::QueueLimit => "queue-limit",
            Feature::QueueDisciplines => "queue-disciplines",
            Feature::TraceReplay => "trace-replay",
            Feature::TcpWorkloads => "tcp-workloads",
            Feature::UdpEcho => "udp-echo",
        }
    }

    /// The text of the row in `docs/COVERAGE.md`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Feature::PointToPoint => "Point to point",
            Feature::Ipv4 => "IPv4",
            Feature::DifferentValuesPerDirection => "Different values per direction",
            Feature::FixedDelay => "Fixed delay",
            Feature::Jitter => "Jitter (uniform, normal, pareto, paretonormal, table)",
            Feature::IndependentLoss => "Independent loss",
            Feature::BurstLoss => "Burst loss (Gilbert-Elliott, four-state)",
            Feature::Reordering => "Reordering",
            Feature::Duplication => "Duplication",
            Feature::Corruption => "Corruption",
            Feature::RateLimit => "Rate limit",
            Feature::ScheduledSteps => "Scheduled steps",
            Feature::Outages => "Outages",
            Feature::LinkDownUp => "Link down and up",
            Feature::UdpGenerators => "UDP traffic generators",
            Feature::UdpWithoutChecksum => "UDP without checksum",
            Feature::Program => "Your program as a node",
            Feature::SeededLoss => "Same seed, same loss pattern",
            Feature::TrafficPatterns => "UDP traffic patterns (Poisson, on/off, bursts)",
            Feature::PacketCapture => "Packet capture",
            Feature::MultipleNodes => "Several nodes and links",
            Feature::StaticRoutes => "Routers and routes",
            Feature::LinkMtu => "MTU, fragmentation",
            Feature::Ipv6 => "IPv6",
            Feature::QueueLimit => "Queue limit",
            Feature::QueueDisciplines => "Queue disciplines (CoDel, FQ-CoDel, PIE)",
            Feature::TraceReplay => "Replay of a trace of packets (times and sizes)",
            Feature::TcpWorkloads => "TCP workloads (bulk transfers, requests and responses)",
            Feature::UdpEcho => "Echo of a UDP stream (the round trip of the datagrams)",
        }
    }
}

/// A feature that a scenario needs, and the first place in the scenario where it is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The feature that is needed.
    pub feature: Feature,
    /// The path in the scenario, for example `topology.links[0].forward.jitter`.
    pub path: String,
}

/// The features that a set of impairments needs, at `path`.
fn needs_of(impairments: &Impairments, path: &str, need: &mut dyn FnMut(Feature, String)) {
    if impairments.delay.is_some() {
        need(Feature::FixedDelay, format!("{path}.delay"));
    }
    if impairments.jitter.is_some() {
        need(Feature::Jitter, format!("{path}.jitter"));
    }
    match &impairments.loss {
        None => {}
        Some(Loss::Random { .. }) => need(Feature::IndependentLoss, format!("{path}.loss")),
        Some(Loss::GilbertElliott { .. } | Loss::Markov4 { .. }) => {
            need(Feature::BurstLoss, format!("{path}.loss"));
        }
    }
    if impairments.reorder.is_some() {
        need(Feature::Reordering, format!("{path}.reorder"));
    }
    if impairments.duplicate.is_some() {
        need(Feature::Duplication, format!("{path}.duplicate"));
    }
    if impairments.corrupt.is_some() {
        need(Feature::Corruption, format!("{path}.corrupt"));
    }
    if impairments.rate.is_some() {
        need(Feature::RateLimit, format!("{path}.rate"));
    }
    if let Some(queue) = &impairments.queue {
        let feature = if queue.discipline == netscenario_model::Discipline::Fifo {
            Feature::QueueLimit
        } else {
            Feature::QueueDisciplines
        };
        need(feature, format!("{path}.queue"));
    }
}

/// The features that `scenario` needs, each once, in the order of their first use.
#[must_use]
// A list of the things a scenario can ask for, one check after the other
#[allow(clippy::too_many_lines)]
pub fn requirements(scenario: &Scenario) -> Vec<Requirement> {
    let mut found = Vec::new();
    let mut need = |feature: Feature, path: String| {
        if !found.iter().any(|r: &Requirement| r.feature == feature) {
            found.push(Requirement { feature, path });
        }
    };
    if scenario.topology.nodes.len() == 2 && scenario.topology.links.len() == 1 {
        need(Feature::PointToPoint, "topology".to_owned());
    } else {
        need(Feature::MultipleNodes, "topology".to_owned());
    }
    let topology = &scenario.topology;
    if let Some(index) = topology
        .nodes
        .iter()
        .position(|node| node.role == netscenario_model::Role::Router)
    {
        need(Feature::StaticRoutes, format!("topology.nodes[{index}]"));
    } else if !topology.routes.is_empty() {
        need(Feature::StaticRoutes, "topology.routes".to_owned());
    } else if topology.routing == netscenario_model::Routing::Manual {
        need(Feature::StaticRoutes, "topology.routing".to_owned());
    } else if topology.ecmp {
        need(Feature::StaticRoutes, "topology.ecmp".to_owned());
    } else if let Some(index) = topology.links.iter().position(|link| link.cost.is_some()) {
        need(
            Feature::StaticRoutes,
            format!("topology.links[{index}].cost"),
        );
    }
    if scenario.topology.ip != netscenario_model::IpMode::V4 {
        need(Feature::Ipv6, "topology.ip".to_owned());
    }
    if scenario.topology.ip != netscenario_model::IpMode::V6 {
        need(Feature::Ipv4, "topology".to_owned());
    }
    for (index, link) in scenario.topology.links.iter().enumerate() {
        let base = format!("topology.links[{index}]");
        if link.mtu.is_some() {
            need(Feature::LinkMtu, format!("{base}.mtu"));
        }
        for (name, impairments) in [
            ("forward", &link.forward),
            ("reverse", &link.reverse),
            ("both", &link.both),
        ] {
            let Some(impairments) = impairments else {
                continue;
            };
            let path = format!("{base}.{name}");
            if name != "both" {
                need(Feature::DifferentValuesPerDirection, path.clone());
            }
            needs_of(impairments, &path, &mut need);
        }
    }
    // What a `set` of the timeline leads to is asked for as well: a link that has nothing at the start and gets
    // a reordering at 5 s needs a backend that reorders. An outage changes no value and needs `Outages`.
    for step in crate::timeline::steps(scenario) {
        if let crate::timeline::Action::Impair { impairments, .. } = &step.action {
            needs_of(
                impairments,
                &format!("timeline[{}].set", step.event),
                &mut need,
            );
        }
    }
    for (index, event) in scenario.timeline.iter().enumerate() {
        let path = format!("timeline[{index}]");
        if event.set.is_some() {
            need(Feature::ScheduledSteps, path.clone());
        }
        if event.outage.is_some() {
            need(Feature::Outages, path.clone());
        }
        if event.down.is_some() || event.up.is_some() {
            need(Feature::LinkDownUp, path);
        }
    }
    for (index, workload) in scenario.workloads.iter().enumerate() {
        match workload.kind() {
            WorkloadKind::UdpCbr => need(Feature::UdpGenerators, format!("workloads[{index}]")),
            WorkloadKind::Command => need(Feature::Program, format!("workloads[{index}]")),
            // A kind of a later version is not asked for here, and the model refuses it before this is reached
            WorkloadKind::UdpPoisson | WorkloadKind::UdpOnoff | WorkloadKind::UdpBursty => {
                need(Feature::UdpGenerators, format!("workloads[{index}]"));
                need(Feature::TrafficPatterns, format!("workloads[{index}]"));
            }
            WorkloadKind::TraceReplay => {
                need(Feature::UdpGenerators, format!("workloads[{index}]"));
                need(Feature::TraceReplay, format!("workloads[{index}]"));
            }
            WorkloadKind::TcpBulk | WorkloadKind::RequestResponse => {
                need(Feature::TcpWorkloads, format!("workloads[{index}]"));
                let ip = workload
                    .as_tcp_bulk()
                    .map(|bulk| bulk.ip)
                    .or_else(|| workload.as_request_response().map(|exchange| exchange.ip));
                if ip == Some(Some(netscenario_model::IpFamily::V6)) {
                    need(Feature::Ipv6, format!("workloads[{index}].ip"));
                }
            }
            _ => {}
        }
        if let Some(generator) = workload.generator() {
            if generator.dont_fragment {
                need(
                    Feature::LinkMtu,
                    format!("workloads[{index}].dont-fragment"),
                );
            }
            if generator.ip == Some(netscenario_model::IpFamily::V6) {
                need(Feature::Ipv6, format!("workloads[{index}].ip"));
            }
            if generator.echo {
                need(Feature::UdpEcho, format!("workloads[{index}].echo"));
            }
        }
        if workload
            .generator()
            .is_some_and(|generator| !generator.udp_checksum.is_on())
        {
            need(
                Feature::UdpWithoutChecksum,
                format!("workloads[{index}].udp-checksum"),
            );
        }
    }
    if !scenario.measure.capture.is_empty() {
        need(Feature::PacketCapture, "measure.capture".to_owned());
    }
    if crate::timeline::draws_random_numbers(scenario) && scenario.seed.is_some() {
        need(Feature::SeededLoss, "seed".to_owned());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(yaml: &str) -> Scenario {
        Scenario::from_yaml(yaml, "test.yaml").expect("a valid scenario")
    }

    const HEAD: &str = "version: 0\nname: t\nduration: 10s\n";
    const TAIL: &str = "workloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

    fn with_link(link: &str, extra_head: &str) -> Scenario {
        scenario(&format!(
            "{HEAD}{extra_head}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}{TAIL}"
        ))
    }

    fn features(scenario: &Scenario) -> Vec<Feature> {
        requirements(scenario).iter().map(|r| r.feature).collect()
    }

    /// A `queue` asks for the features of its discipline: a `fifo` with a limit for `QueueLimit`, the others for
    /// `QueueDisciplines`; a link without a queue asks for neither (D-082).
    #[test]
    fn a_queue_asks_for_the_feature_of_its_discipline() {
        let with = |queue: &str| {
            features(&scenario(&format!(
                "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: {{ rate: 1mbit{queue} }}\n{TAIL}"
            )))
        };
        assert!(with(", queue: { limit: 10p }").contains(&Feature::QueueLimit));
        assert!(!with(", queue: { limit: 10p }").contains(&Feature::QueueDisciplines));
        assert!(with(", queue: { discipline: pie }").contains(&Feature::QueueDisciplines));
        assert!(!with("").contains(&Feature::QueueLimit));
        assert!(!with("").contains(&Feature::QueueDisciplines));
    }

    /// A network of more than two nodes needs `MultipleNodes` and not `PointToPoint`; routers, listed routes, manual routing, a
    /// cost and `ecmp` each need `StaticRoutes`, and a plain network needs none of them (so a backend without routing still
    /// runs the scenarios of the earlier releases).
    #[test]
    fn a_network_of_several_nodes_and_its_routes_ask_for_their_features() {
        let network = |topology: &str| {
            scenario(&format!(
                "{HEAD}topology:\n{topology}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: c, rate: 1mbit, packet-size: 1200b }}\n"
            ))
        };
        let chain = "  nodes: [a, { id: b, role: router }, c]\n  links:\n    - { id: ab, between: [a, b] }\n    - { id: bc, between: [b, c] }\n";
        let found = features(&network(chain));
        assert!(found.contains(&Feature::MultipleNodes) && !found.contains(&Feature::PointToPoint));
        assert!(found.contains(&Feature::StaticRoutes));
        let at = requirements(&network(chain))
            .into_iter()
            .find(|r| r.feature == Feature::StaticRoutes)
            .unwrap();
        assert_eq!(at.path, "topology.nodes[1]");
        // Hosts only, computed routes: several nodes, but no routing of its own to ask for. (A host in the middle does not
        // forward, so this needs a path that does not pass one: two links from one host.)
        let star = "  nodes: [a, b, c]\n  links:\n    - { id: ac, between: [a, c] }\n    - { id: bc, between: [b, c] }\n";
        let found = features(&network(star));
        assert!(found.contains(&Feature::MultipleNodes) && !found.contains(&Feature::StaticRoutes));
        for (extra, path) in [
            (
                "  routing: manual\n  routes:\n    - { node: a, to: c, via: ac }\n",
                "topology.routes",
            ),
            ("  ecmp: true\n", "topology.ecmp"),
        ] {
            let found = requirements(&network(&format!("{star}{extra}")));
            let asked = found.iter().find(|r| r.feature == Feature::StaticRoutes);
            assert_eq!(asked.map(|r| r.path.as_str()), Some(path), "{extra}");
        }
        // Canary: a plain link is a point to point network and asks for neither
        let plain = features(&with_link("", ""));
        assert!(
            !plain.contains(&Feature::MultipleNodes) && !plain.contains(&Feature::StaticRoutes)
        );
    }

    #[test]
    fn a_plain_link_needs_only_the_basics() {
        let s = with_link("", "");
        assert_eq!(
            features(&s),
            [Feature::PointToPoint, Feature::Ipv4, Feature::UdpGenerators]
        );
    }

    /// The three kinds of WRK-02 need the generators and, on top of them, the patterns; a constant bit rate does not need
    /// the patterns, so a backend without them still runs the scenarios of release 0.1.
    #[test]
    fn the_traffic_patterns_need_their_feature_and_a_constant_bit_rate_does_not() {
        let scenario = |workload: &str| {
            Scenario::from_yaml(
                &format!("version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - {workload}\n"),
                "t.yaml",
            )
            .unwrap()
        };
        for (workload, patterns) in [
            (
                "{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }",
                false,
            ),
            (
                "{ id: w, kind: udp-poisson, from: a, to: b, rate: 1mbit, packet-size: 1200b }",
                true,
            ),
            (
                "{ id: w, kind: udp-onoff, from: a, to: b, rate: 1mbit, packet-size: 1200b, on: 1s, off: 1s }",
                true,
            ),
            (
                "{ id: w, kind: udp-bursty, from: a, to: b, packet-size: 1200b, burst-packets: 4, gap: 1ms, pause: 10ms }",
                true,
            ),
        ] {
            let found = features(&scenario(workload));
            assert!(found.contains(&Feature::UdpGenerators), "{workload}");
            assert_eq!(
                found.contains(&Feature::TrafficPatterns),
                patterns,
                "{workload}"
            );
        }
        // The checksum is asked for by the new kinds too
        let found = requirements(&scenario(
            "{ id: w, kind: udp-poisson, from: a, to: b, rate: 1mbit, packet-size: 1200b, udp-checksum: false }",
        ));
        assert!(
            found
                .iter()
                .any(|r| r.feature == Feature::UdpWithoutChecksum
                    && r.path == "workloads[0].udp-checksum")
        );
    }

    #[test]
    fn every_impairment_names_its_feature_and_its_place() {
        let s = with_link(
            "      both: { delay: 20ms, jitter: { amount: 5ms }, loss: { model: random, p: 1% }, rate: 2mbit }\n",
            "",
        );
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(
            at(Feature::FixedDelay),
            Some("topology.links[0].both.delay")
        );
        assert_eq!(at(Feature::Jitter), Some("topology.links[0].both.jitter"));
        assert_eq!(
            at(Feature::IndependentLoss),
            Some("topology.links[0].both.loss")
        );
        assert_eq!(at(Feature::RateLimit), Some("topology.links[0].both.rate"));
        assert_eq!(at(Feature::DifferentValuesPerDirection), None);
        assert_eq!(at(Feature::SeededLoss), None);
    }

    #[test]
    fn the_impairments_of_release_0_2_name_their_features_and_their_places() {
        let s = with_link(
            "      forward: { delay: 20ms, loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }, reorder: { share: 25%, gap: 5 }, duplicate: 1%, corrupt: 0.5% }\n      reverse: { loss: { model: markov4, p13: 2%, p31: 30%, p32: 10%, p23: 20%, p14: 0.5% } }\n",
            "",
        );
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        // the first place that asks for the feature
        assert_eq!(
            at(Feature::BurstLoss),
            Some("topology.links[0].forward.loss")
        );
        assert_eq!(
            at(Feature::Reordering),
            Some("topology.links[0].forward.reorder")
        );
        assert_eq!(
            at(Feature::Duplication),
            Some("topology.links[0].forward.duplicate")
        );
        assert_eq!(
            at(Feature::Corruption),
            Some("topology.links[0].forward.corrupt")
        );
        assert_eq!(at(Feature::IndependentLoss), None);
        // each feature once
        assert_eq!(
            found
                .iter()
                .filter(|r| r.feature == Feature::BurstLoss)
                .count(),
            1
        );
    }

    #[test]
    fn the_events_of_the_timeline_name_their_features_and_their_places() {
        let s = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\ntimeline:\n  - {{ at: 1s, link: l, outage: 1s }}\n  - {{ at: 3s, link: l, set: {{ both: {{ rate: 2mbit }} }} }}\n  - {{ at: 5s, link: l, down: true }}\n  - {{ at: 6s, link: l, up: true }}\n{TAIL}"
        ));
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(at(Feature::Outages), Some("timeline[0]"));
        assert_eq!(at(Feature::ScheduledSteps), Some("timeline[1]"));
        // the first event that asks for it, down or up
        assert_eq!(at(Feature::LinkDownUp), Some("timeline[2]"));
    }

    /// A stream without the checksum of UDP needs a backend that can send one; a plain stream does not ask.
    #[test]
    fn a_workload_without_the_checksum_names_its_feature_and_its_place() {
        let without = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b, udp-checksum: false }}\n"
        ));
        let found = requirements(&without);
        let place = found
            .iter()
            .find(|r| r.feature == Feature::UdpWithoutChecksum)
            .map(|r| r.path.as_str());
        assert_eq!(place, Some("workloads[0].udp-checksum"));
        // Canary: the plain workload, and one that writes the default, do not ask for it
        assert!(!features(&with_link("", "")).contains(&Feature::UdpWithoutChecksum));
        let explicit = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b, udp-checksum: true }}\n"
        ));
        assert!(!features(&explicit).contains(&Feature::UdpWithoutChecksum));
    }

    /// What a `set` leads to is asked for, with the path of the step: a link with nothing at the start that gets a
    /// reordering and a rate at 5 s needs a backend that does both. An outage needs `Outages` and no loss or other value.
    #[test]
    fn what_a_step_leads_to_is_a_requirement_with_the_path_of_the_step() {
        let s = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{TAIL}timeline:\n  - {{ at: 5s, link: l, set: {{ forward: {{ delay: 10ms, reorder: {{ share: 25% }}, rate: 2mbit }} }} }}\n  - {{ at: 7s, link: l, outage: 1s }}\n"
        ));
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(at(Feature::Reordering), Some("timeline[0].set.reorder"));
        assert_eq!(at(Feature::RateLimit), Some("timeline[0].set.rate"));
        assert_eq!(at(Feature::Outages), Some("timeline[1]"));
        // Canary: an outage asks for no loss
        assert_eq!(at(Feature::IndependentLoss), None);
    }

    /// A seed matters for a loss that a step switches on, and not for an outage alone.
    #[test]
    fn a_loss_that_comes_with_a_step_needs_the_seed_and_an_outage_does_not() {
        let with = |timeline: &str| {
            scenario(&format!(
                "{HEAD}seed: 3\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{TAIL}timeline:\n{timeline}"
            ))
        };
        let lossy =
            with("  - { at: 5s, link: l, set: { forward: { loss: { model: random, p: 5% } } } }\n");
        assert!(features(&lossy).contains(&Feature::SeededLoss));
        let outage = with("  - { at: 5s, link: l, outage: 1s }\n");
        assert!(!features(&outage).contains(&Feature::SeededLoss));
    }

    #[test]
    fn forward_or_reverse_needs_values_per_direction() {
        let s = with_link("      reverse: { delay: 20ms }\n", "");
        let found = requirements(&s);
        let r = found
            .iter()
            .find(|r| r.feature == Feature::DifferentValuesPerDirection)
            .expect("the feature is needed");
        assert_eq!(r.path, "topology.links[0].reverse");
    }

    #[test]
    fn a_seed_matters_only_with_loss() {
        let lossy = with_link(
            "      both: { loss: { model: random, p: 1% } }\n",
            "seed: 7\n",
        );
        assert!(features(&lossy).contains(&Feature::SeededLoss));
        let without_loss = with_link("      both: { delay: 5ms }\n", "seed: 7\n");
        assert!(!features(&without_loss).contains(&Feature::SeededLoss));
        let without_seed = with_link("      both: { loss: { model: random, p: 1% } }\n", "");
        assert!(!features(&without_seed).contains(&Feature::SeededLoss));
    }

    #[test]
    fn ids_and_labels_are_unique() {
        for (i, a) in Feature::ALL.iter().enumerate() {
            for b in &Feature::ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.label(), b.label());
            }
        }
    }
}
