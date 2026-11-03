//! The network of a scenario (D-078): nodes with a role, several links, routes. A scenario of release 0.1 or 0.2 keeps its
//! text, and so its hash.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{Duration, Impairments, Role, Routing, Scenario, Size};

const WORKLOAD: &str = "workloads:\n  - { id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1000b }\n";

fn scenario(topology: &str) -> Scenario {
    Scenario::from_yaml(
        &format!("version: 0\nname: t\nduration: 10s\n{topology}{WORKLOAD}"),
        "t.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

/// The canonical text of the topology of a scenario, as JSON.
fn canonical(scenario: &Scenario) -> serde_json::Value {
    serde_json::to_value(&scenario.topology).unwrap()
}

/// The hash of a scenario of release 0.1 and 0.2 is the hash of its canonical text: a node written as a name must be written
/// as a name again, and the new keys must not appear when they were not set, or every replay of an old result would say
/// that the scenario differs.
#[test]
fn a_network_of_release_0_2_keeps_its_canonical_text() {
    let s = scenario(
        "topology:\n  nodes: [client, server]\n  links:\n    - { id: l, between: [client, server] }\n",
    );
    assert_eq!(
        canonical(&s),
        serde_json::json!({
            "nodes": ["client", "server"],
            "links": [{ "id": "l", "between": ["client", "server"] }]
        })
    );
    assert!(s.topology.is_point_to_point());
}

/// Canary for the test above: a router is written as a mapping, so the text does change when the network does.
#[test]
fn a_router_is_written_as_a_mapping_and_a_host_as_a_name() {
    let s = scenario(
        "topology:\n  nodes: [client, { id: r, role: router }, { id: server, role: host }]\n  links:\n    - { id: a, between: [client, r] }\n    - { id: b, between: [r, server] }\n",
    );
    assert_eq!(
        canonical(&s)["nodes"],
        serde_json::json!(["client", { "id": "r", "role": "router" }, "server"])
    );
    assert_eq!(s.topology.nodes[1].role, Role::Router);
    assert_eq!(s.topology.nodes[2].role, Role::Host);
    assert!(!s.topology.is_point_to_point());
    let written = serde_saphyr::to_string(&s).unwrap();
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), s);
}

#[test]
fn routing_routes_cost_and_ecmp_are_read_written_and_equal_to_the_builder() {
    let from_file = scenario(
        "topology:\n  routing: manual\n  nodes: [client, { id: r, role: router }, server]\n  links:\n    - { id: a, between: [client, r], cost: 3 }\n    - { id: b, between: [r, server] }\n    - { id: c, between: [r, server] }\n  routes:\n    - { node: client, to: server, via: a }\n    - { node: r, to: server, via: b, metric: 10 }\n    - { node: r, to: server, via: c, metric: 20, weight: 2 }\n",
    );
    let built = Scenario::builder("t")
        .duration(Duration::from_secs(10).unwrap())
        .node("client")
        .router("r")
        .node("server")
        .link_between("a", "client", "r", None, None)
        .link_cost(3)
        .link_between("b", "r", "server", None, None)
        .link_between("c", "r", "server", None, None)
        .routing(Routing::Manual)
        .route("client", "server", "a")
        .route("r", "server", "b")
        .route_metric(10)
        .route("r", "server", "c")
        .route_metric(20)
        .route_weight(2)
        .udp_cbr(
            "w",
            "client",
            "server",
            "1mbit".parse().unwrap(),
            Size::from_bytes(1000),
        )
        .build()
        .unwrap_or_else(|problems| panic!("{problems:?}"));
    assert_eq!(from_file, built);
    let written = serde_saphyr::to_string(&from_file).unwrap();
    assert_eq!(
        Scenario::from_yaml(&written, "again.yaml").unwrap(),
        from_file
    );
    // What is not set is not written
    let plain = scenario(
        "topology:\n  nodes: [client, server]\n  links:\n    - { id: l, between: [client, server] }\n",
    );
    let text = serde_saphyr::to_string(&plain).unwrap();
    for key in ["routing", "routes", "ecmp", "cost", "role"] {
        assert!(!text.contains(key), "{key} in {text}");
    }
}

#[test]
fn the_builder_checks_a_network_like_the_file_does() {
    let problems = Scenario::builder("t")
        .duration(Duration::from_secs(10).unwrap())
        .node("client")
        .node("server")
        .link_between("l", "client", "server", Some(Impairments::new()), None)
        .udp_cbr(
            "w",
            "client",
            "server",
            "1mbit".parse().unwrap(),
            Size::from_bytes(1000),
        )
        .route("client", "sever", "l")
        .build()
        .unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.message.contains("neither a node")),
        "{problems:?}"
    );
}

/// A network of several nodes and links is refused by `link` (which has only two nodes to choose from) with a pointer to
/// `link_between`, and not by silently picking two.
#[test]
fn link_without_two_nodes_points_to_link_between() {
    let problems = Scenario::builder("t")
        .duration(Duration::from_secs(10).unwrap())
        .node("a")
        .router("r")
        .node("b")
        .link("l", None, None)
        .udp_cbr(
            "w",
            "a",
            "b",
            "1mbit".parse().unwrap(),
            Size::from_bytes(1000),
        )
        .build()
        .unwrap_err();
    assert!(
        problems.iter().any(|p| p.message.contains("link_between")),
        "{problems:?}"
    );
}
