//! The generators of traffic besides the constant bit rate (WRK-02): they are read from a file, built in code, written
//! out again, and seen as a `Generator` whatever their pattern.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{
    Duration, Generator, Pattern, PhaseDistribution, Rate, Scenario, Size, Workload, WorkloadKind,
};

const HEAD: &str = "version: 0\nname: t\nduration: 20s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n";

fn scenario(workloads: &str) -> Scenario {
    Scenario::from_yaml(&format!("{HEAD}{workloads}"), "t.yaml")
        .unwrap_or_else(|error| panic!("{error}"))
}

fn duration(text: &str) -> Duration {
    text.parse().unwrap()
}

#[test]
fn the_three_kinds_are_read_with_their_keys_and_defaults() {
    let s = scenario(
        "  - { id: p, kind: udp-poisson, from: a, to: b, rate: 800kbit, packet-size: 400b }
  - { id: o, kind: udp-onoff, from: a, to: b, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms }
  - { id: x, kind: udp-bursty, from: b, to: a, packet-size: 1200b, burst-packets: 10, gap: 200us, pause: 100ms, distribution: exponential, start: 1s, stop: 9s, udp-checksum: false }
",
    );
    let kinds: Vec<WorkloadKind> = s.workloads.iter().map(Workload::kind).collect();
    assert_eq!(
        kinds,
        [
            WorkloadKind::UdpPoisson,
            WorkloadKind::UdpOnoff,
            WorkloadKind::UdpBursty
        ]
    );
    let patterns: Vec<Pattern> = s
        .workloads
        .iter()
        .map(|w| w.generator().unwrap().pattern)
        .collect();
    assert_eq!(
        patterns,
        [
            Pattern::Poisson {
                rate: "800kbit".parse::<Rate>().unwrap()
            },
            Pattern::OnOff {
                rate: "400kbit".parse().unwrap(),
                on: duration("500ms"),
                off: duration("1500ms"),
                distribution: PhaseDistribution::Fixed,
            },
            Pattern::Bursty {
                packets: 10,
                gap: duration("200us"),
                pause: duration("100ms"),
                distribution: PhaseDistribution::Exponential,
            },
        ]
    );
    let bursty: Generator<'_> = s.workloads[2].generator().unwrap();
    assert_eq!(bursty.packet_size, Size::from_bytes(1200));
    assert_eq!(bursty.start, Some(duration("1s")));
    assert_eq!(bursty.stop, Some(duration("9s")));
    assert!(!bursty.udp_checksum.is_on());
    assert_eq!((bursty.from.as_str(), bursty.to.as_str()), ("b", "a"));
}

#[test]
fn a_scenario_with_every_kind_survives_being_written_out_and_leaves_the_defaults_out() {
    let s = scenario(
        "  - { id: c, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
  - { id: p, kind: udp-poisson, from: a, to: b, rate: 800kbit, packet-size: 400b, start: 2s }
  - { id: o, kind: udp-onoff, from: a, to: b, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms }
  - { id: x, kind: udp-bursty, from: b, to: a, packet-size: 1200b, burst-packets: 10, gap: 200us, pause: 100ms, distribution: exponential }
",
    );
    let written = serde_saphyr::to_string(&s).unwrap();
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), s);
    // Only the bursty workload said `exponential`; the fixed phases of the on/off workload are the default
    assert_eq!(written.matches("distribution").count(), 1, "{written}");
    assert!(written.contains("kind: udp-onoff"), "{written}");
}

/// The builder makes the same model as the file.
#[test]
fn the_builder_makes_what_the_file_makes() {
    let built = Scenario::builder("t")
        .duration(duration("20s"))
        .nodes("a", "b")
        .link("l", None, None)
        .udp_poisson(
            "p",
            "a",
            "b",
            "800kbit".parse().unwrap(),
            Size::from_bytes(400),
        )
        .window(duration("1s"), duration("9s"))
        .udp_onoff(
            "o",
            "a",
            "b",
            "400kbit".parse().unwrap(),
            Size::from_bytes(200),
            duration("500ms"),
            duration("1500ms"),
        )
        .drawn_phases()
        .udp_bursty(
            "x",
            "b",
            "a",
            Size::from_bytes(1200),
            10,
            duration("200us"),
            duration("100ms"),
        )
        .without_udp_checksum()
        .build()
        .unwrap();
    let from_file = scenario(
        "  - { id: p, kind: udp-poisson, from: a, to: b, rate: 800kbit, packet-size: 400b, start: 1s, stop: 9s }
  - { id: o, kind: udp-onoff, from: a, to: b, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms, distribution: exponential }
  - { id: x, kind: udp-bursty, from: b, to: a, packet-size: 1200b, burst-packets: 10, gap: 200us, pause: 100ms, udp-checksum: false }
",
    );
    assert_eq!(built.workloads, from_file.workloads);
}

/// Canary: the comparison above can fail. A different distribution is a different workload.
#[test]
fn canary_a_different_distribution_is_a_different_workload() {
    let fixed = scenario(
        "  - { id: o, kind: udp-onoff, from: a, to: b, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms }\n",
    );
    let drawn = scenario(
        "  - { id: o, kind: udp-onoff, from: a, to: b, rate: 400kbit, packet-size: 200b, on: 500ms, off: 1500ms, distribution: exponential }\n",
    );
    assert_ne!(fixed.workloads, drawn.workloads);
}

#[test]
fn a_program_of_your_own_is_not_a_generator() {
    let s = scenario(
        "  - { id: c, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }
  - { id: app, kind: command, node: b, run: [\"./server\"] }
",
    );
    assert!(s.workloads[0].generator().is_some());
    assert!(s.workloads[1].generator().is_none());
}

/// A packet of no bytes is refused for its size. The check of the rate divides by the size, which was zero here: the fuzz
/// target of the reader found it.
#[test]
fn a_packet_of_no_bytes_is_refused_without_dividing_by_it() {
    for kind in [
        "udp-cbr, rate: 1mbit",
        "udp-poisson, rate: 1mbit",
        "udp-onoff, rate: 1mbit, on: 1s, off: 1s",
    ] {
        let text =
            format!("{HEAD}  - {{ id: w, kind: {kind}, from: a, to: b, packet-size: 0b }}\n");
        let error = Scenario::from_yaml(&text, "t.yaml").unwrap_err();
        let messages: Vec<&str> = error
            .problems()
            .iter()
            .map(|problem| problem.message.as_str())
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("0 bytes") || message.contains("packet")),
            "{messages:?}"
        );
        // The rate was not the problem: it is a rate that the agent sends at
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("packets per second")),
            "{messages:?}"
        );
    }
}
