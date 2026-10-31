//! The timeline in the model: a key that is written with `null` is not the same as a key that is not
//! written, a change applies to impairments as a merge patch, and a timeline survives being written out.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{Direction, Edit, Impairments, Scenario};

const HEAD: &str = "version: 0\nname: t\nduration: 30s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 10ms, rate: 8mbit, loss: { model: random, p: 1% } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\ntimeline:\n";

fn scenario(timeline: &str) -> Scenario {
    Scenario::from_yaml(&format!("{HEAD}{timeline}"), "t.yaml")
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_key_with_null_takes_the_impairment_away_and_a_key_that_is_absent_leaves_it() {
    let s = scenario("  - { at: 5s, link: l, set: { both: { rate: null, delay: 20ms } } }\n");
    let change = s.timeline[0].set.as_ref().unwrap().both.as_ref().unwrap();
    // written with null, written with a value, not written
    assert_eq!(change.rate, Edit::Clear);
    assert_eq!(change.delay, Edit::Set("20ms".parse().unwrap()));
    assert_eq!(change.loss, Edit::Keep);
    let before = s.topology.links[0].both.clone().unwrap();
    let after = change.applied_to(&before);
    assert_eq!(after.rate, None);
    assert_eq!(after.delay, Some("20ms".parse().unwrap()));
    // the loss was not mentioned: it stays
    assert_eq!(after.loss, before.loss);
}

/// Canary: the test above can fail. A change that mentions nothing leaves every impairment as it was, so
/// "not written" and "null" are not read as the same thing.
#[test]
fn canary_a_change_that_writes_nothing_changes_nothing() {
    let before: Impairments = scenario("").topology.links[0].both.clone().unwrap();
    assert_eq!(
        netscenario_model::ImpairmentsChange::default().applied_to(&before),
        before
    );
}

#[test]
fn a_timeline_survives_being_written_out_with_its_nulls() {
    let s = scenario(
        "  - { at: 5s, link: l, set: { forward: { rate: null }, reverse: { duplicate: 2% } } }\n  - { at: 8s, link: l, outage: 1s, direction: forward }\n  - { at: 12s, link: l, down: true }\n  - { at: 13s, link: l, up: true }\n",
    );
    let written = serde_saphyr::to_string(&s).unwrap();
    assert!(written.contains("rate: null"), "{written}");
    assert_eq!(Scenario::from_yaml(&written, "again.yaml").unwrap(), s);
    assert_eq!(s.timeline[1].direction, Some(Direction::Forward));
}

#[test]
fn events_at_one_moment_are_allowed_and_keep_the_order_of_the_file() {
    let s = scenario(
        "  - { at: 5s, link: l, set: { both: { rate: 1mbit } } }\n  - { at: 5s, link: l, set: { both: { rate: 2mbit } } }\n",
    );
    assert_eq!(s.timeline.len(), 2);
    assert_eq!(s.validate(), Vec::new());
}

#[test]
fn a_scenario_without_a_timeline_writes_no_timeline_key() {
    let s = Scenario::from_yaml(HEAD.trim_end_matches("timeline:\n"), "t.yaml").unwrap();
    assert_eq!(s.timeline, Vec::new());
    assert!(!serde_saphyr::to_string(&s).unwrap().contains("timeline"));
}
