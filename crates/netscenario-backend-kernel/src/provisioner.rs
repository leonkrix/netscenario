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

use crate::batch::{ANSWER_WITHIN, Batch, Failure};
use crate::command::{Toolbox, run};
use crate::netem;
use crate::protocol::Way;

/// The most links a run can address.
const MAX_LINKS: usize = 200;

/// The address of one end of a link: link `n` uses `10.100.n.0/30`, the first node `.1` and the second `.2`.
/// The caller and the supervisor both compute it, so that the caller needs no question to the supervisor.
pub(crate) fn link_address(link: u32, end: usize) -> std::net::Ipv4Addr {
    let third = u8::try_from(link).unwrap_or(u8::MAX);
    let host = if end == 0 { 1 } else { 2 };
    std::net::Ipv4Addr::new(10, 100, third, host)
}

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
    /// The queues planned for the directions that change in the run (D-059), by link and direction.
    layouts: std::collections::HashMap<(u32, Way), netem::Layout>,
    /// The `tc -batch` that makes the changes of the node whose holder has this process id (D-063). `None`: there is
    /// none, because it could not be started or stopped working, and a process makes the changes of that node.
    batches: HashMap<u32, Option<Batch>>,
    /// What the backend says about the run besides its result: where it did something other than the usual way.
    notes: Vec<String>,
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
            layouts: std::collections::HashMap::new(),
            batches: HashMap::new(),
            notes: Vec::new(),
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
        let addresses = [0, 1].map(|end| link_address(id(index).unwrap_or(0), end).to_string());
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

    /// Plans the queues of a direction for every state it will have (D-059). It must come before `apply`, which
    /// builds them.
    pub(crate) fn plan(
        &mut self,
        link: u32,
        way: Way,
        states: &[Impairments],
    ) -> Result<(), String> {
        if self.links.get(link as usize).is_none() {
            return Err(format!("the link {link} does not exist in this run"));
        }
        let layout = netem::Layout::of(states)?;
        self.layouts.insert((link, way), layout);
        Ok(())
    }

    /// Writes a state into the queues of a direction that `apply` built from a plan: the whole state, in the
    /// queues that are there, so no packet is lost to a rebuild and the random numbers go on (D-048, D-059).
    pub(crate) fn change(
        &mut self,
        link: u32,
        way: Way,
        impairments: &Impairments,
    ) -> Result<(), String> {
        let layout = self.layouts.get(&(link, way)).ok_or_else(|| {
            format!("the direction of link {link} was not planned for changes, so its queues may not fit the new state")
        })?;
        if !layout.fits(impairments) {
            return Err(
                "the new state needs a queue that this direction was not built with".to_owned(),
            );
        }
        let stages = netem::change(layout, impairments)?;
        // The arguments are put into one line for the batch, so none may hold a space
        if stages
            .first
            .iter()
            .chain(stages.second.iter().flatten())
            .any(|argument| argument.contains(char::is_whitespace))
        {
            return Err("an argument of the change holds a space".to_owned());
        }
        let (pid, interface) = self.interface_of(link, way)?;
        let mut commands = vec![format!(
            "qdisc change dev {interface} root netem {}",
            stages.first.join(" ")
        )];
        if let Some(second) = &stages.second {
            commands.push(format!(
                "qdisc change dev {interface} parent 1:1 handle 2: netem {}",
                second.join(" ")
            ));
        }
        if self.change_through_batch(pid, &commands)? {
            return Ok(());
        }
        for command in &commands {
            let words: Vec<&str> = command.split(' ').collect();
            self.in_node(pid, &self.tools.tc, &words)?;
        }
        Ok(())
    }

    /// Makes a change through the batch of the node, if it has one that works. `Ok(false)`: there is none, or it
    /// stopped working just now and has been ended, and the caller makes the change with a process (the change is
    /// the whole state, so doing it again is the same as doing it once).
    fn change_through_batch(&mut self, pid: u32, commands: &[String]) -> Result<bool, String> {
        let Some(Some(batch)) = self.batches.get_mut(&pid) else {
            return Ok(false);
        };
        match batch.run(commands, ANSWER_WITHIN) {
            Ok(()) => Ok(true),
            Err(Failure::Refused(message)) => Err(format!(
                "`tc` refused the change `{}`: {message}",
                commands.join("; ")
            )),
            Err(Failure::Broken(why)) => {
                batch.end();
                self.batches.insert(pid, None);
                self.notes.push(format!(
                    "the `tc` that makes the changes of a node stopped working ({why}); a `tc` process for each change did the rest, which takes about 2 ms instead of 0.03 ms"
                ));
                Ok(false)
            }
        }
    }

    /// Starts the batch of a node, once. A batch that cannot be started is a note and not an error: the changes
    /// are made by a process then, which is slower and just as exact.
    fn start_batch(&mut self, pid: u32) {
        if self.batches.contains_key(&pid) {
            return;
        }
        let tc = self.tools.tc.to_string_lossy().into_owned();
        let arguments = vec![
            format!("--net=/proc/{pid}/ns/net"),
            "--".to_owned(),
            tc,
            "-force".to_owned(),
            "-batch".to_owned(),
            "-".to_owned(),
        ];
        let batch = match Batch::start(&self.tools.nsenter, &arguments, ANSWER_WITHIN) {
            Ok(batch) => Some(batch),
            Err(why) => {
                self.notes.push(format!(
                    "the `tc` that makes the changes of a node could not be started ({why}); a `tc` process for each change did them, which takes about 2 ms instead of 0.03 ms"
                ));
                None
            }
        };
        self.batches.insert(pid, batch);
    }

    /// What the backend says about the run besides its result (D-063).
    pub(crate) fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }

    /// The process that holds the namespace of the node that sends in a direction, and its interface.
    fn interface_of(&self, link: u32, way: Way) -> Result<(u32, String), String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        Ok((self.nodes[end.node].pid, end.interface.clone()))
    }

    pub(crate) fn apply(
        &mut self,
        link: u32,
        way: Way,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), String> {
        self.failpoints.hit("apply:start");
        let link_index = link;
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        // The traffic of a direction leaves through the interface of its sender
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        // A direction that is planned for changes is built for all its states, also if this one impairs nothing
        let planned = self.layouts.contains_key(&(link_index, way));
        let stages = if let Some(layout) = self.layouts.get(&(link_index, way)) {
            netem::create(layout, impairments, seed)?
        } else if let Some(stages) = netem::arguments(impairments, seed)? {
            stages
        } else {
            return Ok(());
        };
        let pid = self.nodes[end.node].pid;
        let interface = end.interface.as_str();
        // `apply` builds the direction once, at time zero. The changes over time of release 0.2 have to remove
        // the child when a change no longer needs it, because `replace` of the root keeps it.
        let mut root = vec!["qdisc", "replace", "dev", interface, "root"];
        if stages.second.is_some() {
            root.extend(["handle", "1:"]);
        }
        root.push("netem");
        root.extend(stages.first.iter().map(String::as_str));
        self.in_node(pid, &self.tools.tc, &root)?;
        if let Some(second) = &stages.second {
            let mut child = vec![
                "qdisc", "replace", "dev", interface, "parent", "1:1", "handle", "2:", "netem",
            ];
            child.extend(second.iter().map(String::as_str));
            self.in_node(pid, &self.tools.tc, &child)?;
        }
        // A direction that changes in the run gets the `tc` that makes its changes before the workloads start, so
        // that its first change does not pay for the start of a process
        if planned {
            self.start_batch(pid);
        }
        self.failpoints.hit("apply:end");
        Ok(())
    }

    /// How many packets entered the queue that holds the loss of a direction: the root queue of the interface
    /// of its sender (D-036). The packets that were sent on, the ones the queue dropped and the ones it still
    /// holds are all packets that drew a number for the loss, so together they are the packets that entered.
    pub(crate) fn queue_packets(&self, link: u32, way: Way) -> Result<u64, String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        let pid = self.nodes[end.node].pid;
        let shown = self.in_node(
            pid,
            &self.tools.tc,
            &["-j", "-s", "qdisc", "show", "dev", end.interface.as_str()],
        )?;
        packets_entered(&shown)
    }

    /// What the node at the receiving end of a direction has counted (D-040): the frames that arrived at the
    /// interface of the link, and the UDP datagrams that the node dropped for a full receive buffer. Both are
    /// read from `/proc` of the process that holds the namespace of the node, which shows that namespace.
    pub(crate) fn node_counts(&self, link: u32, way: Way) -> Result<(u64, u64), String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[1],
            Way::Reverse => &link.ends[0],
        };
        let pid = self.nodes[end.node].pid;
        let read = |file: &str| {
            fs::read_to_string(format!("/proc/{pid}/net/{file}"))
                .map_err(|error| format!("cannot read /proc/{pid}/net/{file}: {error}"))
        };
        Ok((
            frames_received(&read("dev")?, &end.interface)?,
            udp_receive_buffer_errors(&read("snmp")?)?,
        ))
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
        for (_, batch) in self.batches.drain() {
            if let Some(mut batch) = batch {
                batch.end();
            }
        }
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

