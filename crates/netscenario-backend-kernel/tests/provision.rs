//! A scenario is built in real namespaces, and the network does what the scenario says.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod support;

use netscenario_backend_kernel::{Tool, Version};
use netscenario_core::{Backend, NodeHandle, Program, provision};
use support::{
    assert_nothing_left, backend, is_root, namespaces_available, program, scenario, tools,
    wait_for, work_dir,
};

fn sh(script: &str) -> Program {
    Program::new(vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()])
}

#[test]
fn the_impairments_are_in_the_queues_of_the_right_interfaces() {
    if !namespaces_available() {
        return;
    }
    let dir = work_dir("queues");
    let mut backend = backend(|_| {});
    let scenario = scenario(
        "",
        "      forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal }, rate: 2mbit }\n      reverse: { delay: 1500us }\n",
    );
    let built = provision(&mut backend, &scenario).unwrap();
    assert_eq!(built.nodes.len(), 2);
    let tc = tools().tc;
    for (node, file) in [
        (built.nodes[0].1, "client.txt"),
        (built.nodes[1].1, "server.txt"),
    ] {
        let script = format!(
            "{} qdisc show dev l0 > {}/{file}",
            tc.display(),
            dir.display()
        );
        backend.start_program(node, &sh(&script)).unwrap();
    }
    let client = wait_for(&dir.join("client.txt"), "netem");
    // A jitter with a rate is two qdiscs: the rate first, the delay with its jitter in its child (see
    // `netem.rs` for why). `tc` does not print the name of the distribution, so that the normal one is in use
    // is for the fidelity tests to show.
    let root = client
        .lines()
        .find(|line| line.contains(" root "))
        .unwrap_or_else(|| panic!("no root qdisc in: {client}"));
    let child = client
        .lines()
        .find(|line| line.contains("parent 1:1"))
        .unwrap_or_else(|| panic!("no child qdisc in: {client}"));
    assert!(
        root.contains("limit 1000") && root.contains("rate 2Mbit"),
        "{root}"
    );
    assert!(
        !root.contains("delay"),
        "the delay must not be in the rate stage: {root}"
    );
    assert!(
        child.contains("delay 20ms  5ms") && child.contains("limit 3500"),
        "{child}"
    );
    assert!(
        !child.contains("rate"),
        "the rate must not be in the delay stage: {child}"
    );
    let server = wait_for(&dir.join("server.txt"), "netem");
    assert!(server.contains("delay 1.5ms"), "{server}");
    assert!(!server.contains("rate"), "{server}");
    backend.teardown().unwrap();
    assert_nothing_left(backend.run_id(), "after the teardown");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_ping_takes_the_round_trip_of_both_directions() {
    if !namespaces_available() {
        return;
    }
    let Some(ping) = program("ping") else {
        assert!(std::env::var_os("CI").is_none(), "ping is needed on the CI");
        eprintln!("SKIPPED: ping is not installed");
        return;
    };
    let dir = work_dir("ping");
    let mut backend = backend(|_| {});
    let built = provision(&mut backend, &scenario("", "      both: { delay: 20ms }\n")).unwrap();
    let script = format!(
        "{} -c 5 -i 0.2 10.100.0.2 > {}/ping.txt",
        ping.display(),
        dir.display()
    );
    backend
        .start_program(built.nodes[0].1, &sh(&script))
        .unwrap();
    let output = wait_for(&dir.join("ping.txt"), "rtt min/avg/max");
    let average: f64 = output
        .lines()
        .find(|line| line.starts_with("rtt"))
        .and_then(|line| line.split('=').nth(1))
        .and_then(|numbers| numbers.split('/').nth(1))
        .and_then(|average| average.parse().ok())
        .unwrap_or_else(|| panic!("no round trip time in: {output}"));
    // 20 ms each way, and the measurement noise of the machine is far below a millisecond (spike S4)
    assert!(
        (39.5..42.0).contains(&average),
        "the round trip took {average} ms"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The packets of a ping that did not come back, for a loss of 20 % in the direction to the server.
fn lost_with_seed(seed: &str, ping: &std::path::Path, label: &str) -> Vec<u32> {
    let dir = work_dir(label);
    let mut backend = backend(|_| {});
    let built = provision(
        &mut backend,
        &scenario(seed, "      forward: { loss: { model: random, p: 20% } }\n"),
    )
    .unwrap();
    let script = format!(
        "{} -c 100 -i 0.02 -W 1 10.100.0.2 > {}/ping.txt",
        ping.display(),
        dir.display()
    );
    backend
        .start_program(built.nodes[0].1, &sh(&script))
        .unwrap();
    let output = wait_for(&dir.join("ping.txt"), "packets transmitted");
    let answered: Vec<u32> = output
        .lines()
        .filter_map(|line| line.split("icmp_seq=").nth(1))
        .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
        .collect();
    let _ = std::fs::remove_dir_all(dir);
    (1..=100).filter(|n| !answered.contains(n)).collect()
}

#[test]
fn the_same_seed_loses_the_same_packets() {
    if !namespaces_available() {
        return;
    }
    let Some(ping) = program("ping") else {
        assert!(std::env::var_os("CI").is_none(), "ping is needed on the CI");
        eprintln!("SKIPPED: ping is not installed");
        return;
    };
    let backend = backend(|_| {});
    let version = match backend.environment().tc {
        Tool::Found(version) if version >= Version::new(6, 6, 0) => version,
        ref other => {
            // The CI builds iproute2 6.15 for this test (D-023), so there it must not be skipped
            assert!(
                std::env::var_os("CI").is_none(),
                "the tc of the CI cannot seed: {other:?}"
            );
            eprintln!("SKIPPED: this tc does not know `seed` ({other:?})");
            return;
        }
    };
    eprintln!("tc {version}");
    drop(backend);
    let first = lost_with_seed("seed: 1234\n", &ping, "seed-a");
    let second = lost_with_seed("seed: 1234\n", &ping, "seed-b");
    let other = lost_with_seed("seed: 999\n", &ping, "seed-c");
    assert!(!first.is_empty(), "20 % of 100 packets should lose some");
    assert_eq!(first, second, "the same seed must lose the same packets");
    assert_ne!(first, other, "another seed should lose other packets");
}

#[test]
fn a_refused_scenario_starts_nothing() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    let refused = scenario("", "      both: { delay: 1500ns }\n");
    let error = provision(&mut backend, &refused).unwrap_err();
    assert!(error.to_string().contains("whole microseconds"), "{error}");
    assert_nothing_left(backend.run_id(), "after the teardown");
}

#[test]
fn a_node_that_does_not_exist_is_an_error_not_a_crash() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    let program = Program::new(vec!["true".to_owned()]);
    let error = backend.start_program(NodeHandle(5), &program).unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
}

/// A run has the rights of its caller: a root process runs it with its own rights, and anyone else in a user
/// namespace in which they are root. The CI runs these tests once as root, so that both ways are tested.
#[test]
fn a_run_uses_the_rights_of_its_caller() {
    if !namespaces_available() {
        return;
    }
    let dir = work_dir("rights");
    let mut backend = backend(|_| {});
    let node = netscenario_core::Backend::create_node(
        &mut backend,
        &netscenario_model::Name::new("a").unwrap(),
    )
    .unwrap();
    backend
        .start_program(
            node,
            &sh(&format!(
                "cat /proc/self/uid_map > {}/map.txt",
                dir.display()
            )),
        )
        .unwrap();
    let in_run = wait_for(&dir.join("map.txt"), "0");
    let own = std::fs::read_to_string("/proc/self/uid_map").unwrap();
    if is_root() {
        assert_eq!(
            in_run, own,
            "a root caller runs the run with its own rights"
        );
    } else {
        assert_ne!(
            in_run, own,
            "an ordinary user runs the run in a user namespace"
        );
        assert!(
            in_run.trim_end().ends_with(" 1"),
            "only the user is mapped: {in_run}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn asking_for_root_without_the_rights_says_what_is_missing() {
    if !namespaces_available() || is_root() {
        return;
    }
    let mut backend = backend(|options| options.mode = netscenario_backend_kernel::Mode::Root);
    let error = netscenario_core::Backend::create_node(
        &mut backend,
        &netscenario_model::Name::new("a").unwrap(),
    )
    .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("Operation not permitted"), "{text}");
    assert!(text.contains("docs/COVERAGE.md"), "{text}");
}
