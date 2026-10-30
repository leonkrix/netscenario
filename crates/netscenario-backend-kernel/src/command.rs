//! Running the tools, with errors that say what was run and what the tool answered.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The command line as text, for messages.
pub(crate) fn line(program: &Path, args: &[&str]) -> String {
    let mut text = program.file_name().map_or_else(
        || program.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    for arg in args {
        text.push(' ');
        text.push_str(arg);
    }
    text
}

/// Runs `program` and returns what it printed on its standard output.
pub(crate) fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run `{}`: {error}", line(program, args)))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "`{}` failed ({}): {}",
            line(program, args),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The directories where the tools live. `tc` is in `/usr/sbin`, which an ordinary user often has not on the
/// `PATH` (Debian does not, see the spikes).
const EXTRA_DIRECTORIES: &[&str] = &["/usr/sbin", "/sbin", "/usr/local/sbin", "/usr/bin", "/bin"];

/// The path of the program called `name`.
pub(crate) fn find(name: &str) -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&from_path)
        .chain(EXTRA_DIRECTORIES.iter().map(PathBuf::from))
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// The tools that the Kernel backend calls (D-009, D-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolbox {
    /// `ip` of iproute2.
    pub ip: PathBuf,
    /// `tc` of iproute2.
    pub tc: PathBuf,
    /// `unshare` of util-linux.
    pub unshare: PathBuf,
    /// `nsenter` of util-linux.
    pub nsenter: PathBuf,
    /// `prlimit` of util-linux: limits the memory of a program (SAF-02).
    pub prlimit: PathBuf,
}

impl Toolbox {
    /// Finds the tools.
    ///
    /// # Errors
    ///
    /// The names of the tools that were not found.
    pub fn locate() -> Result<Self, Vec<&'static str>> {
        let ip = find("ip");
        let tc = find("tc");
        let unshare = find("unshare");
        let nsenter = find("nsenter");
        let prlimit = find("prlimit");
        match (ip, tc, unshare, nsenter, prlimit) {
            (Some(ip), Some(tc), Some(unshare), Some(nsenter), Some(prlimit)) => Ok(Self {
                ip,
                tc,
                unshare,
                nsenter,
                prlimit,
            }),
            (ip, tc, unshare, nsenter, prlimit) => Err([
                ("ip", ip.is_none()),
                ("tc", tc.is_none()),
                ("unshare", unshare.is_none()),
                ("nsenter", nsenter.is_none()),
                ("prlimit", prlimit.is_none()),
            ]
            .into_iter()
            .filter_map(|(name, missing)| missing.then_some(name))
            .collect()),
        }
    }
}

/// The capability to create virtual Ethernet pairs and to set the queues on them (`CAP_NET_ADMIN`).
const CAP_NET_ADMIN: u32 = 12;
/// The capability to create namespaces and to enter them (`CAP_SYS_ADMIN`).
const CAP_SYS_ADMIN: u32 = 21;

/// Whether a process with this `/proc/<pid>/status` may create the namespaces of a run with its own rights,
/// without a user namespace: it is root (an effective user id of 0), or it has `CAP_NET_ADMIN` and
/// `CAP_SYS_ADMIN` in its **ambient** set.
///
/// The ambient set is the one that survives `exec`: a run starts `unshare`, and `unshare` starts the
/// supervisor, and each of them has only what the one before passed on. Capabilities in the permitted or
/// effective set alone (for example from `setcap` on the file) are gone after the first `exec`.
pub(crate) fn own_rights_in(status: &str) -> bool {
    let field = |name: &str| status.lines().find_map(|line| line.strip_prefix(name));
    let root = field("Uid:")
        .and_then(|ids| ids.split_whitespace().nth(1))
        .is_some_and(|euid| euid == "0");
    let ambient = field("CapAmb:")
        .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .is_some_and(|mask| {
            [CAP_NET_ADMIN, CAP_SYS_ADMIN]
                .iter()
                .all(|capability| mask & (1 << capability) != 0)
        });
    root || ambient
}

/// Whether this process has the rights for [`own_rights_in`], from `/proc/self/status`. No `unsafe` call to
/// `geteuid` or `capget`.
pub(crate) fn has_own_rights() -> bool {
    std::fs::read_to_string("/proc/self/status").is_ok_and(|status| own_rights_in(&status))
}

/// The way for a run, from what the process has and what works here: its own rights if it has them and they
/// work, else a user namespace. A process without rights of its own never tries them. If neither works, the
/// rights of the process are kept, so that the run fails with the answer of `unshare`.
///
/// `works(user_namespace)` tries the namespaces of a run once; it is only called when the process has rights.
pub(crate) fn uses_own_rights(has_own_rights: bool, works: impl Fn(bool) -> bool) -> bool {
    has_own_rights && (works(false) || !works(true))
}

/// The flags of `unshare` for the namespaces of a run: the user namespace (in which the process is root only
/// inside) if `user_namespace`, then the network, mount and PID namespaces with the supervisor as the first
/// process, which the kernel ends with everything in it when it dies (D-022).
pub(crate) fn namespace_flags(user_namespace: bool) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if user_namespace {
        flags.extend(["--user", "--map-root-user"]);
    }
    flags.extend([
        "--net",
        "--mount",
        "--pid",
        "--fork",
        "--kill-child",
        "--mount-proc",
    ]);
    flags
}

