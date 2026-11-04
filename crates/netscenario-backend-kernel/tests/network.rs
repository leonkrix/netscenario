//! Networks of several nodes (TOP-02 to TOP-06, D-078): flows over several hops, routers that forward, routes that decide the
//! path, a multipath that shares flows, and a failover when a link goes down. The links have no impairment, so these run on a
//! machine without `netem` as well.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod support;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use netscenario_core::{
    Backend, Direction, Error, LinkHandle, NodeHandle, ProgramHandle, RunOptions, RunResult, run,
};
use netscenario_model::{Impairments, Name, Scenario};
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn options(dir: &Path) -> RunOptions {
    RunOptions::new(agent_exe().unwrap(), dir)
}

fn scenario(topology: &str, tail: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: network-test\nduration: 2s\ntopology:\n{topology}workloads:\n{tail}"
        ),
        "network-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

/// 1 mbit with 1000-byte payloads is 125 packets a second: 250 in two seconds.
const FLOW: &str =
    "  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1000b }\n";

const CHAIN: &str = "  nodes: [client, { id: a, role: router }, { id: b, role: router }, { id: c, role: router }, server]\n  links:\n    - { id: l1, between: [client, a] }\n    - { id: l2, between: [a, b] }\n    - { id: l3, between: [b, c] }\n    - { id: l4, between: [c, server] }\n";

fn only_workload(result: &RunResult) -> &netscenario_core::WorkloadResult {
    &result.workloads[0]
}

#[test]
fn a_flow_over_four_hops_arrives_and_the_result_names_the_path_the_routes_and_the_addresses() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-chain");
    let mut backend = backend(|_| {});
    let result = run(&mut backend, &scenario(CHAIN, FLOW), &options(&dir)).unwrap();
    let media = only_workload(&result);
    assert_eq!(
        (media.stats.sent, media.stats.received, media.stats.lost),
        (250, 250, 0)
    );
    assert_eq!(media.sender.send_errors, 0);
    let path = media.path.as_ref().unwrap();
    assert_eq!(path.nodes, ["client", "a", "b", "c", "server"]);
    assert_eq!(path.links, ["l1", "l2", "l3", "l4"]);
    assert!(!path.shared);
    // The packets went to the address of the node on its loopback, the fifth of the file
    assert_eq!(media.to_address, "10.200.0.5:5000");
    let roles: Vec<(&str, &str, &str)> = result
        .nodes
        .iter()
        .map(|n| (n.node.as_str(), n.role.as_str(), n.address.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("client", "host", "10.200.0.1"),
            ("a", "router", "10.200.0.2"),
            ("b", "router", "10.200.0.3"),
            ("c", "router", "10.200.0.4"),
            ("server", "host", "10.200.0.5"),
        ]
    );
    // A route from each of the five nodes to each of the other four
    assert_eq!(result.routes.len(), 20);
    assert!(result.routes.iter().all(|route| !route.given));
    // A delay that adds up per hop is for the fidelity tests; here the packets cross four links in well under a second
    let delay = media.stats.delay.as_ref().unwrap();
    assert!(delay.max_ns < 500_000_000, "{} ns", delay.max_ns);
    assert_nothing_left(backend.run_id(), "after the run");
    let _ = std::fs::remove_dir_all(dir);
}

/// A backend that does everything as the Kernel backend does except that no node forwards: the canary for the test
/// above, which would also pass if the packets reached the server some other way.
struct NoForwarding(netscenario_backend_kernel::KernelBackend);

