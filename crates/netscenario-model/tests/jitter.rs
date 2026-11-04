//! The distributions of the jitter that `netem` draws from a table: the tables that ship with iproute2 (`pareto`,
//! `paretonormal`) and a table made from a file of samples (LNK-02, D-083, D-084).

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::fmt::Write as _;

use netscenario_model::{Edit, JitterDistribution, Scenario};

const HEAD: &str = "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n";
const TAIL: &str =
    "workloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

fn scenario(forward: &str, timeline: &str) -> Result<Scenario, String> {
    let text = format!("{HEAD}      forward: {forward}\n{TAIL}{timeline}");
    Scenario::from_yaml(&text, "t.yaml").map_err(|error| error.render())
}

/// `count` delays between 10 and 50 (evenly spread, so the table is a straight line).
fn uniform_samples(count: usize) -> Vec<u8> {
    let mut text = String::from("# measured round trips in ms\n");
    for i in 0..count {
        #[allow(clippy::cast_precision_loss)]
        let _ = writeln!(text, "{:.3}", 10.0 + 40.0 * (i as f64 + 0.5) / count as f64);
    }
    text.into_bytes()
}

#[test]
fn the_tables_of_iproute2_are_distributions_of_the_jitter() {
    for (word, expected) in [
        ("pareto", JitterDistribution::Pareto),
        ("paretonormal", JitterDistribution::ParetoNormal),
    ] {
        let parsed = scenario(
            &format!("{{ delay: 40ms, jitter: {{ amount: 10ms, distribution: {word} }} }}"),
            "",
        )
        .unwrap();
        let jitter = parsed.topology.links[0]
            .forward
            .as_ref()
            .unwrap()
            .jitter
            .clone()
            .unwrap();
        assert_eq!(jitter.distribution, expected);
        assert_eq!(expected.as_str(), word);
        assert!(jitter.table.is_none());
        // The canonical form says the word again, and reads back to an equal scenario
        let yaml = parsed.to_yaml().unwrap();
        assert!(yaml.contains(&format!("distribution: {word}")), "{yaml}");
        assert_eq!(Scenario::from_yaml(&yaml, "again.yaml").unwrap(), parsed);
    }
}

/// The delay has to reach the lowest entry of the table below the mean: for `pareto` -5461 / 8192 = 0.667 of the amount, for
/// `paretonormal` 12305 / 8192 = 1.502. The bound is exact, so a delay of exactly that is accepted and one nanosecond
/// less is not (a canary: the check can fail).
#[test]
fn the_delay_must_reach_as_far_below_as_the_table_does() {
    // pareto, amount 30 ms: 30 x 5461 / 8192 = 19.9987 ms
    assert!(
        scenario(
            "{ delay: 20ms, jitter: { amount: 30ms, distribution: pareto } }",
            ""
        )
        .is_ok()
    );
    let refused = scenario(
        "{ delay: 19ms, jitter: { amount: 30ms, distribution: pareto } }",
        "",
    )
    .unwrap_err();
    assert!(
        refused.contains("must be at least 0.67 times the jitter"),
        "{refused}"
    );
    // paretonormal, amount 10 ms: 15.02 ms
    assert!(
        scenario(
            "{ delay: 16ms, jitter: { amount: 10ms, distribution: paretonormal } }",
            ""
        )
        .is_ok()
    );
    let refused = scenario(
        "{ delay: 15ms, jitter: { amount: 10ms, distribution: paretonormal } }",
        "",
    )
    .unwrap_err();
    assert!(refused.contains("1.50 times"), "{refused}");
}

#[test]
fn a_table_needs_its_file_and_nothing_else_has_one() {
    let missing = scenario(
        "{ delay: 40ms, jitter: { amount: 5ms, distribution: table } }",
        "",
    )
    .unwrap_err();
    assert!(
        missing.contains("needs a `table` with the `file` of samples"),
        "{missing}"
    );
    let stray = scenario(
        "{ delay: 40ms, jitter: { amount: 5ms, distribution: normal, table: { file: d.txt } } }",
        "",
    )
    .unwrap_err();
    assert!(
        stray.contains("a `table` belongs to `distribution: table`, and this jitter has `normal`"),
        "{stray}"
    );
    let empty = scenario(
        "{ delay: 40ms, jitter: { amount: 5ms, distribution: table, table: {} } }",
        "",
    )
    .unwrap_err();
    assert!(empty.contains("needs a `file` of samples"), "{empty}");
    let ok = scenario(
        "{ delay: 40ms, jitter: { amount: 5ms, distribution: table, table: { file: d.txt } } }",
        "",
    )
    .unwrap();
    assert_eq!(ok.unresolved_table_files(), vec!["d.txt".to_owned()]);
}

