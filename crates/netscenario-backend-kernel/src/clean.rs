//! `netscenario clean`: a safety net for runs that lost their supervisor (BKD-04, D-022).
//!
//! A run leaves nothing behind when its supervisor dies, because it lives in a PID namespace and the kernel
//! ends every process in it. What can remain is a run that was not started that way (an older version, or a
//! holder started by hand): the processes that hold the network namespaces of its nodes, with no supervisor
//! above them. This module finds them in `/proc` by the shape of their command line and ends them; a network
//! namespace that no process holds goes away by itself. Nothing is named, so there is nothing to find by
//! name, and a run with a living supervisor is never touched.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::command::{find, run};

/// The part a process plays in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The first process of the run: it owns the nodes and the links.
    Supervisor,
    /// A process that holds the network namespace of one node.
    Holder,
}

/// A process that belongs to a run, recognised by its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProcess {
    /// The process id as the host sees it.
    pub pid: u32,
    /// The identifier of the run, as in `supervise <run id>` and `hold <run id>`.
    pub run_id: String,
    /// What the process does in the run.
    pub role: Role,
}

/// The processes of one run that has no supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Leftover {
    /// The identifier of the run.
    pub run_id: String,
    /// The process ids of the holders that are still there.
    pub pids: Vec<u32>,
}

/// What `clean` found and did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct CleanReport {
    /// Runs without a supervisor.
    pub found: Vec<Leftover>,
    /// The processes that were ended (none for a dry run).
    pub ended: Vec<u32>,
    /// The processes that could not be ended, with the reason.
    pub failed: Vec<Failure>,
    /// How many runs have a living supervisor. They are left alone.
    pub live_runs: usize,
    /// Whether nothing was ended on purpose.
    pub dry_run: bool,
}

/// A process that `clean` could not end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// The process id.
    pub pid: u32,
    /// Why it is still there.
    pub reason: String,
}

impl CleanReport {
    /// Whether everything that was found is gone (or, for a dry run, nothing failed).
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.failed.is_empty()
    }

    /// The report as text for people.
    #[must_use]
    pub fn render(&self) -> String {
        let plural = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        let runs = |count: usize| plural(count, "run", "runs");
        let processes = |count: usize| plural(count, "process", "processes");
        let mut text = String::from("netscenario clean\n");
        let alone = if self.live_runs == 0 {
            String::new()
        } else {
            format!(
                " {} with a supervisor {} left alone.",
                runs(self.live_runs),
                if self.live_runs == 1 { "is" } else { "are" }
            )
        };
        if self.found.is_empty() {
            let _ = writeln!(text, "No run without a supervisor was found.{alone}");
            return text;
        }
        let _ = writeln!(
            text,
            "Found {} without a supervisor:",
            runs(self.found.len())
        );
        for leftover in &self.found {
            let pids: Vec<String> = leftover.pids.iter().map(u32::to_string).collect();
            let _ = writeln!(
                text,
                "  {}  {}: {}",
                leftover.run_id,
                processes(leftover.pids.len()),
                pids.join(", ")
            );
        }
        if self.dry_run {
            let total: usize = self.found.iter().map(|l| l.pids.len()).sum();
            let _ = writeln!(
                text,
                "Dry run: {} would be ended, nothing was.{alone}",
                processes(total)
            );
        } else {
            let _ = writeln!(text, "Ended {}.{alone}", processes(self.ended.len()));
        }
        for failure in &self.failed {
            let _ = writeln!(text, "Could not end {}: {}", failure.pid, failure.reason);
        }
        text
    }

    /// The report as JSON, for machines.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
    }
}

/// Whether `id` has the shape of a run id, `<pid>-<counter>-<nanoseconds>`.
fn is_run_id(id: &str) -> bool {
    let mut parts = id.split('-');
    let numbers = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    });
    numbers && parts.next().is_none()
}

/// Recognises a process of a run by its command line: `<program> supervise <run id> <nodes>` or `<program>
/// hold <run id>`, where the program is a binary whose name starts with `netscenario`. Anything else, such as
/// a user's `vim hold 1-2-3`, is not ours and is never touched.
#[must_use]
pub fn parse_run_process(pid: u32, args: &[&str]) -> Option<RunProcess> {
    let (program, rest) = args.split_first()?;
    let name = Path::new(program).file_name()?.to_str()?;
    if !name.starts_with("netscenario") {
        return None;
    }
    let (role, run_id) = match rest {
        ["supervise", run_id, nodes] if nodes.parse::<usize>().is_ok() => {
            (Role::Supervisor, *run_id)
        }
        ["hold", run_id] => (Role::Holder, *run_id),
        _ => return None,
    };
    is_run_id(run_id).then(|| RunProcess {
        pid,
        run_id: run_id.to_owned(),
        role,
    })
}

