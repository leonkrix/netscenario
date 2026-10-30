//! The work inside the namespaces: nodes as network namespaces held by a process, links as virtual Ethernet
//! pairs, impairments as `netem`. It runs in the supervisor, which is the first process of a PID namespace and
//! lives in a network namespace of its own (D-022), so nothing here can reach the network of the host (SAF-01).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use netscenario_model::Impairments;

use crate::command::{Toolbox, run};
use crate::netem;
use crate::protocol::Way;

/// The addresses of the links: link `n` uses `10.100.n.0/30`, the first node `.1` and the second `.2`.
const ADDRESS_BASE: &str = "10.100";
/// The most links a run can address.
const MAX_LINKS: usize = 200;

/// A failpoint of the crash tests: `NETSCENARIO_FAIL_AT=link:created` ends the supervisor, without cleaning
/// up, the first time it passes that point; `link:created#2` the second time. It exists to test the cleanup
/// guarantee (BKD-04) and does nothing when the variable is not set.
#[derive(Debug, Default)]
pub(crate) struct Failpoints {
    wanted: Option<(String, usize)>,
    seen: HashMap<String, usize>,
}

impl Failpoints {
    pub(crate) fn new(wanted: Option<&str>) -> Self {
        let wanted = wanted.map(|text| match text.split_once('#') {
            Some((point, count)) => (point.to_owned(), count.parse().unwrap_or(1)),
            None => (text.to_owned(), 1),
        });
        Self {
            wanted,
            seen: HashMap::new(),
        }
    }

    pub(crate) fn hit(&mut self, point: &str) {
        let count = self.seen.entry(point.to_owned()).or_insert(0);
        *count += 1;
        if self.wanted.as_ref() == Some(&(point.to_owned(), *count)) {
            // A crash, not an orderly exit: no destructor runs and nothing is cleaned up
            std::process::abort();
        }
    }
}

struct Node {
    name: String,
    holder: Child,
    pid: u32,
}

/// One end of a link: the node and the name of the interface in it.
struct End {
    node: usize,
    interface: String,
}

struct Link {
    ends: [End; 2],
}

/// The most lines of standard output that the supervisor keeps for a program that nobody asks about.
const MAX_LINES: usize = 10_000;

/// The most bytes of standard error that are kept: the end of it.
const STDERR_LIMIT: usize = 16 * 1024;

/// How long a program that has ended may take to deliver the rest of its output, if a process that it started
/// keeps the pipe open.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);

/// What the threads that read a program's output collect.
#[derive(Default)]
struct Output {
    state: Mutex<OutputState>,
    news: Condvar,
}

#[derive(Default)]
struct OutputState {
    lines: Vec<String>,
    kept: usize,
    truncated: bool,
    stdout_ended: bool,
    stderr_ended: bool,
    stderr: String,
}

impl Output {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutputState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A program in a node, with the threads that read its standard output and standard error.
struct RunningProgram {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Output>,
}

/// What a poll reports.
pub(crate) struct ProgramReport {
    pub(crate) lines: Vec<String>,
    pub(crate) exit: Option<crate::protocol::Exit>,
    pub(crate) stderr: String,
    pub(crate) truncated: bool,
}

fn read_stdout(pipe: impl Read, output: &Output) {
    let mut reader = BufReader::new(pipe);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        // A program may write what is not UTF-8: it is shown with replacement characters, not dropped
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&bytes)
                    .trim_end_matches(['\n', '\r'])
                    .to_owned();
                let mut state = output.lock();
                if state.kept < MAX_LINES {
                    state.lines.push(line);
                    state.kept += 1;
                } else {
                    state.truncated = true;
                }
                output.news.notify_all();
            }
        }
    }
    output.lock().stdout_ended = true;
    output.news.notify_all();
}

fn read_stderr(mut pipe: impl Read, output: &Output) {
    let mut buffer = [0_u8; 1024];
    while let Ok(n) = pipe.read(&mut buffer) {
        if n == 0 {
            break;
        }
        let mut state = output.lock();
        state
            .stderr
            .push_str(&String::from_utf8_lossy(&buffer[..n]));
        if state.stderr.len() > STDERR_LIMIT {
            let cut = state.stderr.len() - STDERR_LIMIT;
            let cut = (cut..state.stderr.len())
                .find(|&i| state.stderr.is_char_boundary(i))
                .unwrap_or(0);
            state.stderr.drain(..cut);
        }
    }
    output.lock().stderr_ended = true;
    output.news.notify_all();
}

