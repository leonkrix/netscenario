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

/// Whether this process is root, from `/proc/self/status` (no `unsafe` call to `geteuid`).
pub(crate) fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|ids| ids.split_whitespace().nth(1).map(|euid| euid == "0"))
        })
        .unwrap_or(false)
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
}