/// The runs that have processes but no supervisor, and how many runs have one.
#[must_use]
pub fn leftovers(processes: &[RunProcess]) -> (Vec<Leftover>, usize) {
    let mut runs: Vec<&str> = processes.iter().map(|p| p.run_id.as_str()).collect();
    runs.sort_unstable();
    runs.dedup();
    let mut found = Vec::new();
    let mut live = 0;
    for run_id in runs {
        let of_run = || processes.iter().filter(|p| p.run_id == run_id);
        if of_run().any(|p| p.role == Role::Supervisor) {
            live += 1;
        } else {
            found.push(Leftover {
                run_id: run_id.to_owned(),
                pids: of_run().map(|p| p.pid).collect(),
            });
        }
    }
    (found, live)
}

/// Reads the processes of runs from a `/proc` directory.
#[must_use]
pub fn scan(proc_dir: &Path) -> Vec<RunProcess> {
    let Ok(entries) = std::fs::read_dir(proc_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let bytes = std::fs::read(entry.path().join("cmdline")).ok()?;
            let text = String::from_utf8_lossy(&bytes);
            let args: Vec<&str> = text.split('\0').filter(|arg| !arg.is_empty()).collect();
            parse_run_process(pid, &args)
        })
        .collect()
}

/// Ends the processes of runs that have no supervisor. With `dry_run` it only reports them.
///
/// A process is ended with `kill -KILL`, after reading its command line once more so that a process id that
/// was reused in the meantime is not hit. The network namespace of a holder goes away with its last process.
#[must_use]
pub fn clean(dry_run: bool) -> CleanReport {
    clean_in(Path::new("/proc"), None, dry_run)
}

/// Like [`clean`], but only for the run called `run_id`, if it has no supervisor. Other leftovers are not
/// reported and not touched. For tests that run side by side, and for a caller that knows its run.
#[must_use]
pub fn clean_run(run_id: &str, dry_run: bool) -> CleanReport {
    clean_in(Path::new("/proc"), Some(run_id), dry_run)
}

fn clean_in(proc_dir: &Path, only: Option<&str>, dry_run: bool) -> CleanReport {
    let (mut found, live_runs) = leftovers(&scan(proc_dir));
    if let Some(run_id) = only {
        found.retain(|leftover| leftover.run_id == run_id);
    }
    let mut report = CleanReport {
        found,
        live_runs,
        dry_run,
        ..CleanReport::default()
    };
    if dry_run {
        return report;
    }
    let pids: Vec<u32> = report.found.iter().flat_map(|l| l.pids.clone()).collect();
    for pid in pids {
        match end(proc_dir, pid) {
            Ok(()) => report.ended.push(pid),
            Err(reason) => report.failed.push(Failure { pid, reason }),
        }
    }
    report
}

