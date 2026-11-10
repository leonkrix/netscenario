//! The profile library (SCN-06, D-095): a link names a profile, the reader replaces it by its values, the keys of the file win,
//! and a scenario that records a library is not resolved again.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{Scenario, library};

fn scenario(link: &str, head: &str) -> String {
    format!(
        "version: 0\nname: t\n{head}duration: 5s\ntopology:\n  nodes: [terminal, network]\n  links:\n    - id: access\n      between: [terminal, network]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: terminal, to: network, rate: 1mbit, packet-size: 1200b }}\n"
    )
}

fn read(link: &str, head: &str) -> Result<Scenario, String> {
    Scenario::from_yaml(&scenario(link, head), "t.yaml").map_err(|error| error.to_string())
}

fn ms(duration: Option<netscenario_model::Duration>) -> f64 {
    std::time::Duration::from(duration.unwrap()).as_secs_f64() * 1000.0
}

#[test]
fn a_profile_is_replaced_by_its_values_and_kept_as_the_record() {
    let scenario = read("      profile: lte-good\n", "").unwrap();
    let link = &scenario.topology.links[0];
    assert_eq!(link.profile.as_deref(), Some("lte-good"));
    // `forward` is up, from the first node of `between`
    assert!((ms(link.forward.as_ref().unwrap().delay) - 24.0).abs() < 1e-9);
    assert!((ms(link.reverse.as_ref().unwrap().delay) - 8.0).abs() < 1e-9);
    assert_eq!(scenario.profile_library, Some(library().version()));
}

#[test]
fn the_keys_of_the_file_win_key_by_key() {
    // `both` is for both directions and wins; what it does not write comes from the profile (the rate is downstream only)
    let scenario = read("      profile: geo-good\n      both: { delay: 50ms }\n", "").unwrap();
    let link = &scenario.topology.links[0];
    let (forward, reverse) = (
        link.forward.as_ref().unwrap(),
        link.reverse.as_ref().unwrap(),
    );
    assert!((ms(forward.delay) - 50.0).abs() < 1e-9);
    assert!((ms(reverse.delay) - 50.0).abs() < 1e-9);
    assert!(forward.loss.is_some() && reverse.loss.is_some());
    assert!(forward.rate.is_none());
    assert_eq!(reverse.rate.unwrap().to_string(), "144mbit");
    assert!(link.both.is_none());

    // A direction of its own: its keys win in that direction only
    let scenario = read(
        "      profile: lte-good\n      forward: { delay: 99ms, rate: 2mbit }\n",
        "",
    )
    .unwrap();
    let link = &scenario.topology.links[0];
    assert!((ms(link.forward.as_ref().unwrap().delay) - 99.0).abs() < 1e-9);
    assert!(link.forward.as_ref().unwrap().rate.is_some());
    assert!((ms(link.reverse.as_ref().unwrap().delay) - 8.0).abs() < 1e-9);
    assert!(link.reverse.as_ref().unwrap().rate.is_none());
}

#[test]
fn a_profile_that_does_not_exist_is_refused_with_the_closest_one() {
    let message = read("      profile: lte-typcal\n", "").unwrap_err();
    assert!(
        message.contains("no profile called 'lte-typcal'"),
        "{message}"
    );
    assert!(message.contains("did you mean 'lte-typical'"), "{message}");
    assert!(message.contains("netscenario profiles"), "{message}");
    assert!(message.contains(":9:"), "the line of the key: {message}");
    let message = read("      profile: nothing-like-it\n", "").unwrap_err();
    assert!(message.contains("no profile called"), "{message}");
    assert!(!message.contains("did you mean"), "{message}");
}

#[test]
fn both_with_a_direction_stays_an_error_of_its_own_next_to_a_profile() {
    let message = read(
        "      profile: lte-good\n      both: { delay: 5ms }\n      forward: { delay: 6ms }\n",
        "",
    )
    .unwrap_err();
    assert!(message.contains("both"), "{message}");
}

#[test]
fn the_written_scenario_is_read_back_unchanged() {
    let first = read(
        "      profile: geo-good\n      reverse: { loss: { model: random, p: 1% } }\n",
        "",
    )
    .unwrap();
    let text = first.to_yaml().unwrap();
    assert!(text.contains("profile-library"), "{text}");
    let second = Scenario::from_yaml(&text, "again.yaml").unwrap();
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}

/// Canary: a scenario that records the library is not resolved again. Its values are what the run used, so a library of a later
/// release cannot change a replay. Without the key the same text is resolved, so the test can fail.
#[test]
fn a_recorded_library_keeps_the_values_of_the_scenario() {
    let link = "      profile: lte-good\n      forward: { delay: 99ms }\n";
    let recorded = read(link, "profile-library: 1\n").unwrap();
    let forward = recorded.topology.links[0].forward.as_ref().unwrap();
    assert!((ms(forward.delay) - 99.0).abs() < 1e-9);
    assert!(
        recorded.topology.links[0].reverse.is_none(),
        "not resolved again"
    );
    let fresh = read(link, "").unwrap();
    assert!(fresh.topology.links[0].reverse.is_some());
    assert_ne!(recorded, fresh);
}

#[test]
fn every_profile_makes_a_valid_scenario() {
    for profile in library().profiles() {
        let link = format!("      profile: {}\n", profile.id);
        let scenario = read(&link, "").unwrap_or_else(|error| panic!("{}: {error}", profile.id));
        let link = &scenario.topology.links[0];
        assert_eq!(
            link.forward.clone().unwrap_or_default(),
            profile.forward(),
            "{}",
            profile.id
        );
        assert_eq!(
            link.reverse.clone().unwrap_or_default(),
            profile.reverse(),
            "{}",
            profile.id
        );
    }
}

/// The numbers have a source in the document, and the document has no profile that the library does not have.
#[test]
fn the_library_and_the_document_agree() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/PROFILES.md");
    let text = std::fs::read_to_string(&path).unwrap();
    for profile in library().profiles() {
        assert!(
            text.contains(&format!("`{}`", profile.id)),
            "{} is not in PROFILES.md",
            profile.id
        );
        for source in &profile.sources {
            assert!(
                text.contains(&format!("- **{source}**")),
                "{} cites {source}, which PROFILES.md does not list",
                profile.id
            );
        }
    }
    // Every profile the document names in a table is in the library
    for line in text.lines().filter(|l| l.starts_with("| `")) {
        let id = line.trim_start_matches("| `").split('`').next().unwrap();
        if id.contains('-')
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            assert!(
                library().get(id).is_some(),
                "PROFILES.md names {id}, which the library does not have"
            );
        }
    }
}