pub(crate) struct Provisioner {
    tools: Toolbox,
    /// This program, which also holds the namespaces of the nodes (`hold`).
    exe: PathBuf,
    run_id: String,
    max_nodes: usize,
    failpoints: Failpoints,
    nodes: Vec<Node>,
    links: Vec<Link>,
    programs: Vec<RunningProgram>,
}

fn id(index: usize) -> Result<u32, String> {
    u32::try_from(index).map_err(|_| "too many objects".to_owned())
}

impl Provisioner {
    pub(crate) fn new(
        tools: Toolbox,
        run_id: String,
        max_nodes: usize,
        failpoints: Failpoints,
    ) -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("cannot find the supervisor's own program: {error}"))?;
        Ok(Self {
            tools,
            exe,
            run_id,
            max_nodes,
            failpoints,
            nodes: Vec::new(),
            links: Vec::new(),
            programs: Vec::new(),
        })
    }

    /// SAF-01: the supervisor touches the network only if it is the first process of a PID namespace and sees
    /// nothing but the loopback interface, which means it was started by `unshare` for this run and is not
    /// in the network namespace of the host.
    pub(crate) fn check_isolated(&mut self) -> Result<(), String> {
        let pid = std::process::id();
        if pid != 1 {
            return Err(format!(
                "the supervisor is process {pid}, not the first process of a PID namespace; it refuses to run (SAF-01)"
            ));
        }
        let listing = run(&self.tools.ip, &["-o", "link", "show"])?;
        let interfaces: Vec<&str> = listing
            .lines()
            .filter_map(|line| line.split(": ").nth(1))
            .map(|name| name.split('@').next().unwrap_or(name))
            .collect();
        if interfaces != ["lo"] {
            return Err(format!(
                "the supervisor sees the network interfaces {}: it is not in a network namespace of its own and refuses to touch the network (SAF-01)",
                interfaces.join(", ")
            ));
        }
        // New interfaces start without IPv6, so that no packet that nobody asked for shifts the loss pattern
        // of `netem` (spike S2, `docs/COVERAGE.md`)
        disable_ipv6()
    }

    pub(crate) fn create_node(&mut self, name: &str) -> Result<u32, String> {
        self.failpoints.hit("node:start");
        if self.nodes.len() >= self.max_nodes {
            return Err(format!(
                "a run may use at most {} network namespaces (SAF-02) and the scenario needs more",
                self.max_nodes
            ));
        }
        if self.nodes.iter().any(|node| node.name == name) {
            return Err(format!("the node '{name}' exists already"));
        }
        let mut holder = Command::new(&self.tools.unshare)
            .args(["--net", "--"])
            .arg(&self.exe)
            .args(["hold", &self.run_id])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("cannot start the holder of the node '{name}': {error}"))?;
        let pid = holder.id();
        let stdout = holder.stdout.take();
        // From here on the holder is known, so a failure leaves it to the teardown
        self.nodes.push(Node {
            name: name.to_owned(),
            holder,
            pid,
        });
        self.failpoints.hit("node:spawned");
        let mut ready = String::new();
        if let Some(stdout) = stdout {
            BufReader::new(stdout)
                .read_line(&mut ready)
                .map_err(|error| format!("cannot read from the holder of '{name}': {error}"))?;
        }
        if ready.trim() != "ready" {
            return Err(format!(
                "the holder of the node '{name}' did not start (it said '{}')",
                ready.trim()
            ));
        }
        self.in_node(pid, &self.tools.ip, &["link", "set", "lo", "up"])?;
        self.failpoints.hit("node:end");
        id(self.nodes.len() - 1)
    }

    pub(crate) fn create_link(&mut self, name: &str, between: [u32; 2]) -> Result<u32, String> {
        self.failpoints.hit("link:start");
        let index = self.links.len();
        if index >= MAX_LINKS {
            return Err(format!("a run may have at most {MAX_LINKS} links"));
        }
        let [first, second] = between.map(|node| node as usize);
        if first == second || first >= self.nodes.len() || second >= self.nodes.len() {
            return Err(format!(
                "the link '{name}' needs two different nodes of this run"
            ));
        }
        let outside = [format!("nsc{index}a"), format!("nsc{index}b")];
        let inside = format!("l{index}");
        let macs = [0, 1].map(|end| format!("02:4e:53:00:{index:02x}:{:02x}", end + 1));
        run(
            &self.tools.ip,
            &[
                "link",
                "add",
                &outside[0],
                "address",
                &macs[0],
                "type",
                "veth",
                "peer",
                "name",
                &outside[1],
                "address",
                &macs[1],
            ],
        )?;
        self.failpoints.hit("link:created");
        let pids = [self.nodes[first].pid, self.nodes[second].pid];
        for (end, pid) in pids.iter().enumerate() {
            run(
                &self.tools.ip,
                &[
                    "link",
                    "set",
                    &outside[end],
                    "netns",
                    &pid.to_string(),
                    "name",
                    &inside,
                ],
            )?;
        }
        self.failpoints.hit("link:moved");
        let addresses = [1, 2].map(|host| format!("{ADDRESS_BASE}.{index}.{host}"));
        for end in 0..2 {
            let other = 1 - end;
            let pid = pids[end];
            self.in_node(
                pid,
                &self.tools.ip,
                &[
                    "addr",
                    "add",
                    &format!("{}/30", addresses[end]),
                    "dev",
                    &inside,
                ],
            )?;
            // The neighbour is fixed instead of learned: no ARP packet may shift the loss pattern
            self.in_node(
                pid,
                &self.tools.ip,
                &[
                    "neigh",
                    "replace",
                    &addresses[other],
                    "lladdr",
                    &macs[other],
                    "dev",
                    &inside,
                    "nud",
                    "permanent",
                ],
            )?;
            self.in_node(pid, &self.tools.ip, &["link", "set", &inside, "up"])?;
        }
        self.links.push(Link {
            ends: [
                End {
                    node: first,
                    interface: inside.clone(),
                },
                End {
                    node: second,
                    interface: inside,
                },
            ],
        });
        self.failpoints.hit("link:end");
        id(index)
    }

    pub(crate) fn apply(
        &mut self,
        link: u32,
        way: Way,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), String> {
        self.failpoints.hit("apply:start");
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        // The traffic of a direction leaves through the interface of its sender
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        let Some(arguments) = netem::arguments(impairments, seed)? else {
            return Ok(());
        };
        let pid = self.nodes[end.node].pid;
        let mut command = vec![
            "qdisc",
            "replace",
            "dev",
            end.interface.as_str(),
            "root",
            "netem",
        ];
        command.extend(arguments.iter().map(String::as_str));
        self.in_node(pid, &self.tools.tc, &command)?;
        self.failpoints.hit("apply:end");
        Ok(())
    }

    pub(crate) fn start_program(
        &mut self,
        node: u32,
        argv: &[String],
        memory_limit: Option<u64>,
    ) -> Result<u32, String> {
        self.failpoints.hit("program:start");
        let node = self
            .nodes
            .get(node as usize)
            .ok_or_else(|| format!("the node {node} does not exist in this run"))?;
        let Some((program, arguments)) = argv.split_first() else {
            return Err("a program needs at least its name".to_owned());
        };
        let mut command = Command::new(&self.tools.nsenter);
        command
            .arg(format!("--net=/proc/{}/ns/net", node.pid))
            .arg("--");
        if let Some(bytes) = memory_limit {
            // `prlimit` sets the limit and then becomes the program: nothing stays between it and us
            command
                .arg(&self.tools.prlimit)
                .arg(format!("--as={bytes}"))
                .arg("--");
        }
        let mut child = command
            .arg(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start '{program}' in the node '{}': {error}",
                    node.name
                )
            })?;
        let output = Arc::new(Output::default());
        if let Some(pipe) = child.stdout.take() {
            let output = Arc::clone(&output);
            std::thread::spawn(move || read_stdout(pipe, &output));
        } else {
            output.lock().stdout_ended = true;
        }
        if let Some(pipe) = child.stderr.take() {
            let output = Arc::clone(&output);
            std::thread::spawn(move || read_stderr(pipe, &output));
        } else {
            output.lock().stderr_ended = true;
        }
        let stdin = child.stdin.take();
        self.programs.push(RunningProgram {
            child,
            stdin,
            output,
        });
        self.failpoints.hit("program:end");
        id(self.programs.len() - 1)
    }

    /// Waits up to `wait` for a line or the end of a program and returns what is new since the last call.
    pub(crate) fn poll(&mut self, program: u32, wait: Duration) -> Result<ProgramReport, String> {
        let running = self
            .programs
            .get_mut(program as usize)
            .ok_or_else(|| format!("the program {program} does not exist in this run"))?;
        let deadline = Instant::now() + wait;
        loop {
            let exit = running
                .child
                .try_wait()
                .map_err(|error| format!("cannot ask the program {program}: {error}"))?;
            let mut state = running.output.lock();
            if exit.is_some() {
                // Everything the program wrote is read before it is reported as ended
                let grace = Instant::now() + OUTPUT_GRACE;
                while !(state.stdout_ended && state.stderr_ended) && Instant::now() < grace {
                    state = running
                        .output
                        .news
                        .wait_timeout(state, Duration::from_millis(10))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
            let now = Instant::now();
            if !state.lines.is_empty() || exit.is_some() || now >= deadline {
                return Ok(ProgramReport {
                    lines: std::mem::take(&mut state.lines),
                    exit: exit.map(|status| match (status.code(), status.signal()) {
                        (Some(code), _) => crate::protocol::Exit::Code(code),
                        (None, Some(signal)) => crate::protocol::Exit::Signal(signal),
                        (None, None) => crate::protocol::Exit::Code(-1),
                    }),
                    stderr: state.stderr.clone(),
                    truncated: state.truncated,
                });
            }
            // A line wakes this up at once; the end of the program is looked for every few milliseconds
            let slice = (deadline - now).min(Duration::from_millis(10));
            drop(
                running
                    .output
                    .news
                    .wait_timeout(state, slice)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    /// Writes a line to the standard input of a program. A program that has ended is not an error.
    pub(crate) fn send_line(&mut self, program: u32, line: &str) -> Result<(), String> {
        let running = self
            .programs
            .get_mut(program as usize)
            .ok_or_else(|| format!("the program {program} does not exist in this run"))?;
        if let Some(stdin) = &mut running.stdin {
            // A broken pipe means that the program is gone, which is what the caller wanted anyway
            let _ = writeln!(stdin, "{line}").and_then(|()| stdin.flush());
        }
        Ok(())
    }

    /// Ends the programs and the holders of the nodes. It can be called at any point and twice. What is left
    /// (the interfaces, the namespaces) goes with the holders, and the kernel removes the rest when the
    /// supervisor ends.
    pub(crate) fn teardown(&mut self) -> Result<(), String> {
        self.failpoints.hit("teardown:start");
        let mut left = Vec::new();
        for mut program in self.programs.drain(..) {
            // A program that ended already is not an error
            let _ = program.child.kill();
            let _ = program.child.wait();
        }
        for mut node in self.nodes.drain(..) {
            if node.holder.kill().is_err() && node.holder.try_wait().ok().flatten().is_none() {
                left.push(node.name.clone());
            }
            let _ = node.holder.wait();
        }
        self.links.clear();
        self.failpoints.hit("teardown:end");
        if left.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "the holders of the nodes {} could not be ended",
                left.join(", ")
            ))
        }
    }

    /// Runs `program` with `arguments` in the network namespace held by process `pid`.
    fn in_node(
        &self,
        pid: u32,
        program: &std::path::Path,
        arguments: &[&str],
    ) -> Result<String, String> {
        let namespace = format!("--net=/proc/{pid}/ns/net");
        let program = program.to_string_lossy();
        let mut command = vec![namespace.as_str(), "--", program.as_ref()];
        command.extend_from_slice(arguments);
        run(&self.tools.nsenter, &command)
    }
}

/// Switches IPv6 off for the interfaces that are created from now on in the network namespace of the caller.
pub(crate) fn disable_ipv6() -> Result<(), String> {
    for file in [
        "/proc/sys/net/ipv6/conf/all/disable_ipv6",
        "/proc/sys/net/ipv6/conf/default/disable_ipv6",
    ] {
        match fs::write(file, "1") {
            Ok(()) => {}
            // The kernel has no IPv6, so there is nothing to switch off
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot switch off IPv6 ({file}): {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failpoint_fires_only_at_its_point_and_count() {
        // `hit` would abort the test process, so only the bookkeeping is checked
        let mut failpoints = Failpoints::new(Some("link:end#2"));
        assert_eq!(failpoints.wanted, Some(("link:end".to_owned(), 2)));
        failpoints.hit("link:end");
        failpoints.hit("node:end");
        assert_eq!(failpoints.seen["link:end"], 1);
        assert_eq!(Failpoints::new(Some("a")).wanted, Some(("a".to_owned(), 1)));
        assert_eq!(Failpoints::new(None).wanted, None);
    }
}