fn end(proc_dir: &Path, pid: u32) -> Result<(), String> {
    let still_a_holder = || scan(proc_dir).iter().any(|p| p.pid == pid);
    if !still_a_holder() {
        // It ended by itself since the scan
        return Ok(());
    }
    let kill = find("kill").ok_or("the tool `kill` was not found (package `util-linux`)")?;
    let outcome = run(&kill, &["-KILL", &pid.to_string()]);
    let deadline = Instant::now() + Duration::from_secs(2);
    while still_a_holder() {
        if Instant::now() >= deadline {
            return Err(outcome
                .err()
                .unwrap_or_else(|| "it is still there two seconds after the signal".to_owned()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4711-0-123456789";

    fn process(pid: u32, run_id: &str, role: Role) -> RunProcess {
        RunProcess {
            pid,
            run_id: run_id.to_owned(),
            role,
        }
    }

    #[test]
    fn the_processes_of_a_run_are_recognised_by_their_command_line() {
        assert_eq!(
            parse_run_process(
                7,
                &["/x/netscenario-kernel-supervisor", "supervise", ID, "8"]
            ),
            Some(process(7, ID, Role::Supervisor))
        );
        assert_eq!(
            parse_run_process(8, &["netscenario", "hold", ID]),
            Some(process(8, ID, Role::Holder))
        );
    }

    /// Canary: a process that merely looks similar is not ours and must not be ended.
    #[test]
    fn other_processes_are_not_recognised() {
        for args in [
            &["vim", "hold", ID][..],
            &["/usr/bin/sleep", "supervise", ID, "8"][..],
            &["netscenario", "hold", "not-a-run-id"][..],
            &["netscenario", "hold", "1-2"][..],
            &["netscenario", "hold", "1-2-3-4"][..],
            &["netscenario", "hold", "1--3"][..],
            &["netscenario", "supervise", ID][..],
            &["netscenario", "supervise", ID, "many"][..],
            &["netscenario", "hold", ID, "extra"][..],
            &["netscenario", "run", ID][..],
            &[][..],
        ] {
            assert_eq!(parse_run_process(1, args), None, "{args:?}");
        }
    }

    #[test]
    fn a_run_without_a_supervisor_is_a_leftover() {
        let other = "4712-0-5";
        let processes = [
            process(10, ID, Role::Holder),
            process(11, ID, Role::Holder),
            process(20, other, Role::Supervisor),
            process(21, other, Role::Holder),
        ];
        let (found, live) = leftovers(&processes);
        assert_eq!(
            found,
            [Leftover {
                run_id: ID.to_owned(),
                pids: vec![10, 11]
            }]
        );
        assert_eq!(live, 1);
    }

    #[test]
    fn the_text_says_what_was_found_and_what_was_done() {
        let found = vec![Leftover {
            run_id: ID.to_owned(),
            pids: vec![10, 11],
        }];
        let nothing = CleanReport {
            live_runs: 2,
            ..CleanReport::default()
        };
        assert_eq!(
            nothing.render(),
            "netscenario clean\nNo run without a supervisor was found. 2 runs with a supervisor are left alone.\n"
        );
        let ended = CleanReport {
            found: found.clone(),
            ended: vec![10, 11],
            ..CleanReport::default()
        };
        assert_eq!(
            ended.render(),
            "netscenario clean\nFound 1 run without a supervisor:\n  4711-0-123456789  2 processes: 10, 11\nEnded 2 processes.\n"
        );
        let dry = CleanReport {
            found,
            dry_run: true,
            ..CleanReport::default()
        };
        assert!(
            dry.render()
                .contains("Dry run: 2 processes would be ended, nothing was.")
        );
        let failed = CleanReport {
            found: vec![Leftover {
                run_id: ID.to_owned(),
                pids: vec![10],
            }],
            failed: vec![Failure {
                pid: 10,
                reason: "denied".to_owned(),
            }],
            ..CleanReport::default()
        };
        assert!(!failed.succeeded());
        assert!(failed.render().contains("Could not end 10: denied"));
    }

    #[test]
    fn no_processes_no_leftovers() {
        assert_eq!(leftovers(&[]), (Vec::new(), 0));
    }

    fn fake_proc(name: &str, entries: &[(u32, &[&str])]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("netscenario-proc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (pid, args) in entries {
            let process = dir.join(pid.to_string());
            std::fs::create_dir_all(&process).expect("a temporary directory");
            std::fs::write(process.join("cmdline"), args.join("\0") + "\0").expect("a file");
        }
        // Entries of /proc that are not processes
        std::fs::create_dir_all(dir.join("self")).expect("a temporary directory");
        std::fs::write(dir.join("uptime"), "1 1").expect("a file");
        dir
    }

    #[test]
    fn the_scan_reads_the_command_lines_of_proc() {
        let dir = fake_proc(
            "scan",
            &[
                (10, &["netscenario", "hold", ID]),
                (11, &["bash", "-c", "hold"]),
                (12, &["netscenario", "supervise", "4712-0-5", "8"]),
            ],
        );
        let mut found = scan(&dir);
        found.sort_by_key(|p| p.pid);
        assert_eq!(
            found,
            [
                process(10, ID, Role::Holder),
                process(12, "4712-0-5", Role::Supervisor)
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dry_run_reports_and_ends_nothing() {
        let dir = fake_proc("dry", &[(10, &["netscenario", "hold", ID])]);
        let report = clean_in(&dir, None, true);
        assert_eq!(report.found.len(), 1);
        assert!(report.ended.is_empty() && report.failed.is_empty() && report.dry_run);
        assert!(report.succeeded());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_scoped_clean_reports_only_its_own_run() {
        let dir = fake_proc(
            "scope",
            &[
                (10, &["netscenario", "hold", ID]),
                (11, &["netscenario", "hold", "4712-0-5"]),
            ],
        );
        let report = clean_in(&dir, Some("4712-0-5"), true);
        assert_eq!(
            report.found,
            [Leftover {
                run_id: "4712-0-5".to_owned(),
                pids: vec![11]
            }]
        );
        assert_eq!(clean_in(&dir, Some("1-1-1"), true).found, []);
        assert_eq!(clean_in(&dir, None, true).found.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_process_that_is_gone_since_the_scan_counts_as_ended() {
        // The file for the process 10 exists only while the scan runs: here it is removed by `end` finding
        // no holder under that id, so the answer is Ok
        let dir = fake_proc("gone", &[]);
        assert_eq!(end(&dir, 10), Ok(()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
