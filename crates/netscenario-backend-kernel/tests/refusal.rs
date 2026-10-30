//! What a user sees when the Kernel backend cannot reproduce a scenario on this machine.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_backend_kernel::{Environment, Tool, Version, capabilities};
use netscenario_core::check;
use netscenario_model::Scenario;

const SEEDED: &str = "version: 0
name: seeded
seed: 1234
duration: 10s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 20ms, loss: { model: random, p: 1% } }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
";

fn scenario(text: &str) -> Scenario {
    Scenario::from_yaml(text, "seeded.yaml").expect("a valid scenario")
}

#[test]
fn a_seed_with_an_old_tc_is_refused_with_the_way_out() {
    let old = Environment {
        tc: Tool::Found(Version::new(6, 1, 0)),
    };
    let error = check(&scenario(SEEDED), &capabilities(&old), false).unwrap_err();
    assert_eq!(
        error.to_string(),
        "error: the kernel backend cannot reproduce 'Same seed, same loss pattern': the `tc` of this machine is iproute2 6.1.0, and `seed` needs 6.6.0 or newer\n  = path: seed\n  = help: install iproute2 6.6.0 or newer, leave out `seed` (the run is then not reproducible and says so), or use the Engine backend (planned for release 0.4)\n"
    );
}

/// The same scenario without a seed is not refused: it asks for no reproducibility.
#[test]
fn the_same_scenario_without_a_seed_runs_with_an_old_tc() {
    let old = Environment {
        tc: Tool::Found(Version::new(6, 1, 0)),
    };
    let without_seed = SEEDED.replace("seed: 1234\n", "");
    assert!(check(&scenario(&without_seed), &capabilities(&old), false).is_ok());
}

#[test]
fn a_complete_machine_accepts_the_seeded_scenario() {
    assert!(
        check(
            &scenario(SEEDED),
            &capabilities(&Environment::complete()),
            false
        )
        .is_ok()
    );
}