/// The packets that entered the root queue, from the JSON of `tc -j -s qdisc show dev <interface>`: what it
/// sent on (`packets`), dropped (`drops`) and still holds (`qlen`). A child queue (the delay and jitter stage
/// of a direction that also has a rate) is behind the root and does not draw the loss, so it is not counted.
fn packets_entered(json: &str) -> Result<u64, String> {
    let qdiscs: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("cannot read the statistics of the queue: {error}"))?;
    let root = qdiscs
        .as_array()
        .and_then(|qdiscs| {
            qdiscs
                .iter()
                .find(|qdisc| qdisc.get("root").and_then(serde_json::Value::as_bool) == Some(true))
        })
        .ok_or_else(|| "the interface has no root queue".to_owned())?;
    if root.get("kind").and_then(serde_json::Value::as_str) != Some("netem") {
        return Err("the root queue of the interface is not a netem queue".to_owned());
    }
    let count = |name: &str| {
        root.get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("the statistics of the queue have no '{name}'"))
    };
    Ok(count("packets")? + count("drops")? + count("qlen")?)
}

/// The frames that arrived at `interface`, from the text of `/proc/net/dev`: the second number after the name.
fn frames_received(text: &str, interface: &str) -> Result<u64, String> {
    text.lines()
        .find_map(|line| {
            let (name, counters) = line.split_once(':')?;
            (name.trim() == interface).then_some(counters)
        })
        .ok_or_else(|| format!("/proc/net/dev has no interface '{interface}'"))?
        .split_whitespace()
        .nth(1)
        .and_then(|packets| packets.parse().ok())
        .ok_or_else(|| format!("cannot read the frames of '{interface}' in /proc/net/dev"))
}

