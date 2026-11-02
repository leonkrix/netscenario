//! The programs of your own in the nodes (`kind: command`; WRK-06, WRK-10, PRB-08; D-072): started in the order of the
//! file, each waited for until it is ready, watched while the run goes, stopped when it ends, and reported.
//!
//! - **Start.** A program is started in its node, as the invoking user, in the directory of the scenario file, without a
//!   shell, with the environment of the file and `NETSCENARIO_*` ([`environment`]), and with `{node.addr}` replaced
//!   ([`substitute`]). It leads a process group of its own. Its standard output and standard error go to files of the
//!   run directory (the first megabyte of each).
//! - **Ready.** The next program, and the senders, start only when it is ready (the way of `ready`, or at once without
//!   one). A program that ends first, or is not ready in time, fails the run, with the end of its standard error.
//! - **Watched.** While the workloads run the run looks at each program every 50 ms or so: its metric lines are read
//!   ([`crate::metrics`]), and an end with an error is the failure that `on-exit: fail` asks for.
//! - **Stop.** After the run, the signal of `shutdown` goes to the process group, and `KILL` follows after `within`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use netscenario_agent::clock::monotonic_ns;
use netscenario_model::{Command, OnExit, Ready, Scenario, Workload};

use crate::backend::{
    Backend, NodeHandle, OutputStats, PortKind, Program, ProgramExit, ProgramHandle, ProgramSignal,
    ProgramState,
};
use crate::error::Error;
use crate::metrics::Collector;
use crate::provision::Provisioned;
use crate::result::{MetricSummary, ProgramOutput, ProgramResult};
use crate::run::{Control, RunOptions, failure};

/// How often the programs are looked at while the workloads run.
pub(crate) const WATCH_EVERY: Duration = Duration::from_millis(50);

/// How long the run waits for a program to end after `KILL`, before it reports that it did not.
const AFTER_KILL: Duration = Duration::from_secs(5);

/// How a readiness check looks at the program in between.
const READY_POLL: Duration = Duration::from_millis(10);

