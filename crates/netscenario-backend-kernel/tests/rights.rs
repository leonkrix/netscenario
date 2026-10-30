//! A process that is not root but has `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its ambient set runs with its own
//! rights, as `doctor` says it can (D-035). Giving a process capabilities needs root, so this test runs in the
//! CI step that runs the tests as root; as an ordinary user it says so and stops.
//!
//! The test starts its own executable again as the ordinary user who started `sudo`, with the capabilities that the case
//! names, and the second process checks which way its backend chose and that the way works.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod support;

use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};

use netscenario_core::Backend;
use netscenario_model::Name;
use support::{backend, is_root, program};

/// The environment variable that makes the second process run its check, with the way it must have chosen.
const EXPECT: &str = "NETSCENARIO_RIGHTS_EXPECT";

/// Runs this test executable as its owner with `ambient` capabilities and returns what it printed if it
/// succeeded, else what went wrong. `None` if this machine cannot do that.
fn as_owner_with(ambient: &str, expect: &str) -> Option<Result<(), String>> {
    if !is_root() {
        return unavailable(
            "this test needs root, to give a process capabilities (the CI runs it as root)",
        );
    }
    let Some(setpriv) = program("setpriv") else {
        return unavailable("setpriv (util-linux) is not installed");
    };
    let exe = std::env::current_exe().unwrap();
    let Some((uid, gid)) = ordinary_user(&exe) else {
        return unavailable(
            "there is no ordinary user to run the test as (no SUDO_UID, and root owns the executable)",
        );
    };
    let mut command = Command::new(setpriv);
    command
        .arg(format!("--reuid={uid}"))
        .arg(format!("--regid={gid}"))
        .arg("--clear-groups");
    if !ambient.is_empty() {
        command
            .arg(format!("--inh-caps={}", plus(ambient)))
            .arg(format!("--ambient-caps={}", plus(ambient)));
    }
    let output = command
        .args(["--", exe.to_str().unwrap()])
        .args([
            "--exact",
            "the_second_process_checks_its_way",
            "--nocapture",
        ])
        .env(EXPECT, expect)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Some(if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    })
}

/// The user and group to run the second process as: the user who started `sudo`, else the owner of the test
/// executable. It must be someone who can reach the executable and the supervisor in `target/`. Never root.
fn ordinary_user(exe: &std::path::Path) -> Option<(u32, u32)> {
    let from_sudo = |name: &str| std::env::var(name).ok()?.parse::<u32>().ok();
    let (uid, gid) = if let (Some(uid), Some(gid)) = (from_sudo("SUDO_UID"), from_sudo("SUDO_GID"))
    {
        (uid, gid)
    } else {
        let owner = std::fs::metadata(exe).ok()?;
        (owner.uid(), owner.gid())
    };
    (uid != 0).then_some((uid, gid))
}

/// `net_admin,sys_admin` as `+net_admin,+sys_admin`, the form that `setpriv` takes.
fn plus(names: &str) -> String {
    names
        .split(',')
        .map(|name| format!("+{name}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Where this test cannot run it says so and returns, except on the CI, where a test that never runs proves
/// nothing.
fn unavailable(why: &str) -> Option<Result<(), String>> {
    assert!(
        std::env::var_os("CI").is_none() || !is_root(),
        "this test must run on the CI as root, but: {why}"
    );
    eprintln!("SKIPPED: {why}");
    None
}

/// The second process: it runs only when the first one starts it, and checks that the backend chose the way
/// that the case expects and that the namespaces of that way can be created.
#[test]
fn the_second_process_checks_its_way() {
    let Ok(expected) = std::env::var(EXPECT) else {
        return;
    };
    let mut backend = backend(|_| {});
    let details = backend.describe().details;
    assert_eq!(
        details.get("rights").map(String::as_str),
        Some(expected.as_str()),
        "the way of the run"
    );
    // The way works: a node is a network namespace, created by the supervisor that this way started
    backend.create_node(&Name::new("a").unwrap()).unwrap();
    backend.teardown().unwrap();
}

#[test]
fn an_ordinary_user_with_the_two_ambient_capabilities_runs_with_its_own_rights() {
    if let Some(result) = as_owner_with("net_admin,sys_admin", "own rights") {
        result.unwrap();
    }
}

#[test]
fn canary_one_capability_is_not_enough() {
    // Only `CAP_NET_ADMIN`: the run must not take the rights of the process, and uses a user namespace
    if let Some(result) = as_owner_with("net_admin", "user namespace") {
        result.unwrap();
    }
}

#[test]
fn canary_an_ordinary_user_without_capabilities_uses_a_user_namespace() {
    if let Some(result) = as_owner_with("", "user namespace") {
        result.unwrap();
    }
}

#[test]
fn canary_the_check_of_the_second_process_can_fail() {
    // The second process has no capabilities and is told to expect its own rights: it must fail
    if let Some(result) = as_owner_with("", "own rights") {
        assert!(
            result.is_err(),
            "the second process passed with a wrong expectation: the check does not check"
        );
    }
}
