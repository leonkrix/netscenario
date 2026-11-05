//! The TCP workloads (D-086): read, written out with only what was set, built in code, and refused with the place of the fault.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{
    Duration, MAX_CONNECTIONS, MIN_REQUEST_INTERVAL, Scenario, Size, Workload, WorkloadKind,
};

const HEAD: &str = "version: 0\nname: t\nduration: 20s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n";

fn scenario(workload: &str) -> Scenario {
    Scenario::from_yaml(&format!("{HEAD}  - {workload}\n"), "t.yaml")
        .unwrap_or_else(|error| panic!("{error}"))
}

fn problems(workload: &str) -> Vec<String> {
    let text = format!("{HEAD}  - {workload}\n");
    match Scenario::from_yaml(&text, "t.yaml") {
        Ok(scenario) => scenario
            .validate()
            .iter()
            .map(|p| format!("{}: {}", p.path.as_deref().unwrap_or(""), p.message))
            .collect(),
        Err(error) => error
            .problems()
            .iter()
            .map(|p| format!("{}: {}", p.path.as_deref().unwrap_or(""), p.message))
            .collect(),
    }
}

#[test]
fn a_bulk_workload_has_one_connection_and_no_amount_unless_it_says_so() {
    let s = scenario("{ id: w, kind: tcp-bulk, from: a, to: b }");
    let bulk = s.workloads[0].as_tcp_bulk().unwrap();
    assert_eq!(s.workloads[0].kind(), WorkloadKind::TcpBulk);
    assert_eq!(
        (bulk.connections, bulk.bytes, bulk.cc.is_none()),
        (1, None, true)
    );
    // The defaults are not written out
    let written = serde_saphyr::to_string(&s).unwrap();
    assert!(
        !written.contains("connections") && !written.contains("cc:"),
        "{written}"
    );
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), s);
}

#[test]
fn every_key_of_both_kinds_is_read_and_survives_being_written_out() {
    let s = scenario(
        "{ id: w, kind: tcp-bulk, from: a, to: b, bytes: 5mb, connections: 3, cc: bbr, start: 1s, stop: 10s, ip: v4 }",
    );
    let bulk = s.workloads[0].as_tcp_bulk().unwrap();
    assert_eq!(bulk.bytes, Some(Size::from_bytes(5_000_000)));
    assert_eq!(
        (bulk.connections, bulk.cc.as_ref().unwrap().as_str()),
        (3, "bbr")
    );
    assert_eq!(
        Scenario::from_yaml(&serde_saphyr::to_string(&s).unwrap(), "again.yaml").unwrap(),
        s
    );
    let s = scenario(
        "{ id: w, kind: request-response, from: a, to: b, request: 200b, response: 4kb, connections: 4, interval: 20ms, timeout: 5s, cc: reno, start: 1s, stop: 10s }",
    );
    let exchange = s.workloads[0].as_request_response().unwrap();
    assert_eq!(s.workloads[0].kind(), WorkloadKind::RequestResponse);
    assert_eq!(
        (
            exchange.request.bytes(),
            exchange.response.bytes(),
            exchange.connections
        ),
        (200, 4_000, 4)
    );
    assert_eq!(exchange.interval, Some(Duration::from_millis(20).unwrap()));
    assert_eq!(
        Scenario::from_yaml(&serde_saphyr::to_string(&s).unwrap(), "again.yaml").unwrap(),
        s
    );
}

