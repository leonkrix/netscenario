//! `netscenario clean` (BKD-04): it ends the processes of a run that lost its supervisor, and never touches a
//! run that has one. The leftover is made the way an older version could have left one: a holder of a network
//! namespace that was started without the PID namespace of a run, so that nothing ends it when its supervisor
//! is gone.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use netscenario_backend_kernel::clean::{clean, clean_run};
use netscenario_core::Backend;
use netscenario_model::Name;
use support::{
    assert_nothing_left, backend, is_root, namespaces_available, processes_with, supervisor_exe,
    tools,
};

/// A holder with no supervisor, as a leftover looks. Returns when it says that its namespace is ready. It is
/// started the way a run starts: in a user namespace for an unprivileged user, with its own rights for root. A
/// root that went into a user namespace could not even start the test binary from a home directory that only
/// its owner may enter (`CAP_DAC_OVERRIDE` does not reach files whose owner is not mapped there).
fn leftover_holder(run_id: &str) -> Child {
    let mut command = Command::new(tools().unshare);
    if !is_root() {
        command.args(["--user", "--map-root-user"]);
    }
    let mut child = command
        .args(["--net", "--"])
        .arg(supervisor_exe())
        .args(["hold", run_id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if !ready.contains("ready") {
        let mut said = String::new();
        let _ = child.stderr.take().unwrap().read_to_string(&mut said);
        panic!("the holder did not start: '{ready}'; it said: '{said}'");
    }
    child
}

fn wait_until_gone(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the process is still there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_run_without_a_supervisor_is_found_and_ended() {
    if !namespaces_available() {
        return;
    }
    let run_id = format!("{}-7-424242", std::process::id());
    let mut holder = leftover_holder(&run_id);
    let pid = holder.id();

    // A dry run says what it would do and does nothing
    let report = clean_run(&run_id, true);
    let found = report
        .found
        .iter()
        .find(|leftover| leftover.run_id == run_id)
        .expect("the dry run does not find the leftover");
    assert_eq!(found.pids, [pid]);
    assert!(report.ended.is_empty() && report.dry_run);
    assert!(holder.try_wait().unwrap().is_none(), "a dry run ended it");

    let report = clean_run(&run_id, false);
    assert!(report.ended.contains(&pid), "{report:?}");
    assert!(report.succeeded(), "{report:?}");
    wait_until_gone(&mut holder);
    assert_nothing_left(&run_id, "after clean");

    // A second call has nothing left to do for this run
    assert_eq!(clean_run(&run_id, true).found, []);
}

/// Canary for the rule that matters most: a run with a living supervisor is never touched, and a command line
/// that only looks similar is not ours.
#[test]
fn a_run_with_a_supervisor_is_left_alone() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    backend.create_node(&Name::new("a").unwrap()).unwrap();
    let run_id = backend.run_id().to_owned();

    let report = clean(false);
    assert!(
        report
            .found
            .iter()
            .all(|leftover| leftover.run_id != run_id),
        "the live run was reported: {report:?}"
    );
    assert!(report.live_runs >= 1, "{report:?}");
    assert!(
        !processes_with(&run_id).is_empty(),
        "the live run was ended"
    );
    // It still works
    backend.create_node(&Name::new("b").unwrap()).unwrap();
    backend.teardown().unwrap();
}