/// Replaces `{node.addr}` by the address of the node in `text`; `{{` and `}}` are the braces themselves. The model has
/// refused every other use of a brace when the file was read, so anything else is left as it is.
pub(crate) fn substitute(text: &str, addresses: &BTreeMap<String, Ipv4Addr>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if let Some(end) = tail.find('}').filter(|_| tail.starts_with('{')) {
            let name = tail[1..end].strip_suffix(".addr");
            match name.and_then(|node| addresses.get(node)) {
                Some(address) => out.push_str(&address.to_string()),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The variables the tool sets for a program: `NETSCENARIO_WORKLOAD` and `NETSCENARIO_NODE` (the ids of the program
/// and of its node), `NETSCENARIO_RUN_DIR` (the directory of the run), `NETSCENARIO_SEED` (if the run has one), and
/// `NETSCENARIO_ADDR_<NODE>` for every node (the name in capitals, `-` as `_`).
pub(crate) fn environment(
    command: &Command,
    addresses: &BTreeMap<String, Ipv4Addr>,
    run_dir: &Path,
    seed: Option<u64>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = command
        .env
        .iter()
        .map(|(name, value)| (name.clone(), substitute(value, addresses)))
        .collect();
    env.push(("NETSCENARIO_WORKLOAD".to_owned(), command.id.to_string()));
    env.push(("NETSCENARIO_NODE".to_owned(), command.node.to_string()));
    env.push((
        "NETSCENARIO_RUN_DIR".to_owned(),
        run_dir.display().to_string(),
    ));
    if let Some(seed) = seed {
        env.push(("NETSCENARIO_SEED".to_owned(), seed.to_string()));
    }
    for (node, address) in addresses {
        let name: String = node
            .chars()
            .map(|c| {
                if c == '-' {
                    '_'
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        env.push((format!("NETSCENARIO_ADDR_{name}"), address.to_string()));
    }
    env
}

/// The address of each node: its end of the first link it is on.
fn node_addresses(provisioned: &Provisioned) -> BTreeMap<String, Ipv4Addr> {
    let mut found = BTreeMap::new();
    for entry in &provisioned.addresses {
        found.entry(entry.node.to_string()).or_insert(entry.address);
    }
    found
}

/// What a program must have for the run to call it ready: the check, called with the lines it printed since the last look.
type ReadyCheck = Box<dyn FnMut(&mut dyn Backend, &[String]) -> Result<bool, Error>>;

/// A program and what the run knows about it.
struct Item {
    id: String,
    node_name: String,
    node: NodeHandle,
    run: Vec<String>,
    handle: ProgramHandle,
    signal: ProgramSignal,
    within: Duration,
    on_exit: OnExit,
    /// `CLOCK_MONOTONIC` readings in nanoseconds.
    started: u64,
    ready: u64,
    /// When the run asked it to stop, and whether it had to be killed.
    stopping: Option<Instant>,
    killed: bool,
    exit: Option<(ProgramExit, u64)>,
    stderr: String,
    output: OutputStats,
    truncated: bool,
    /// Lines read before the start of the measured phase, which has no time yet: (when they were read, the line).
    early: Vec<(u64, String)>,
    collector: Collector,
    stdout_name: String,
    stderr_name: String,
    metrics_name: String,
}

/// The programs of a run.
#[derive(Default)]
pub(crate) struct Programs {
    items: Vec<Item>,
    /// The start of the measured phase, once it is known.
    begin: Option<u64>,
    last_look: Option<Instant>,
}

impl Programs {
    /// Whether the scenario has any.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Starts the programs of `scenario` in the order of the file, each when the one before is ready.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] if one cannot be started, ends before it is ready, or is not ready in time;
    /// [`Error::Cancelled`] if the run is cancelled; [`Error::Backend`] from the backend.
    pub(crate) fn start(
        &mut self,
        backend: &mut dyn Backend,
        scenario: &Scenario,
        provisioned: &Provisioned,
        options: &RunOptions,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        let addresses = node_addresses(provisioned);
        let run_dir =
            std::fs::canonicalize(&options.out_dir).unwrap_or_else(|_| options.out_dir.clone());
        for command in scenario.workloads.iter().filter_map(Workload::as_command) {
            control.checkpoint()?;
            let node = provisioned
                .nodes
                .iter()
                .find(|(name, _)| *name == command.node)
                .map(|(_, handle)| *handle)
                .ok_or_else(|| {
                    failure(
                        "find a node",
                        format!("the node '{}' was not built", command.node),
                    )
                })?;
            let run: Vec<String> = command
                .run
                .iter()
                .map(|argument| substitute(argument, &addresses))
                .collect();
            let stdout_name = format!("program-{}.stdout.log", command.id);
            let stderr_name = format!("program-{}.stderr.log", command.id);
            let metrics_name = format!("metrics-{}.jsonl", command.id);
            let mut program = Program::new(run.clone())
                .with_env(environment(command, &addresses, &run_dir, provisioned.seed))
                .in_own_group()
                .with_log_files(
                    options.out_dir.join(&stdout_name),
                    options.out_dir.join(&stderr_name),
                );
            if let Some(directory) = &options.working_dir {
                program = program.with_working_dir(directory.clone());
            }
            let what = format!("start the program '{}'", command.id);
            if let Err(why) = find_program(&run[0], options.working_dir.as_deref()) {
                return Err(failure(what, why));
            }
            let handle = backend
                .start_program(node, &program)
                .map_err(|error| failure(what.clone(), error.to_string()))?;
            let started = monotonic_ns();
            let shutdown = command.shutdown.unwrap_or_default();
            self.items.push(Item {
                id: command.id.to_string(),
                node_name: command.node.to_string(),
                node,
                run,
                handle,
                signal: shutdown
                    .signal
                    .map_or(ProgramSignal::Term, ProgramSignal::from),
                within: shutdown
                    .within
                    .map_or(Duration::from_secs(5), Duration::from),
                on_exit: command.on_exit,
                started,
                ready: started,
                stopping: None,
                killed: false,
                exit: None,
                stderr: String::new(),
                output: OutputStats::default(),
                truncated: false,
                early: Vec::new(),
                collector: Collector::with_file(Box::new(LazyFile::new(
                    options.out_dir.join(&metrics_name),
                ))),
                stdout_name,
                stderr_name,
                metrics_name,
            });
            let last = self.items.len() - 1;
            self.wait_ready(backend, last, command.ready.as_ref(), options, control)?;
        }
        Ok(())
    }

    fn wait_ready(
        &mut self,
        backend: &mut dyn Backend,
        index: usize,
        ready: Option<&Ready>,
        options: &RunOptions,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        let Some(ready) = ready else {
            return Ok(());
        };
        let id = self.items[index].id.clone();
        let (what, mut check): (String, ReadyCheck) = if let Some(port) = ready.udp_port {
            let node = self.items[index].node;
            (
                format!("a UDP socket on port {port}"),
                Box::new(move |backend, _| backend.port_bound(node, PortKind::Udp, port)),
            )
        } else if let Some(port) = ready.tcp_port {
            let node = self.items[index].node;
            (
                format!("a TCP socket listening on port {port}"),
                Box::new(move |backend, _| backend.port_bound(node, PortKind::Tcp, port)),
            )
        } else if let Some(text) = ready.stdout_line.clone() {
            (
                format!("a line of standard output with '{text}'"),
                Box::new(move |_, lines| Ok(lines.iter().any(|line| line.contains(&text)))),
            )
        } else if let Some(file) = &ready.file {
            let path: PathBuf = options
                .working_dir
                .as_ref()
                .map_or_else(|| PathBuf::from(file), |directory| directory.join(file));
            (
                format!("the file {}", path.display()),
                Box::new(move |_, _| Ok(path.exists())),
            )
        } else if let Some(delay) = ready.delay {
            // Nothing is checked: the time passes, and a program that ends in the meantime is looked at as usual
            let until = Instant::now() + Duration::from(delay);
            (
                format!(
                    "{} to pass",
                    netscenario_model::Duration::from_nanos(
                        u64::try_from(Duration::from(delay).as_nanos()).unwrap_or(u64::MAX)
                    )
                ),
                Box::new(move |_, _| Ok(Instant::now() >= until)),
            )
        } else {
            return Ok(());
        };
        let within = ready.within.map(Duration::from);
        let deadline = within.map(|within| Instant::now() + within);
        loop {
            control.checkpoint()?;
            let state = backend.poll_program(self.items[index].handle, READY_POLL)?;
            let lines = state.lines.clone();
            self.absorb(index, state)?;
            let item = &self.items[index];
            if let Some((exit, _)) = item.exit
                && ready.delay.is_none()
            {
                return Err(failure(
                    format!("wait for the program '{id}'"),
                    format!(
                        "it ended with {exit} before it was ready (waiting for {what}); the end of its standard error: {}",
                        tail(&item.stderr)
                    ),
                ));
            }
            if check(backend, &lines)? {
                self.items[index].ready = monotonic_ns();
                return Ok(());
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(failure(
                    format!("wait for the program '{id}'"),
                    format!(
                        "it was not ready within {} (waiting for {what}); the end of its standard error: {}",
                        within.map_or_else(String::new, |within| format!("{within:?}")),
                        tail(&self.items[index].stderr)
                    ),
                ));
            }
        }
    }

    /// Takes what a poll found out about program `index`.
    fn absorb(&mut self, index: usize, state: ProgramState) -> Result<(), Error> {
        let now = monotonic_ns();
        let begin = self.begin;
        let item = &mut self.items[index];
        for line in state.lines {
            match begin {
                Some(begin) => feed(item, &line, now, begin),
                None => item.early.push((now, line)),
            }
        }
        item.stderr = state.stderr;
        item.output = state.output;
        item.truncated |= state.truncated;
        if let Some(exit) = state.exit
            && item.exit.is_none()
        {
            item.exit = Some((exit, now));
            if item.on_exit == OnExit::Fail && !exit.success() && item.stopping.is_none() {
                return Err(failure(
                    format!("run the program '{}'", item.id),
                    format!(
                        "it ended with {exit} before the end of the run (on-exit: fail); the end of its standard error: {}",
                        tail(&item.stderr)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The measured phase begins at `begin` (`CLOCK_MONOTONIC`): what the programs have printed so far gets its time.
    pub(crate) fn set_begin(&mut self, begin: u64) {
        self.begin = Some(begin);
        for item in &mut self.items {
            for (when, line) in std::mem::take(&mut item.early) {
                feed(item, &line, when, begin);
            }
        }
    }

    /// Looks at every program that is still going, if it is time: its new lines, and whether it has ended.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] for a program with `on-exit: fail` that ended with an error; [`Error::Backend`].
    pub(crate) fn watch(&mut self, backend: &mut dyn Backend) -> Result<(), Error> {
        if self.items.is_empty() {
            return Ok(());
        }
        if self
            .last_look
            .is_some_and(|last| last.elapsed() < WATCH_EVERY)
        {
            return Ok(());
        }
        self.last_look = Some(Instant::now());
        for index in 0..self.items.len() {
            if self.items[index].exit.is_none() {
                let state = backend.poll_program(self.items[index].handle, Duration::ZERO)?;
                self.absorb(index, state)?;
            }
        }
        Ok(())
    }

    /// Stops every program that is still going: the signal of `shutdown` to its process group, `KILL` after `within`.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] for a program that is still there after `KILL`; [`Error::Backend`].
    pub(crate) fn stop_all(&mut self, backend: &mut dyn Backend) -> Result<(), Error> {
        for index in 0..self.items.len() {
            // A last look first: what it printed and whether it has ended on its own
            if self.items[index].exit.is_none() {
                let state = backend.poll_program(self.items[index].handle, Duration::ZERO)?;
                self.absorb(index, state)?;
            }
            let item = &mut self.items[index];
            if item.exit.is_none() {
                backend.signal_program(item.handle, item.signal)?;
                item.stopping = Some(Instant::now());
            }
        }
        loop {
            let mut waiting = false;
            for index in 0..self.items.len() {
                if self.items[index].exit.is_some() {
                    continue;
                }
                let state = backend.poll_program(self.items[index].handle, READY_POLL)?;
                self.absorb(index, state)?;
                let item = &mut self.items[index];
                if item.exit.is_some() {
                    continue;
                }
                waiting = true;
                let since = item.stopping.map_or(Duration::ZERO, |at| at.elapsed());
                if !item.killed && since >= item.within {
                    backend.signal_program(item.handle, ProgramSignal::Kill)?;
                    item.killed = true;
                } else if item.killed && since >= item.within + AFTER_KILL {
                    return Err(failure(
                        format!("stop the program '{}'", item.id),
                        format!(
                            "it was still there {AFTER_KILL:?} after KILL; the end of its standard error: {}",
                            tail(&item.stderr)
                        ),
                    ));
                }
            }
            if !waiting {
                break;
            }
        }
        Ok(())
    }

    /// Ends the programs at once, after an error: no waiting, no report.
    pub(crate) fn kill_all(&mut self, backend: &mut dyn Backend) {
        for item in &mut self.items {
            if item.exit.is_none() {
                let _ = backend.signal_program(item.handle, ProgramSignal::Kill);
            }
        }
    }

    /// The report of each program, with times from `begin`.
    pub(crate) fn into_results(mut self, begin: u64) -> Vec<ProgramResult> {
        let relative = |abs: u64| -> i64 {
            i64::try_from(i128::from(abs) - i128::from(begin)).unwrap_or(i64::MAX)
        };
        self.items
            .drain(..)
            .map(|mut item| {
                item.collector.finish();
                let (exit, ended_at) = item.exit.unwrap_or((ProgramExit::Code(-1), item.ready));
                let ended = if item.killed {
                    "killed"
                } else if item.stopping.is_some() {
                    "stopped"
                } else {
                    "on-its-own"
                };
                let (exit_code, exit_signal) = match exit {
                    ProgramExit::Code(code) => (Some(code), None),
                    ProgramExit::Signal(signal) => (None, Some(signal)),
                };
                ProgramResult {
                    id: item.id,
                    node: item.node_name,
                    run: item.run,
                    started_ns: relative(item.started),
                    ready_ns: relative(item.ready),
                    ended_ns: relative(ended_at),
                    ended: ended.to_owned(),
                    exit_code,
                    exit_signal,
                    stdout_file: item.stdout_name,
                    stderr_file: item.stderr_name,
                    output: ProgramOutput {
                        stdout_bytes: item.output.stdout_bytes,
                        stdout_dropped_bytes: item.output.stdout_dropped_bytes,
                        stderr_bytes: item.output.stderr_bytes,
                        stderr_dropped_bytes: item.output.stderr_dropped_bytes,
                        long_lines: item.output.long_lines,
                    },
                    lines_dropped: item.truncated,
                    metrics: item
                        .collector
                        .summaries()
                        .iter()
                        .map(|(name, summary)| MetricSummary {
                            name: name.clone(),
                            unit: summary.unit.clone(),
                            count: summary.count,
                            first: summary.first,
                            last: summary.last,
                            min: summary.min,
                            max: summary.max,
                        })
                        .collect(),
                    metrics_file: (item.collector.written() > 0).then_some(item.metrics_name),
                    metric_readings: item.collector.readings(),
                    bad_metric_lines: item.collector.bad(),
                    bad_metric_examples: item.collector.examples().to_vec(),
                }
            })
            .collect()
    }
}

/// Gives a line of standard output to the metrics of a program, read at `when` (`CLOCK_MONOTONIC`).
fn feed(item: &mut Item, line: &str, when: u64, begin: u64) {
    let t = i64::try_from(i128::from(when) - i128::from(begin)).unwrap_or(i64::MAX);
    item.collector.take(line, t);
}

/// The file of readings of a program, made when the first reading is written, so that a program without metrics leaves
/// no file behind.
struct LazyFile {
    path: PathBuf,
    file: Option<BufWriter<File>>,
}

impl LazyFile {
    fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }
}

impl Write for LazyFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.file.is_none() {
            self.file = Some(BufWriter::new(File::create(&self.path)?));
        }
        self.file
            .as_mut()
            .map_or(Ok(bytes.len()), |file| file.write(bytes))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}

/// Whether `program` can be run: a path with a slash is looked for from `directory` (the directory of the scenario
/// file), a name without one in the directories of `PATH`. The node shares the file system with the run, so what is
/// not found here is not found there, and the program would end at once with the exit code 127 of `nsenter`, which
/// is no answer for a person who misspelled a name.
fn find_program(program: &str, directory: Option<&Path>) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let runnable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let path = directory.map_or_else(
            || PathBuf::from(program),
            |directory| directory.join(program),
        );
        return if runnable(&path) {
            Ok(path)
        } else if path.exists() {
            Err(format!("{} is not an executable file", path.display()))
        } else {
            Err(format!("{} does not exist", path.display()))
        };
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|path| path.join(program))
        .find(|path| runnable(path))
        .ok_or_else(|| format!("'{program}' is not in any directory of PATH"))
}

/// The end of what a program wrote to standard error, for a message.
fn tail(stderr: &str) -> String {
    let text = stderr.trim();
    if text.is_empty() {
        return "(nothing)".to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(400);
    chars[start..].iter().collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn addresses() -> BTreeMap<String, Ipv4Addr> {
        BTreeMap::from([
            ("client".to_owned(), Ipv4Addr::new(10, 100, 0, 1)),
            ("edge-1".to_owned(), Ipv4Addr::new(10, 100, 1, 2)),
        ])
    }

    #[test]
    fn the_address_of_a_node_replaces_its_placeholder_and_doubled_braces_are_single() {
        let found = addresses();
        assert_eq!(substitute("{client.addr}:4433", &found), "10.100.0.1:4433");
        assert_eq!(
            substitute("--peer={edge-1.addr}", &found),
            "--peer=10.100.1.2"
        );
        assert_eq!(
            substitute("{{\"a\": 1}} {client.addr}", &found),
            "{\"a\": 1} 10.100.0.1"
        );
        assert_eq!(substitute("no braces", &found), "no braces");
        assert_eq!(substitute("", &found), "");
        // Canary: what the model refuses is not invented here: an unknown node is left as it is
        assert_eq!(substitute("{nobody.addr}", &found), "{nobody.addr}");
        assert_eq!(substitute("a } b {", &found), "a } b {");
    }

    #[test]
    fn the_tool_sets_its_variables_after_the_ones_of_the_file() {
        let mut command = Command::new(
            netscenario_model::Name::new("app").unwrap(),
            netscenario_model::Name::new("client").unwrap(),
            vec!["./x".to_owned()],
        );
        command
            .env
            .insert("PEER".to_owned(), "{edge-1.addr}".to_owned());
        let env = environment(&command, &addresses(), Path::new("/runs/1"), Some(9));
        let get = |name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
        assert_eq!(get("PEER"), Some("10.100.1.2"));
        assert_eq!(get("NETSCENARIO_WORKLOAD"), Some("app"));
        assert_eq!(get("NETSCENARIO_NODE"), Some("client"));
        assert_eq!(get("NETSCENARIO_RUN_DIR"), Some("/runs/1"));
        assert_eq!(get("NETSCENARIO_SEED"), Some("9"));
        assert_eq!(get("NETSCENARIO_ADDR_CLIENT"), Some("10.100.0.1"));
        // A dash in the name of a node is an underscore in the variable
        assert_eq!(get("NETSCENARIO_ADDR_EDGE_1"), Some("10.100.1.2"));
        let without = environment(&command, &addresses(), Path::new("/runs/1"), None);
        assert!(without.iter().all(|(name, _)| name != "NETSCENARIO_SEED"));
    }

    #[test]
    fn a_program_is_found_from_the_directory_of_the_file_or_in_the_path() {
        let directory =
            std::env::temp_dir().join(format!("netscenario-find-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        // Not executable yet
        let why = find_program("./run.sh", Some(&directory)).unwrap_err();
        assert!(why.contains("is not an executable file"), "{why}");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program("./run.sh", Some(&directory)).unwrap(), script);
        let why = find_program("./missing.sh", Some(&directory)).unwrap_err();
        assert!(why.contains("does not exist"), "{why}");
        // A name without a slash is looked for in PATH: `sh` is there, a made-up name is not
        assert!(find_program("sh", None).is_ok());
        let why = find_program("no-such-program-for-netscenario", None).unwrap_err();
        assert!(why.contains("PATH"), "{why}");
        let _ = std::fs::remove_dir_all(directory);
    }
}
