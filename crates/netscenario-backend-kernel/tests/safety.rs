//! The safety rules: nothing touches the network of the host (SAF-01), the limits hold (SAF-02) and the
//! watchdog ends a run that lasts too long (SAF-03).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use netscenario_core::{Backend, Program};
use netscenario_model::Name;
use support::{assert_nothing_left, backend, namespaces_available, supervisor_exe, tools};

fn host_interfaces() -> String {
    let output = Command::new(tools().ip)
        .args(["-o", "link", "show"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// SAF-01: started outside its namespaces, the supervisor must refuse before it does anything. This runs in the
/// namespace of the host, so it is the test that proves the refusal; it needs no user namespaces.
#[test]
fn a_supervisor_in_the_namespace_of_the_host_refuses_to_run() {
    let before = host_interfaces();
    let mut child = Command::new(supervisor_exe())
        .args(["supervise", "refusal-test", "8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(first.contains("\"status\":\"error\""), "{first}");
    assert!(first.contains("SAF-01"), "{first}");
    assert!(!status.success());
    assert_eq!(
        host_interfaces(),
        before,
        "the interfaces of the host changed"
    );
}

#[test]
fn a_run_may_not_use_more_namespaces_than_the_limit() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|options| options.max_nodes = 2);
    for name in ["a", "b"] {
        backend.create_node(&Name::new(name).unwrap()).unwrap();
    }
    let error = backend.create_node(&Name::new("c").unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("at most 2") && error.to_string().contains("SAF-02"),
        "{error}"
    );
    backend.teardown().unwrap();
    assert_nothing_left(backend.run_id(), "after the limit");
}

#[test]
fn the_watchdog_ends_a_run_that_lasts_too_long() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|options| options.max_run_time = Duration::from_secs(1));
    backend.create_node(&Name::new("a").unwrap()).unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    let error = backend.create_node(&Name::new("b").unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("watchdog") && error.to_string().contains("SAF-03"),
        "{error}"
    );
    assert_nothing_left(backend.run_id(), "after the watchdog");
    // The backend can be torn down after that, and again
    backend.teardown().unwrap();
    backend.teardown().unwrap();
}

#[test]
fn programs_end_with_the_run() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    let node = backend.create_node(&Name::new("a").unwrap()).unwrap();
    let marker = format!("marker-{}", backend.run_id());
    let program = Program::new(vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "while :; do sleep 1; done".to_owned(),
        marker.clone(),
    ]);
    backend.start_program(node, &program).unwrap();
    assert!(
        !support::processes_with(&marker).is_empty(),
        "the program did not start"
    );
    backend.teardown().unwrap();
    assert_nothing_left(&marker, "after the teardown");
    assert_nothing_left(backend.run_id(), "after the teardown");
}

#[test]
fn dropping_the_backend_ends_the_run() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    backend.create_node(&Name::new("a").unwrap()).unwrap();
    let run_id = backend.run_id().to_owned();
    assert_ne!(support::processes_with(&run_id), Vec::<String>::new());
    drop(backend);
    assert_nothing_left(&run_id, "after the drop");
}