/// Whether `unshare` creates the namespaces of a run with these flags, here, now.
pub(crate) fn namespaces_work(unshare: &Path, user_namespace: bool) -> bool {
    let mut arguments = namespace_flags(user_namespace);
    arguments.extend(["--", "true"]);
    run(unshare, &arguments).is_ok()
}

/// The places where a tool is looked for, as text for a message: the `PATH` and the directories that are added.
pub(crate) fn searched() -> String {
    let from_path = std::env::var("PATH").unwrap_or_default();
    format!("PATH ({from_path}) and {}", EXTRA_DIRECTORIES.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_command_says_what_it_ran_and_what_it_answered() {
        let error = run(Path::new("/bin/sh"), &["-c", "echo no good >&2; exit 3"]).unwrap_err();
        assert!(
            error.contains("`sh -c echo no good >&2; exit 3` failed"),
            "{error}"
        );
        assert!(
            error.contains("exit status: 3") && error.ends_with("no good"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_program_says_so() {
        let error = run(Path::new("/nonexistent/tool"), &["x"]).unwrap_err();
        assert!(error.starts_with("cannot run `tool x`"), "{error}");
    }

    #[test]
    fn the_output_of_a_command_is_returned() {
        assert_eq!(
            run(Path::new("/bin/sh"), &["-c", "printf abc"]).as_deref(),
            Ok("abc")
        );
    }

    #[test]
    fn a_program_that_does_not_exist_is_not_found() {
        assert_eq!(find("no-such-program-netscenario"), None);
        assert!(find("sh").is_some());
    }

    /// A `/proc/<pid>/status` with this user id and these ambient capabilities (the lines that matter).
    fn status(euid: u32, ambient: u64) -> String {
        format!("Name:\tx\nUid:\t{euid}\t{euid}\t{euid}\t{euid}\nCapAmb:\t{ambient:016x}\n")
    }

    const NET_ADMIN: u64 = 1 << 12;
    const SYS_ADMIN: u64 = 1 << 21;

    #[test]
    fn root_has_rights_of_its_own() {
        assert!(own_rights_in(&status(0, 0)));
    }

    #[test]
    fn an_ordinary_user_with_both_ambient_capabilities_has_them_too() {
        assert!(own_rights_in(&status(1000, NET_ADMIN | SYS_ADMIN)));
        // Other capabilities in the set do not matter
        assert!(own_rights_in(&status(1000, NET_ADMIN | SYS_ADMIN | 1 << 7)));
    }

    #[test]
    fn canary_one_capability_is_not_enough_and_neither_is_none() {
        assert!(!own_rights_in(&status(1000, NET_ADMIN)));
        assert!(!own_rights_in(&status(1000, SYS_ADMIN)));
        assert!(!own_rights_in(&status(1000, 0)));
        // 0x201000 is the mask of the two; shifted by one bit it is two other capabilities
        assert!(!own_rights_in(&status(1000, (NET_ADMIN | SYS_ADMIN) << 1)));
    }

    #[test]
    fn a_status_that_cannot_be_read_gives_no_rights() {
        assert!(!own_rights_in(""));
        assert!(!own_rights_in(
            "Uid:\t1000\t1000\t1000\t1000\nCapAmb:\tnot hex\n"
        ));
        // An old kernel has no CapAmb line
        assert!(!own_rights_in("Uid:\t1000\t1000\t1000\t1000\n"));
        // The effective user id counts, not the real one
        assert!(own_rights_in("Uid:\t1000\t0\t1000\t0\n"));
        assert!(!own_rights_in("Uid:\t0\t1000\t0\t1000\n"));
    }

    #[test]
    fn the_way_follows_what_the_process_has_and_what_works() {
        use std::cell::Cell;
        // No rights of its own: the user namespace, and nothing is tried
        let tried = Cell::new(0);
        assert!(!uses_own_rights(false, |_| {
            tried.set(tried.get() + 1);
            true
        }));
        assert_eq!(tried.get(), 0);
        // Rights that work are used, also when the user namespace would work too
        assert!(uses_own_rights(true, |_| true));
        // Rights that are refused (root in a container without SYS_ADMIN) give way to a user namespace
        assert!(!uses_own_rights(true, |user_namespace| user_namespace));
        // If nothing works, the rights are kept, and the run fails with the answer of unshare
        assert!(uses_own_rights(true, |_| false));
    }

    #[test]
    fn the_flags_of_a_run_have_the_user_namespace_only_when_asked() {
        assert!(namespace_flags(true).starts_with(&["--user", "--map-root-user"]));
        assert!(!namespace_flags(false).contains(&"--user"));
        for flags in [namespace_flags(true), namespace_flags(false)] {
            assert!(
                flags.contains(&"--net")
                    && flags.contains(&"--pid")
                    && flags.contains(&"--kill-child")
            );
        }
    }
}
