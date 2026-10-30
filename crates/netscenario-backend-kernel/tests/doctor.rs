//! `netscenario doctor` (ENV-04) on the machine that runs the tests. The judging itself is tested with
//! findings of machines that are broken in every way, in the module; here the real probe has to agree with the
//! other tests of this crate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use netscenario_backend_kernel::doctor::{Status, diagnose};
use support::namespaces_available;

fn host_interfaces() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir("/sys/class/net")
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_machine_that_runs_the_other_tests_passes_the_doctor() {
    if !namespaces_available() {
        return;
    }
    let before = host_interfaces();
    let report = diagnose();
    assert!(report.ok, "{}", report.render());
    for id in ["can-create-namespaces", "veth", "netem"] {
        assert_eq!(
            report.check(id).map(|c| c.status),
            Some(Status::Ok),
            "{id}\n{}",
            report.render()
        );
    }
    // The CI has a `tc` with the seed and its tables; a developer's machine may not
    if std::env::var_os("CI").is_some() {
        for id in ["seed", "tables"] {
            assert_eq!(
                report.check(id).map(|c| c.status),
                Some(Status::Ok),
                "{id}\n{}",
                report.render()
            );
        }
    }
    // The probe lives in a namespace of its own: the host has the same interfaces afterwards (SAF-01)
    assert_eq!(host_interfaces(), before);
}

/// The machine tells the truth about the seed: the doctor says Ok exactly when a seeded run can be made.
#[test]
fn the_doctor_and_the_backend_agree_about_the_seed() {
    if !namespaces_available() {
        return;
    }
    let report = diagnose();
    let features = report
        .check("seeded-loss")
        .expect("a line for the seed")
        .status;
    let kernel = report.check("seed").expect("a line for the probe").status;
    assert_eq!(
        features == Status::Ok,
        kernel == Status::Ok,
        "{}",
        report.render()
    );
}
