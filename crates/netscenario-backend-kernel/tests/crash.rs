//! The cleanup guarantee (BKD-04): the supervisor dies without any cleanup at every step of the setup and
//! of the teardown, and nothing of the run is left afterwards. A later run works as if nothing happened.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use netscenario_core::{Backend, Program, provision};
use support::{assert_nothing_left, backend, namespaces_available, scenario};

/// Every point where the supervisor can be made to die, with the number of the time it passes it.
const POINTS: &[&str] = &[
    "node:start",
    "node:spawned",
    "node:end",
    "node:start#2",
    "node:spawned#2",
    "node:end#2",
    "link:start",
    "link:created",
    "link:moved",
    "link:end",
    // The steps of a network (D-078): the address of a node, whether it forwards, and a route; each node and each route
    // passes the point once, so the second pass is a second node
    "address:start",
    "address:end",
    "address:start#2",
    "forwarding:start",
    "forwarding:end",
    "forwarding:start#2",
    "route:start",
    "route:end",
    "route:start#2",
    "apply:start",
    "apply:end",
    "apply:start#2",
    "apply:end#2",
    "program:start",
    "program:end",
    "teardown:start",
    "teardown:end",
];

fn spin(marker: &str) -> Program {
    Program::new(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "while :; do sleep 1; done".to_owned(),
        marker.to_owned(),
    ])
}

/// Runs the whole life of a run: build, start a program, tear down. Returns what failed first.
fn life(point: Option<&str>) -> (String, Option<String>) {
    life_of(point, &scenario("", "      both: { delay: 5ms }\n"))
}

fn life_of(
    point: Option<&str>,
    scenario: &netscenario_model::Scenario,
) -> (String, Option<String>) {
    let mut backend = backend(|options| options.fail_at = point.map(str::to_owned));
    let run_id = backend.run_id().to_owned();
    let marker = format!("marker-{run_id}");
    let failed = (|| {
        let built = provision(&mut backend, scenario).map_err(|e| e.to_string())?;
        backend
            .start_program(built.nodes[0].1, &spin(&marker))
            .map_err(|e| e.to_string())?;
        backend.teardown().map_err(|e| e.to_string())
    })()
    .err();
    // The marker of the program is checked as well as the run id of the supervisor and the holders
    assert_nothing_left(&marker, &format!("{point:?}"));
    (run_id, failed)
}

#[test]
fn nothing_is_left_when_the_supervisor_dies_at_any_step() {
    if !namespaces_available() {
        return;
    }
    for point in POINTS {
        let (run_id, failed) = life(Some(point));
        if point.starts_with("teardown") {
            // A death during the teardown has done what the teardown is for: the kernel ended everything, so
            // the call reports success. What counts is that nothing is left.
            assert_eq!(failed, None, "{point}");
        } else {
            let message =
                failed.unwrap_or_else(|| panic!("{point}: the supervisor did not die there"));
            assert!(
                message.contains("ended") || message.contains("it said"),
                "{point}: unexpected error: {message}"
            );
        }
        assert_nothing_left(&run_id, point);
    }
}

/// Canary: without a failpoint the same life succeeds, so a failure above comes from the failpoint and not
/// from the setup.
#[test]
fn the_same_life_without_a_failpoint_succeeds_and_leaves_nothing() {
    if !namespaces_available() {
        return;
    }
    let (run_id, failed) = life(None);
    assert_eq!(failed, None);
    assert_nothing_left(&run_id, "no failpoint");
}

#[test]
fn a_run_after_a_crash_works() {
    if !namespaces_available() {
        return;
    }
    let (_, failed) = life(Some("link:moved"));
    assert!(failed.is_some());
    let (_, failed) = life(None);
    assert_eq!(failed, None);
}

/// The steps that only a link with an `mtu` and a network with IPv6 have (D-081), each with the supervisor dying at it: the MTU of
/// a link, and the wait for the quiet. Nothing is left afterwards. The scenario has no impairment, so the points of the MTU run
/// on a machine without `netem` as well; the points of IPv6 need a kernel that has it.
fn dies_at(points: &[&str], yaml: &str, needs_ipv6: bool) {
    if !namespaces_available() || (needs_ipv6 && !support::ipv6_available()) {
        return;
    }
    let scenario = netscenario_model::Scenario::from_yaml(yaml, "crash.yaml").unwrap();
    for point in points {
        let (run_id, failed) = life_of(Some(point), &scenario);
        let message = failed.unwrap_or_else(|| panic!("{point}: the supervisor did not die there"));
        assert!(
            message.contains("ended") || message.contains("it said"),
            "{point}: unexpected error: {message}"
        );
        assert_nothing_left(&run_id, point);
    }
}

#[test]
fn nothing_is_left_when_the_supervisor_dies_while_the_mtu_is_set() {
    dies_at(
        &["mtu:start", "mtu:end"],
        "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [client, server]\n  links:\n    - { id: access, between: [client, server], mtu: 1000 }\nworkloads:\n  - { id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 500b }\n",
        false,
    );
}

#[test]
fn nothing_is_left_when_the_supervisor_dies_in_a_network_with_ipv6() {
    dies_at(
        &["quiet:start", "quiet:end", "address:end#2", "route:end#2"],
        "version: 0\nname: t\nduration: 10s\ntopology:\n  ip: dual\n  nodes: [client, server]\n  links:\n    - { id: access, between: [client, server] }\nworkloads:\n  - { id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 500b }\n",
        true,
    );
}

/// The fabric of a run (D-079) is a namespace like the nodes, with its own holder: the supervisor dies when it is made, when it
/// is ready, and while the tree of the queue is built, and nothing is left. The scenario has a queue and no other impairment.
#[test]
fn nothing_is_left_when_the_supervisor_dies_while_the_fabric_is_made() {
    dies_at(
        &["fabric:start", "fabric:end", "apply:start", "apply:end"],
        "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [client, server]\n  links:\n    - { id: access, between: [client, server], forward: { rate: 2mbit, queue: { limit: 50p } } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 500b }\n",
        false,
    );
}