#[test]
fn reading_the_file_fills_the_table_and_the_scenario_then_runs_without_it() {
    let mut parsed = scenario(
        "{ delay: 80ms, jitter: { amount: 20ms, distribution: table, table: { file: delays.txt } } }",
        "",
    )
    .unwrap();
    let reads = Cell::new(0);
    let problems = parsed.resolve_tables(|path| {
        reads.set(reads.get() + 1);
        assert_eq!(path, "delays.txt");
        Ok(uniform_samples(1000))
    });
    assert_eq!(problems, Vec::new(), "{problems:?}");
    assert_eq!(reads.get(), 1);
    assert_eq!(parsed.unresolved_table_files(), Vec::<String>::new());
    let table = parsed.topology.links[0]
        .forward
        .as_ref()
        .unwrap()
        .jitter
        .as_ref()
        .unwrap()
        .table
        .clone()
        .unwrap();
    assert_eq!(table.samples, Some(1000));
    assert_eq!(table.sha256.as_ref().map(String::len), Some(64));
    assert_eq!(table.values.as_ref().unwrap().entries().len(), 4096);
    // The canonical form holds the table, so reading it again needs no file (a recorded scenario replays)
    let yaml = parsed.to_yaml().unwrap();
    let again = Scenario::from_yaml(&yaml, "scenario.yaml").unwrap();
    assert_eq!(again, parsed);
    assert_eq!(again.unresolved_table_files(), Vec::<String>::new());
    // A change of the file is a change of the scenario: another content gives another fingerprint in the canonical form
    let mut other = scenario(
        "{ delay: 80ms, jitter: { amount: 20ms, distribution: table, table: { file: delays.txt } } }",
        "",
    )
    .unwrap();
    assert_eq!(
        other.resolve_tables(|_| Ok(uniform_samples(999))),
        Vec::new()
    );
    assert_ne!(other.to_yaml().unwrap(), yaml);
}

/// The file is read once even if several links name it, and each file by the path that is written.
#[test]
fn a_file_that_two_links_name_is_read_once() {
    let text = format!(
        "{HEAD}      forward: {{ delay: 80ms, jitter: {{ amount: 20ms, distribution: table, table: {{ file: d.txt }} }} }}\n      reverse: {{ delay: 80ms, jitter: {{ amount: 10ms, distribution: table, table: {{ file: d.txt }} }} }}\n{TAIL}"
    );
    let mut parsed = Scenario::from_yaml(&text, "t.yaml").unwrap();
    let reads = Cell::new(0);
    let problems = parsed.resolve_tables(|_| {
        reads.set(reads.get() + 1);
        Ok(uniform_samples(500))
    });
    assert_eq!(problems, Vec::new());
    assert_eq!(reads.get(), 1);
}

#[test]
fn a_file_that_cannot_be_used_is_a_problem_at_the_table_and_the_scenario_stays_as_it_was() {
    let source = "{ delay: 80ms, jitter: { amount: 20ms, distribution: table, table: { file: delays.txt } } }";
    for (content, expected) in [
        (
            Err("cannot read 'delays.txt': No such file or directory".to_owned()),
            "No such file",
        ),
        (Ok(b"1\n2\nx\n".to_vec()), "line 3: 'x' is not a number"),
        (Ok(b"1\n2\n3\n".to_vec()), "3 samples are too few"),
        (Ok(vec![0xff, 0xfe, 0x00]), "is not text in UTF-8"),
        (Ok(b"5\n".repeat(300)), "do not vary"),
    ] {
        let mut parsed = scenario(source, "").unwrap();
        let before = parsed.clone();
        let problems = parsed.resolve_tables(|_| content.clone());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].message.contains(expected),
            "{}",
            problems[0].message
        );
        assert_eq!(
            problems[0].path.as_deref(),
            Some("topology.links[0].forward.jitter.table.file")
        );
        assert_eq!(parsed, before, "{expected}");
    }
}