impl Backend for NoForwarding {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn capabilities(&self) -> netscenario_core::Capabilities {
        self.0.capabilities()
    }
    fn create_node(&mut self, name: &Name) -> Result<NodeHandle, Error> {
        self.0.create_node(name)
    }
    fn create_link(&mut self, name: &Name, between: [NodeHandle; 2]) -> Result<LinkHandle, Error> {
        self.0.create_link(name, between)
    }
    fn set_node_address(
        &mut self,
        node: NodeHandle,
        address: std::net::Ipv4Addr,
    ) -> Result<(), Error> {
        self.0.set_node_address(node, address)
    }
    fn set_forwarding(&mut self, node: NodeHandle, _on: bool) -> Result<(), Error> {
        // No node forwards, whatever the machine has set: the routers are told to stop, as are the hosts
        self.0.set_forwarding(node, false)
    }
    fn add_route(
        &mut self,
        node: NodeHandle,
        route: &netscenario_core::NodeRoute,
    ) -> Result<(), Error> {
        self.0.add_route(node, route)
    }
    fn apply(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), Error> {
        self.0.apply(link, direction, impairments, seed)
    }
    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &netscenario_core::Program,
    ) -> Result<ProgramHandle, Error> {
        self.0.start_program(node, program)
    }
    fn address(&self, link: LinkHandle, end: usize) -> Result<std::net::Ipv4Addr, Error> {
        self.0.address(link, end)
    }
    fn describe(&self) -> netscenario_core::BackendInfo {
        self.0.describe()
    }
    fn poll_program(
        &mut self,
        program: ProgramHandle,
        wait: Duration,
    ) -> Result<netscenario_core::ProgramState, Error> {
        self.0.poll_program(program, wait)
    }
    fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error> {
        self.0.send_line(program, line)
    }
    fn teardown(&mut self) -> Result<(), Error> {
        self.0.teardown()
    }
}

#[test]
fn without_forwarding_in_the_routers_nothing_arrives() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-no-forwarding");
    let mut backend = NoForwarding(backend(|_| {}));
    let result = run(&mut backend, &scenario(CHAIN, FLOW), &options(&dir)).unwrap();
    let media = only_workload(&result);
    assert_eq!(media.stats.sent, 250);
    assert_eq!(
        media.stats.received, 0,
        "a host in the middle does not forward"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_star_carries_flows_in_both_directions_through_its_hub() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-star");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &scenario(
            "  star:\n    hub: hub\n    leaves: [client, server, third]\n",
            "  - { id: first, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1000b }\n  - { id: second, kind: udp-cbr, from: server, to: third, rate: 500kbit, packet-size: 1000b }\n  - { id: third, kind: udp-cbr, from: third, to: client, rate: 250kbit, packet-size: 1000b }\n",
        ),
        &options(&dir),
    )
    .unwrap();
    let counts: Vec<(u64, u64)> = result
        .workloads
        .iter()
        .map(|w| (w.stats.sent, w.stats.received))
        .collect();
    assert_eq!(counts, [(250, 250), (125, 125), (63, 63)]);
    for workload in &result.workloads {
        assert_eq!(workload.path.as_ref().unwrap().links.len(), 2);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Two paths, a route that prefers the second one by a smaller metric: the packets take it, and none take the other. The
/// capture counts the frames of the first link of each path.
#[test]
fn the_metric_of_a_listed_route_decides_the_path() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-metric");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &with_capture(
            &scenario(
                &two_paths("  routes:\n    - { node: client, to: server, via: slow-in, metric: 10 }\n    - { node: client, to: server, via: fast-in, metric: 20 }\n"),
                FLOW,
            ),
            &["fast-in", "slow-in"],
        ),
        &options(&dir),
    )
    .unwrap();
    let media = only_workload(&result);
    assert_eq!(media.stats.received, 250);
    assert_eq!(media.path.as_ref().unwrap().links, ["slow-in", "slow-out"]);
    assert_eq!(frames(&result, "slow-in", "client"), 250);
    assert_eq!(frames(&result, "fast-in", "client"), 0);
    let _ = std::fs::remove_dir_all(dir);
}

fn two_paths(extra: &str) -> String {
    format!(
        "  nodes: [client, {{ id: fast, role: router }}, {{ id: slow, role: router }}, server]\n  links:\n    - {{ id: fast-in, between: [client, fast] }}\n    - {{ id: fast-out, between: [fast, server] }}\n    - {{ id: slow-in, between: [client, slow] }}\n    - {{ id: slow-out, between: [slow, server] }}\n{extra}"
    )
}

/// The frames that the capture of the interface of `node` on `link` saw.
fn frames(result: &RunResult, link: &str, node: &str) -> u64 {
    result
        .captures
        .iter()
        .find(|capture| capture.link == link && capture.node == node)
        .unwrap_or_else(|| panic!("no capture of {link} at {node}: {:?}", result.captures))
        .packets
}