#[test]
fn the_builder_makes_the_same_workloads_as_the_file() {
    let built = Scenario::builder("t")
        .duration("20s".parse().unwrap())
        .nodes("a", "b")
        .link("l", None, None)
        .tcp_bulk("w", "a", "b")
        .bytes_each("5mb".parse().unwrap())
        .connections(3)
        .congestion("bbr")
        .window("1s".parse().unwrap(), "10s".parse().unwrap())
        .request_response(
            "r",
            "b",
            "a",
            "200b".parse().unwrap(),
            "4kb".parse().unwrap(),
        )
        .connections(4)
        .request_interval("20ms".parse().unwrap())
        .request_timeout("5s".parse().unwrap())
        .build()
        .unwrap_or_else(|problems| panic!("{problems:?}"));
    assert!(matches!(built.workloads[0], Workload::TcpBulk(_)));
    let bulk = built.workloads[0].as_tcp_bulk().unwrap();
    assert_eq!(
        (bulk.connections, bulk.cc.as_ref().unwrap().as_str()),
        (3, "bbr")
    );
    assert_eq!(bulk.start, Some("1s".parse().unwrap()));
    let exchange = built.workloads[1].as_request_response().unwrap();
    assert_eq!(
        (exchange.connections, exchange.timeout),
        (4, Some("5s".parse().unwrap()))
    );
    // A name that is not one is a problem with the path of the key, as for the other kinds
    let wrong = Scenario::builder("t")
        .duration("20s".parse().unwrap())
        .nodes("a", "b")
        .link("l", None, None)
        .tcp_bulk("w", "a", "b")
        .congestion("not a name")
        .build()
        .unwrap_err();
    assert!(
        wrong
            .iter()
            .any(|p| p.path.as_deref() == Some("workloads[0].cc")),
        "{wrong:?}"
    );
}

/// Canaries: each rule can fail.
#[test]
fn the_rules_name_the_key_that_breaks_them() {
    let cases: [(&str, &str); 9] = [
        (
            "{ id: w, kind: tcp-bulk, from: a, to: b, connections: 0 }",
            "workloads[0].connections: 0 connections are outside",
        ),
        (
            &format!(
                "{{ id: w, kind: tcp-bulk, from: a, to: b, connections: {} }}",
                MAX_CONNECTIONS + 1
            ),
            "workloads[0].connections",
        ),
        (
            "{ id: w, kind: tcp-bulk, from: a, to: b, bytes: 0b }",
            "workloads[0].bytes: `bytes` is 0",
        ),
        (
            "{ id: w, kind: tcp-bulk, from: a, to: a }",
            "workloads[0].to: a workload needs two different nodes",
        ),
        (
            "{ id: w, kind: tcp-bulk, from: a, to: c }",
            "workloads[0].to:",
        ),
        (
            "{ id: w, kind: tcp-bulk, from: a, to: b, start: 30s }",
            "workloads[0].start",
        ),
        (
            "{ id: w, kind: request-response, from: a, to: b, request: 0b, response: 1kb }",
            "workloads[0].request: the request of 0b is outside",
        ),
        (
            "{ id: w, kind: request-response, from: a, to: b, request: 1b, response: 2gib }",
            "workloads[0].response",
        ),
        (
            "{ id: w, kind: request-response, from: a, to: b, request: 1b, response: 1b, interval: 100us }",
            "workloads[0].interval",
        ),
    ];
    for (workload, expected) in cases {
        let found = problems(workload);
        assert!(
            found.iter().any(|p| p.starts_with(expected)),
            "{workload}: expected '{expected}', found {found:?}"
        );
    }
    assert_eq!(MIN_REQUEST_INTERVAL, Duration::from_millis(1).unwrap());
}

/// Keys of another kind are refused with the name of the kind, and a missing key is named.
#[test]
fn keys_of_other_kinds_are_refused_and_missing_ones_named() {
    let found = problems("{ id: w, kind: tcp-bulk, from: a, to: b, rate: 1mbit }");
    assert!(
        found
            .iter()
            .any(|m| m.contains("rate") && m.contains("tcp-bulk")),
        "{found:?}"
    );
    let found = problems("{ id: w, kind: request-response, from: a, to: b, request: 1b }");
    assert!(found.iter().any(|m| m.contains("response")), "{found:?}");
    let found = problems(
        "{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b, connections: 2 }",
    );
    assert!(
        found
            .iter()
            .any(|m| m.contains("connections") && m.contains("udp-cbr")),
        "{found:?}"
    );
}