/// A table with a `sha256` is a promise about the file. Canary: the right fingerprint passes, the wrong one is refused.
#[test]
fn a_pinned_fingerprint_must_match_the_file() {
    let samples = uniform_samples(400);
    let hash = netscenario_model::sha256_hex(&samples);
    let with = |hash: &str| {
        scenario(
            &format!("{{ delay: 80ms, jitter: {{ amount: 20ms, distribution: table, table: {{ file: d.txt, sha256: {hash} }} }} }}"),
            "",
        )
        .unwrap()
    };
    let mut right = with(&hash);
    assert_eq!(right.resolve_tables(|_| Ok(samples.clone())), Vec::new());
    let mut wrong = with(&"0".repeat(64));
    let problems = wrong.resolve_tables(|_| Ok(samples.clone()));
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0]
            .message
            .contains("is not the file the scenario was written for"),
        "{}",
        problems[0].message
    );
    assert!(problems[0].message.contains(&hash));
}

/// The delay must reach as far below as the table does, which is known after the file is read: samples between 10 and 50 are a
/// straight line from -1.73 to 1.73 standard deviations, so 20 ms of jitter needs a delay of 34.7 ms.
#[test]
fn the_reach_of_a_table_made_from_samples_is_checked_when_it_is_read() {
    let ok = |delay: &str| {
        let mut parsed = scenario(
            &format!("{{ delay: {delay}, jitter: {{ amount: 20ms, distribution: table, table: {{ file: d.txt }} }} }}"),
            "",
        )
        .unwrap();
        parsed.resolve_tables(|_| Ok(uniform_samples(1000)))
    };
    assert_eq!(ok("35ms"), Vec::new());
    let problems = ok("30ms");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0]
            .message
            .contains("must be at least 1.73 times the jitter"),
        "{}",
        problems[0].message
    );
}

const STEP_BASE: &str =
    "{ delay: 80ms, jitter: { amount: 20ms, distribution: table, table: { file: d.txt } } }";

#[test]
fn a_step_changes_the_amount_of_a_table_and_the_table_stays_the_one_of_the_link() {
    let mut parsed = scenario(
        STEP_BASE,
        "timeline:\n  - { at: 5s, link: l, set: { forward: { jitter: { amount: 10ms, distribution: table } } } }\n",
    )
    .unwrap();
    assert_eq!(
        parsed.resolve_tables(|_| Ok(uniform_samples(500))),
        Vec::new()
    );
    let link = &parsed.topology.links[0];
    let change = parsed.timeline[0]
        .set
        .as_ref()
        .unwrap()
        .forward
        .clone()
        .unwrap();
    let after = change.applied_to(link.forward.as_ref().unwrap());
    let jitter = after.jitter.unwrap();
    assert_eq!(jitter.amount.as_nanos(), 10_000_000);
    assert_eq!(
        jitter.table,
        link.forward
            .as_ref()
            .unwrap()
            .jitter
            .as_ref()
            .unwrap()
            .table
    );
    assert!(matches!(change.jitter, Edit::Set(_)));
}

#[test]
fn a_step_cannot_bring_a_table_and_cannot_use_one_the_link_does_not_have() {
    let own = scenario(
        STEP_BASE,
        "timeline:\n  - { at: 5s, link: l, set: { forward: { jitter: { amount: 10ms, distribution: table, table: { file: e.txt } } } } }\n",
    )
    .unwrap_err();
    assert!(own.contains("a step cannot bring a `table`"), "{own}");
    let none = scenario(
        "{ delay: 80ms, jitter: { amount: 10ms, distribution: normal } }",
        "timeline:\n  - { at: 5s, link: l, set: { forward: { jitter: { amount: 10ms, distribution: table } } } }\n",
    )
    .unwrap_err();
    assert!(
        none.contains("needs a `table` with the `file` of samples"),
        "{none}"
    );
}
