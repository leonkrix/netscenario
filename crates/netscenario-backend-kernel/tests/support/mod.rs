//! What the tests of the Kernel backend share. They need user namespaces, so they say so and stop where
//! the machine does not allow them; on the CI (`CI` is set) a machine without them is a failure.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use netscenario_backend_kernel::{KernelBackend, KernelOptions, Toolbox};
use netscenario_model::Scenario;

pub(crate) fn supervisor_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_netscenario-kernel-supervisor"))
}

/// The agent, which is built next to the supervisor when the packages of the workspace are tested together
/// (`just test`, `just test-kernel`). A test that needs it stops where it is missing, except on the CI.
pub(crate) fn agent_exe() -> Option<PathBuf> {
    let path = supervisor_exe().with_file_name("netscenario-agent");
    if path.is_file() {
        return Some(path);
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "this test must run on the CI, but the agent is not built at {}",
        path.display()
    );
    eprintln!(
        "SKIPPED: the agent is not built at {} (use `just test-kernel`)",
        path.display()
    );
    None
}

/// Whether this machine lets the tests create the namespaces of a run. If not, the test prints why and
/// returns, except on the CI, where it fails: a test that never runs there proves nothing.
pub(crate) fn namespaces_available() -> bool {
    let Ok(tools) = Toolbox::locate() else {
        return unavailable("ip, tc, unshare or nsenter is not installed");
    };
    let probe = Command::new(&tools.unshare)
        .args([
            "--user",
            "--map-root-user",
            "--net",
            "--mount",
            "--pid",
            "--fork",
            "--kill-child",
            "--mount-proc",
            "true",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    if probe.status.success() {
        true
    } else {
        unavailable(&format!(
            "unprivileged user namespaces are not allowed here ({})",
            String::from_utf8_lossy(&probe.stderr).trim()
        ))
    }
}

fn unavailable(why: &str) -> bool {
    assert!(
        std::env::var_os("CI").is_none(),
        "this test must run on the CI, but: {why}"
    );
    eprintln!("SKIPPED: {why}");
    false
}

pub(crate) fn backend(options: impl FnOnce(&mut KernelOptions)) -> KernelBackend {
    let mut o = KernelOptions::new(supervisor_exe());
    o.max_run_time = Duration::from_secs(30);
    options(&mut o);
    KernelBackend::new(o).unwrap()
}

/// A scenario of two nodes with one link that has the lines `link` (indented as the keys of a link) and a
/// `head` of top-level lines such as `seed: 1`.
pub(crate) fn scenario(head: &str, link: &str) -> Scenario {
    let yaml = format!(
        "version: 0\nname: t\nduration: 10s\n{head}topology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }}\n"
    );
    Scenario::from_yaml(&yaml, "t.yaml").unwrap()
}

/// The command lines of the processes whose command line contains `needle`.
pub(crate) fn processes_with(needle: &str) -> Vec<String> {
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|c| c.is_ascii_digit())
        })
        .filter(|entry| entry.file_name().to_string_lossy() != me)
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .filter(|line| line.contains(needle))
        .collect()
}

/// Waits until a process whose command line contains `needle` exists. A process that was just started has the
/// command line of its parent until it has called `exec`, and on a busy machine that takes a while.
pub(crate) fn assert_started(needle: &str, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while processes_with(needle).is_empty() {
        assert!(
            Instant::now() < deadline,
            "{context}: no process with '{needle}' appeared"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until no process of the run is left. The kernel ends them when the supervisor dies, but not at the
/// same instant.
pub(crate) fn assert_nothing_left(run_id: &str, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = processes_with(run_id);
        if left.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{context}: processes are left: {left:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A directory for the files that programs of a test write.
pub(crate) fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("netscenario-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The text of `file` once it contains `needle`.
pub(crate) fn wait_for(file: &Path, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} never contained '{needle}': '{text}'",
            file.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn tools() -> Toolbox {
    Toolbox::locate().unwrap()
}

/// The path of a program on the `PATH`, if there is one.
pub(crate) fn program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Whether this process is root (an effective user id of 0), from `/proc/self/status`.
pub(crate) fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1).map(|euid| euid == "0"))
        .unwrap_or(false)
}
