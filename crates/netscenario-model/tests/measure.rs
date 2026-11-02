//! The section `measure` (D-073): read, written out with only what was set, and the same for the builder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{Duration, Measure, RecoveryBasis, Scenario};

const HEAD: &str = "version: 0\nname: t\nduration: 20s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n";

fn scenario(tail: &str) -> Scenario {
    Scenario::from_yaml(&format!("{HEAD}{tail}"), "t.yaml")
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_scenario_without_the_section_has_the_defaults_and_writes_none() {
    let s = scenario("");
    assert!(s.measure.is_default());
    assert!(!serde_saphyr::to_string(&s).unwrap().contains("measure"));
}

#[test]
fn every_key_is_read_and_the_section_survives_being_written_out() {
    let s = scenario(
        "measure:\n  series: { bin: 250ms }\n  recovery: { basis: arrival, baseline-bins: 4, window-bins: 2, fraction: 80% }\n  capture: [l]\n  packets: true\n",
    );
    let series = s.measure.series.as_ref().unwrap();
    assert_eq!(series.bin, Some(Duration::from_millis(250).unwrap()));
    let recovery = s.measure.recovery.as_ref().unwrap();
    assert_eq!(recovery.basis, Some(RecoveryBasis::Arrival));
    assert_eq!(
        (recovery.baseline_bins, recovery.window_bins),
        (Some(4), Some(2))
    );
    assert_eq!(recovery.fraction.unwrap().micro_percent(), 80_000_000);
    let names: Vec<&str> = s
        .measure
        .capture
        .iter()
        .map(netscenario_model::Name::as_str)
        .collect();
    assert_eq!(names, ["l"]);
    assert!(s.measure.packets);
    let written = serde_saphyr::to_string(&s).unwrap();
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), s);
}

/// Only what was set is written: a recovery that names the basis and nothing else does not grow the other keys.
#[test]
fn a_part_of_a_section_stays_a_part() {
    let s = scenario("measure:\n  recovery: { basis: send }\n");
    let written = serde_saphyr::to_string(&s).unwrap();
    assert!(written.contains("basis: send"), "{written}");
    assert!(
        !written.contains("window-bins") && !written.contains("series"),
        "{written}"
    );
}

#[test]
fn the_builder_makes_what_the_file_makes() {
    let mut measure = Measure::default();
    measure.capture = vec!["l".parse().unwrap()];
    measure.packets = true;
    let built = Scenario::builder("t")
        .duration(Duration::from_secs(20).unwrap())
        .nodes("a", "b")
        .link("l", None, None)
        .udp_cbr(
            "w",
            "a",
            "b",
            "1mbit".parse().unwrap(),
            netscenario_model::Size::from_bytes(1000),
        )
        .measure(measure)
        .build()
        .unwrap();
    let from_file = scenario("measure:\n  capture: [l]\n  packets: true\n");
    assert_eq!(built.measure, from_file.measure);
}
