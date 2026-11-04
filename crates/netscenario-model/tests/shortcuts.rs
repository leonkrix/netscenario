//! The shortcuts `chain`, `star`, `dumbbell` and `multipath` (D-078): each is the same scenario as its long form, with the
//! same canonical text and so the same hash, and what is written out again is the long form.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::Scenario;

fn scenario(topology: &str, workload: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: t\nduration: 10s\n{topology}workloads:\n  - {{ id: w, kind: udp-cbr, {workload}, rate: 1mbit, packet-size: 1000b }}\n"
        ),
        "t.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

/// The scenario of the shortcut and of the long form are equal, their canonical JSON is the same text, and writing the
/// scenario out does not bring the shortcut back.
fn same(shortcut: &str, long: &str, workload: &str) {
    let short = scenario(shortcut, workload);
    let long = scenario(long, workload);
    assert_eq!(short, long);
    assert_eq!(
        serde_json::to_string(&short).unwrap(),
        serde_json::to_string(&long).unwrap(),
        "the canonical text, which the hash is made of"
    );
    let written = serde_saphyr::to_string(&short).unwrap();
    for key in ["chain", "star", "dumbbell", "multipath"] {
        assert!(!written.contains(&format!("{key}:")), "{written}");
    }
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), short);
}

#[test]
fn a_chain_has_hosts_at_the_ends_routers_between_and_a_link_for_each_pair() {
    same(
        "topology:\n  chain:\n    nodes: [client, edge, core, server]\n    link: { both: { delay: 5ms } }\n",
        "topology:\n  nodes: [client, { id: edge, role: router }, { id: core, role: router }, server]\n  links:\n    - { id: client-edge, between: [client, edge], both: { delay: 5ms } }\n    - { id: edge-core, between: [edge, core], both: { delay: 5ms } }\n    - { id: core-server, between: [core, server], both: { delay: 5ms } }\n",
        "from: client, to: server",
    );
    // A chain of two nodes is a link between two hosts
    same(
        "topology:\n  chain: { nodes: [a, b] }\n",
        "topology:\n  nodes: [a, b]\n  links:\n    - { id: a-b, between: [a, b] }\n",
        "from: a, to: b",
    );
    // Each link its own values
    same(
        "topology:\n  chain:\n    nodes: [a, r, b]\n    links:\n      - { both: { delay: 1ms } }\n      - { cost: 4, forward: { rate: 2mbit } }\n",
        "topology:\n  nodes: [a, { id: r, role: router }, b]\n  links:\n    - { id: a-r, between: [a, r], both: { delay: 1ms } }\n    - { id: r-b, between: [r, b], cost: 4, forward: { rate: 2mbit } }\n",
        "from: a, to: b",
    );
}

#[test]
fn a_star_has_a_router_for_a_hub_and_a_link_from_each_leaf_towards_it() {
    same(
        "topology:\n  star:\n    hub: hub\n    leaves: [a, b, c]\n    link: { forward: { delay: 4ms } }\n",
        "topology:\n  nodes: [{ id: hub, role: router }, a, b, c]\n  links:\n    - { id: a-hub, between: [a, hub], forward: { delay: 4ms } }\n    - { id: b-hub, between: [b, hub], forward: { delay: 4ms } }\n    - { id: c-hub, between: [c, hub], forward: { delay: 4ms } }\n",
        "from: a, to: c",
    );
}

#[test]
fn a_dumbbell_shares_a_bottleneck_between_two_routers() {
    same(
        "topology:\n  dumbbell:\n    left: [a1, a2]\n    right: [b1]\n    access: { both: { delay: 1ms } }\n    bottleneck: { both: { rate: 10mbit } }\n",
        "topology:\n  nodes: [a1, a2, { id: router-left, role: router }, { id: router-right, role: router }, b1]\n  links:\n    - { id: a1-router-left, between: [a1, router-left], both: { delay: 1ms } }\n    - { id: a2-router-left, between: [a2, router-left], both: { delay: 1ms } }\n    - { id: bottleneck, between: [router-left, router-right], both: { rate: 10mbit } }\n    - { id: b1-router-right, between: [b1, router-right], both: { delay: 1ms } }\n",
        "from: a1, to: b1",
    );
}

#[test]
fn a_multipath_has_a_chain_of_routers_for_each_path() {
    same(
        "topology:\n  multipath:\n    from: client\n    to: server\n    paths:\n      - { routers: [fast], link: { both: { delay: 5ms } } }\n      - { routers: [slow1, slow2], link: { both: { delay: 20ms } } }\n",
        "topology:\n  nodes: [client, { id: fast, role: router }, { id: slow1, role: router }, { id: slow2, role: router }, server]\n  links:\n    - { id: client-fast, between: [client, fast], both: { delay: 5ms } }\n    - { id: fast-server, between: [fast, server], both: { delay: 5ms } }\n    - { id: client-slow1, between: [client, slow1], both: { delay: 20ms } }\n    - { id: slow1-slow2, between: [slow1, slow2], both: { delay: 20ms } }\n    - { id: slow2-server, between: [slow2, server], both: { delay: 20ms } }\n",
        "from: client, to: server",
    );
}

/// The routing keys sit beside a shortcut, and a route can name what the shortcut made.
#[test]
fn routing_and_routes_can_be_written_next_to_a_shortcut() {
    let s = scenario(
        "topology:\n  multipath:\n    from: client\n    to: server\n    paths:\n      - { routers: [fast] }\n      - { routers: [slow] }\n  routing: auto\n  ecmp: true\n  routes:\n    - { node: client, to: server, via: client-slow, metric: 50 }\n",
        "from: client, to: server",
    );
    assert!(s.topology.ecmp);
    let routes = s.topology.routes();
    let path = s
        .topology
        .path(
            &routes,
            &netscenario_model::Name::new("client").unwrap(),
            &netscenario_model::Name::new("server").unwrap(),
        )
        .unwrap();
    assert_eq!(
        path.nodes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" "),
        "client slow server"
    );
}

/// Canary for the equality above: a shortcut that differs from the long form in one value is a different scenario.
#[test]
fn a_shortcut_and_a_long_form_that_differ_are_different_scenarios() {
    let short = scenario(
        "topology:\n  chain: { nodes: [a, r, b], link: { both: { delay: 5ms } } }\n",
        "from: a, to: b",
    );
    let long = scenario(
        "topology:\n  nodes: [a, { id: r, role: router }, b]\n  links:\n    - { id: a-r, between: [a, r], both: { delay: 5ms } }\n    - { id: r-b, between: [r, b], both: { delay: 6ms } }\n",
        "from: a, to: b",
    );
    assert_ne!(short, long);
}