/// The UDP datagrams dropped for a full receive buffer, from the text of `/proc/net/snmp`: the column
/// `RcvbufErrors` of the line `Udp:`, whose names come on the line before the numbers.
fn udp_receive_buffer_errors(text: &str) -> Result<u64, String> {
    let mut lines = text.lines().filter(|line| line.starts_with("Udp:"));
    let (Some(names), Some(values)) = (lines.next(), lines.next()) else {
        return Err("/proc/net/snmp has no counters for UDP".to_owned());
    };
    names
        .split_whitespace()
        .position(|name| name == "RcvbufErrors")
        .and_then(|at| values.split_whitespace().nth(at))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "/proc/net/snmp has no RcvbufErrors for UDP".to_owned())
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

    /// What `tc -j -s qdisc show` printed for a root queue with a child behind it (a direction with a loss, a
    /// rate and a jitter), after 3 packets had entered the root: 1 sent on, 2 dropped.
    const ROOT_AND_CHILD: &str = r#"[{"kind":"netem","handle":"1:","root":true,"refcnt":17,"options":{"limit":1000},"bytes":90,"packets":1,"drops":2,"overlimits":0,"requeues":0,"backlog":0,"qlen":0},{"kind":"netem","handle":"2:","parent":"1:1","options":{"limit":1000},"bytes":90,"packets":1,"drops":0,"overlimits":0,"requeues":0,"backlog":0,"qlen":0}]"#;

    #[test]
    fn the_packets_that_entered_are_those_sent_on_dropped_and_held() {
        assert_eq!(packets_entered(ROOT_AND_CHILD), Ok(3));
        // A packet that still waits in the queue entered it too
        let waiting = ROOT_AND_CHILD.replace(r#""qlen":0},{"kind""#, r#""qlen":4},{"kind""#);
        assert_eq!(packets_entered(&waiting), Ok(7));
    }

    #[test]
    fn canary_the_child_is_not_counted_and_what_is_not_a_netem_root_is_refused() {
        // The child has 1 packet and no drops: counting it as well would give 4
        assert_ne!(packets_entered(ROOT_AND_CHILD), Ok(4));
        let only_child =
            r#"[{"kind":"netem","handle":"2:","parent":"1:1","packets":1,"drops":0,"qlen":0}]"#;
        assert!(
            packets_entered(only_child)
                .unwrap_err()
                .contains("no root queue")
        );
        let other =
            r#"[{"kind":"fq_codel","handle":"1:","root":true,"packets":1,"drops":0,"qlen":0}]"#;
        assert!(packets_entered(other).unwrap_err().contains("not a netem"));
        let without = r#"[{"kind":"netem","handle":"1:","root":true,"packets":1,"qlen":0}]"#;
        assert!(packets_entered(without).unwrap_err().contains("'drops'"));
        assert!(
            packets_entered("not json")
                .unwrap_err()
                .contains("cannot read")
        );
        assert!(packets_entered("[]").unwrap_err().contains("no root queue"));
    }

    /// What `/proc/net/dev` printed in a node with a loopback and one end of a link: 12 frames arrived at `np1`.
    const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
  np1:    1176      12    0    0    0     0          0         0      588       6    0    0    0     0       0          0
";

    #[test]
    fn the_frames_of_an_interface_are_the_second_number_after_its_name() {
        assert_eq!(frames_received(NET_DEV, "np1"), Ok(12));
        assert_eq!(frames_received(NET_DEV, "lo"), Ok(0));
    }

    /// Canary: another interface is not mistaken for the asked one (a name that is a prefix of another), and an
    /// interface that is not there is an error and not a zero.
    #[test]
    fn canary_a_missing_interface_is_an_error_and_a_prefix_is_not_a_match() {
        assert!(
            frames_received(NET_DEV, "np")
                .unwrap_err()
                .contains("no interface 'np'")
        );
        assert!(frames_received(NET_DEV, "np0").is_err());
        assert!(frames_received("", "np1").is_err());
        assert!(
            frames_received("  np1: 1 x", "np1")
                .unwrap_err()
                .contains("cannot read")
        );
    }

    /// What `/proc/net/snmp` printed: 3 datagrams dropped for a full buffer (the column is the fifth).
    const SNMP: &str = "Ip: Forwarding DefaultTTL InReceives
Ip: 1 64 40
Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
Udp: 100 1 4 90 3 0 1 0 0
UdpLite: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
UdpLite: 0 0 0 0 9 0 0 0 0
";

    #[test]
    fn the_datagrams_dropped_for_a_full_buffer_are_read_by_the_name_of_their_column() {
        assert_eq!(udp_receive_buffer_errors(SNMP), Ok(3));
        // Not the line of UdpLite, and not the column next to it
        assert_ne!(udp_receive_buffer_errors(SNMP), Ok(9));
        assert_ne!(udp_receive_buffer_errors(SNMP), Ok(0));
        assert!(
            udp_receive_buffer_errors("Ip: A\nIp: 1\n")
                .unwrap_err()
                .contains("no counters for UDP")
        );
        assert!(
            udp_receive_buffer_errors("Udp: InDatagrams\nUdp: 5\n")
                .unwrap_err()
                .contains("no RcvbufErrors")
        );
    }

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