/// Canary for the test above: without the listed route the shortest path is taken, and with equal costs the first link of
/// the file, so the packets take the fast one.
#[test]
fn without_a_listed_route_the_first_of_two_equal_paths_is_taken() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-first");
    let mut backend = backend(|_| {});
    let result = run(
        &mut backend,
        &with_capture(&scenario(&two_paths(""), FLOW), &["fast-in", "slow-in"]),
        &options(&dir),
    )
    .unwrap();
    assert_eq!(
        only_workload(&result).path.as_ref().unwrap().links,
        ["fast-in", "fast-out"]
    );
    assert_eq!(frames(&result, "fast-in", "client"), 250);
    assert_eq!(frames(&result, "slow-in", "client"), 0);
    let _ = std::fs::remove_dir_all(dir);
}

fn with_capture(scenario: &Scenario, links: &[&str]) -> Scenario {
    let mut scenario = scenario.clone();
    scenario.measure.capture = links.iter().map(|l| Name::new(l).unwrap()).collect();
    scenario
}

/// With `ecmp` the flows between the same two nodes share the two equally short paths (the kernel looks at the ports too, which
/// the backend asks for): 16 workloads of 50 packets, every flow whole on one path, and each path has some. The chance that
/// all 16 take the same path by luck is 2 x 2^-16, below 4e-5.
#[test]
fn with_ecmp_flows_between_the_same_two_nodes_share_the_paths() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-ecmp");
    let mut backend = backend(|_| {});
    let mut workloads = String::new();
    for n in 0..16 {
        writeln!(
            workloads,
            "  - {{ id: w{n}, kind: udp-cbr, from: client, to: server, rate: 200kbit, packet-size: 1000b }}"
        )
        .unwrap();
    }
    let result = run(
        &mut backend,
        &with_capture(
            &scenario(&two_paths("  ecmp: true\n"), &workloads),
            &["fast-in", "slow-in"],
        ),
        &options(&dir),
    )
    .unwrap();
    let (fast, slow) = (
        frames(&result, "fast-in", "client"),
        frames(&result, "slow-in", "client"),
    );
    // 200 kbit with 1000-byte payloads is 25 packets a second: 50 in two seconds, 800 in all
    assert_eq!(fast + slow, 800, "{fast} and {slow}");
    assert!(
        fast % 50 == 0 && slow % 50 == 0,
        "a flow is not split: {fast} and {slow}"
    );
    assert!(
        fast > 0 && slow > 0,
        "{fast} over fast and {slow} over slow"
    );
    for workload in &result.workloads {
        assert_eq!(workload.stats.received, 50);
        assert!(workload.path.as_ref().unwrap().shared);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A failover (D-078): the fast path has the smaller metric; at 1.5 s its first link goes down and the packets that follow take
/// the slow one. Nothing draws random numbers here, so the step is done by the clock and not at a packet (D-062), and it lands
/// some milliseconds late: the packets of the fast path are those sent before `applied_ns` of the step, which the result
/// records. A packet leaves every 8 ms, so n packets left before a moment `t` when `t` is in ((n-1) x 8 ms, n x 8 ms]; the
/// sender is late by at most `max_lateness_ns`, which widens the lower bound. A packet that the router holds for the
/// microseconds of forwarding when its end of the link goes down could be lost, so at most one is.
#[test]
fn when_the_first_link_goes_down_the_route_of_the_next_metric_takes_over() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-failover");
    let mut backend = backend(|_| {});
    let mut failover = scenario(
        &two_paths(
            "  routes:\n    - { node: client, to: server, via: fast-in, metric: 10 }\n    - { node: client, to: server, via: slow-in, metric: 20 }\n",
        ),
        FLOW,
    );
    failover.duration = "4s".parse().unwrap();
    failover.timeline = vec![
        serde_json::from_str(r#"{ "at": "1500ms", "link": "fast-in", "down": true }"#).unwrap(),
    ];
    let result = run(
        &mut backend,
        &with_capture(&failover, &["fast-in", "slow-in"]),
        &options(&dir),
    )
    .unwrap();
    let media = only_workload(&result);
    assert_eq!(media.stats.sent, 500);
    assert_eq!(media.sender.send_errors, 0);
    assert!(media.stats.lost <= 1, "{} lost", media.stats.lost);
    let step = &result.events[0];
    let gap = 8_000_000_u64;
    let lowest = (step.started_ns.saturating_sub(media.sender.max_lateness_ns)).div_ceil(gap);
    let highest = step.applied_ns.div_ceil(gap);
    let (fast, slow) = (
        frames(&result, "fast-in", "client"),
        frames(&result, "slow-in", "client"),
    );
    assert_eq!(fast + slow, 500, "{fast} and {slow}");
    assert!(
        (lowest..=highest).contains(&fast),
        "{fast} packets took the fast path, the step began at {} ns and ended at {} ns (so {lowest} to {highest})",
        step.started_ns,
        step.applied_ns
    );
    assert!(
        slow > 300,
        "the packets after the step took the slow path: {slow}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A link that goes down takes the routes through it with it, and the kernel does not bring them back when it comes up
/// again. The chain loses the packets of the second from 1 s to 2 s and carries them again afterwards: with the routes lost
/// for good it would lose everything after 1 s (375 of 500). A packet leaves every 8 ms and is lost if it reaches the router
/// while the link is down, so the lost ones are those sent from the moment the step is done until the moment the next is,
/// give or take the lateness of the sender and a packet at each edge.
#[test]
fn a_link_that_comes_back_carries_the_routes_again() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-comes-back");
    let mut backend = backend(|_| {});
    let mut chain = scenario(CHAIN, FLOW);
    chain.duration = "4s".parse().unwrap();
    chain.timeline = vec![
        serde_json::from_str(r#"{ "at": "1s", "link": "l2", "down": true }"#).unwrap(),
        serde_json::from_str(r#"{ "at": "2s", "link": "l2", "up": true }"#).unwrap(),
    ];
    let result = run(&mut backend, &chain, &options(&dir)).unwrap();
    let media = only_workload(&result);
    assert_eq!(media.stats.sent, 500);
    let (down, up) = (&result.events[0], &result.events[1]);
    let gap = 8_000_000_u64;
    let late = media.sender.max_lateness_ns;
    let most = (up.applied_ns + late).saturating_sub(down.started_ns) / gap + 2;
    let least = (up.started_ns.saturating_sub(late)).saturating_sub(down.applied_ns) / gap;
    assert!(
        media.stats.lost >= least.saturating_sub(2) && media.stats.lost <= most,
        "{} packets lost, expected {least} to {most}: {:?}",
        media.stats.lost,
        result.events
    );
    assert!(
        media.stats.received > 340,
        "the packets after the link came back: {} received",
        media.stats.received
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The failover and the way back: the packets go to the slow path while the fast link is down, and to the fast one again
/// when it is back, because the route of the smaller metric is added again. Without that they would stay on the slow path.
#[test]
fn when_the_first_link_is_back_the_route_of_the_smaller_metric_is_used_again() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("network-way-back");
    let mut backend = backend(|_| {});
    let mut failover = scenario(
        &two_paths(
            "  routes:\n    - { node: client, to: server, via: fast-in, metric: 10 }\n    - { node: client, to: server, via: slow-in, metric: 20 }\n",
        ),
        FLOW,
    );
    failover.duration = "4s".parse().unwrap();
    failover.timeline = vec![
        serde_json::from_str(r#"{ "at": "1s", "link": "fast-in", "down": true }"#).unwrap(),
        serde_json::from_str(r#"{ "at": "2s", "link": "fast-in", "up": true }"#).unwrap(),
    ];
    let result = run(
        &mut backend,
        &with_capture(&failover, &["fast-in", "slow-in"]),
        &options(&dir),
    )
    .unwrap();
    let media = only_workload(&result);
    assert_eq!(media.stats.sent, 500);
    assert!(media.stats.lost <= 1, "{} lost", media.stats.lost);
    let (down, up) = (&result.events[0], &result.events[1]);
    let gap = 8_000_000_u64;
    let late = media.sender.max_lateness_ns;
    // The packets sent between the two steps take the slow path, and only those
    let most = (up.applied_ns + late).saturating_sub(down.started_ns) / gap + 2;
    let least = (up.started_ns.saturating_sub(late)).saturating_sub(down.applied_ns) / gap;
    let (fast, slow) = (
        frames(&result, "fast-in", "client"),
        frames(&result, "slow-in", "client"),
    );
    assert_eq!(fast + slow, 500, "{fast} and {slow}");
    assert!(
        (least.saturating_sub(2)..=most).contains(&slow),
        "{slow} packets took the slow path, expected {least} to {most}: {:?}",
        result.events
    );
    let _ = std::fs::remove_dir_all(dir);
}
