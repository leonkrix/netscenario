//! The Kernel backend as the caller sees it: it starts the supervisor of a run (D-022) and talks to it.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netscenario_core::{
    Backend, BackendInfo, Capabilities, CapabilityError, Direction, Error, Feature, LinkHandle,
    NodeCounts, NodeHandle, Program, ProgramExit, ProgramHandle, ProgramState, Refusal,
};
use netscenario_model::{Impairments, Name, Scenario};

use crate::capabilities::{Environment, Probed, capabilities_probed};
use crate::command::{
    Toolbox, has_own_rights, namespace_flags, namespaces_work, run, uses_own_rights,
};
use crate::protocol::{Exit, Request, Response, Way};
use crate::provisioner::link_address;
use crate::supervisor::FAIL_AT;
use crate::tools::{Tool, parse_tc_version};

const BACKEND: &str = "kernel";

/// How the run gets its rights to create namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// With the rights of the process if it has them and they work, else in an unprivileged user namespace.
    /// A process has them if it is root, or has `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its ambient set.
    #[default]
    Auto,
    /// In an unprivileged user namespace, where the process is root only inside.
    Unprivileged,
    /// With the rights of the process, which must be root or have `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its
    /// ambient set.
    Root,
}

/// The settings of a Kernel backend.
#[derive(Debug, Clone)]
pub struct KernelOptions {
    /// The binary that is started as the supervisor (see [`supervisor::main`](crate::supervisor::main)).
    pub supervisor: PathBuf,
    /// How long a run may last before the watchdog ends it (SAF-03).
    pub max_run_time: Duration,
    /// How many network namespaces a run may use (SAF-02).
    pub max_nodes: usize,
    /// How the rights are obtained.
    pub mode: Mode,
    /// A failpoint of the crash tests (see `NETSCENARIO_FAIL_AT`).
    #[doc(hidden)]
    pub fail_at: Option<String>,
}

impl KernelOptions {
    /// The defaults: at most 8 namespaces and 60 seconds.
    #[must_use]
    pub fn new(supervisor: impl Into<PathBuf>) -> Self {
        Self {
            supervisor: supervisor.into(),
            max_run_time: Duration::from_secs(60),
            max_nodes: 8,
            mode: Mode::Auto,
            fail_at: None,
        }
    }

    /// The defaults for `scenario`: the run may last the duration of the scenario and 60 seconds more.
    #[must_use]
    pub fn for_scenario(supervisor: impl Into<PathBuf>, scenario: &Scenario) -> Self {
        let mut options = Self::new(supervisor);
        options.max_run_time = Duration::from(scenario.duration) + Duration::from_secs(60);
        options
    }
}

/// Ends the supervisor when the time is up. Killing the `unshare` process that started it ends the first
/// process of the PID namespace, and with it everything in the run.
struct Watchdog {
    stop: Arc<(Mutex<bool>, Condvar)>,
    fired: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    fn start(child: Arc<Mutex<Child>>, limit: Duration) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, fired) = (Arc::clone(&stop), Arc::clone(&fired));
            std::thread::spawn(move || {
                let (lock, condition) = &*stop;
                let stopped = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let (guard, result) = condition
                    .wait_timeout_while(stopped, limit, |stopped| !*stopped)
                    .unwrap_or_else(PoisonError::into_inner);
                drop(guard);
                if result.timed_out() {
                    fired.store(true, Ordering::SeqCst);
                    let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
                }
            })
        };
        Self {
            stop,
            fired,
            thread: Some(thread),
        }
    }

    fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    fn stop(&mut self) {
        let (lock, condition) = &*self.stop;
        *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
        condition.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The last 16 KiB that the supervisor and its tools wrote to their standard error: what `unshare` says when
/// it is not allowed to create a namespace.
const STDERR_LIMIT: usize = 16 * 1024;

struct Session {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<Mutex<String>>,
    /// Set when the standard error of the supervisor has ended and `stderr` is complete.
    stderr_done: Arc<AtomicBool>,
    watchdog: Watchdog,
}

/// The Kernel backend: one network namespace per node, virtual Ethernet pairs as links and `netem` on each
/// direction. The supervisor starts with the first node, so a scenario that is refused creates nothing.
pub struct KernelBackend {
    options: KernelOptions,
    tools: Toolbox,
    environment: Environment,
    run_id: String,
    /// Whether the run uses the rights of this process (else a user namespace), decided once in `new`.
    own_rights: bool,
    /// What the probe of `doctor` found out about the kernel, run once when the declaration is first asked for (D-068).
    probed: std::sync::OnceLock<Probed>,
    session: Option<Session>,
}

static RUNS: AtomicU64 = AtomicU64::new(0);

impl KernelBackend {
    /// Finds the tools and reads the version of `tc`.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if `ip`, `tc`, `unshare` or `nsenter` is not installed.
    pub fn new(options: KernelOptions) -> Result<Self, Error> {
        let tools = Toolbox::locate().map_err(|missing| Error::Backend {
            backend: BACKEND.to_owned(),
            action: "find its tools".to_owned(),
            message: format!(
                "{} not found; install iproute2 (ip, tc) and util-linux (unshare, nsenter, prlimit)",
                missing.join(", ")
            ),
        })?;
        let environment = Environment {
            tc: match run(&tools.tc, &["-V"]) {
                Ok(printed) => parse_tc_version(&printed)
                    .map_or_else(|| Tool::Unreadable(printed.trim().to_owned()), Tool::Found),
                Err(error) => Tool::Unreadable(error),
            },
        };
        let run_id = format!(
            "{}-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos())
        );
        let own_rights = match options.mode {
            Mode::Auto => uses_own_rights(has_own_rights(), |user_namespace| {
                namespaces_work(&tools.unshare, user_namespace)
            }),
            Mode::Unprivileged => false,
            Mode::Root => true,
        };
        Ok(Self {
            options,
            tools,
            environment,
            run_id,
            own_rights,
            probed: std::sync::OnceLock::new(),
            session: None,
        })
    }

    /// What the probe found out about the kernel and `tc` of this machine, in the way that the run takes: it is tried
    /// once, in a throwaway namespace, the first time the declaration or a value is checked (it takes about 15 ms). If
    /// it cannot be run, nothing is known and the declaration follows the version of `tc` alone.
    fn probed(&self) -> &Probed {
        self.probed.get_or_init(|| {
            Probed::new(
                crate::doctor::probe_for_run(&self.tools, self.own_rights).unwrap_or_default(),
            )
        })
    }

    /// The tools of this machine, as far as the declaration of the capabilities depends on them.
    #[must_use]
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// The identifier of this run. It is in the command line of every process of the run, so the processes
    /// can be recognised.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    fn error(action: &str, message: impl Into<String>) -> Error {
        Error::Backend {
            backend: BACKEND.to_owned(),
            action: action.to_owned(),
            message: message.into(),
        }
    }

    fn start(&mut self) -> Result<(), Error> {
        let mut command = Command::new(&self.tools.unshare);
        command
            .args(namespace_flags(!self.own_rights))
            .arg("--")
            .arg(&self.options.supervisor)
            .args([
                "supervise",
                &self.run_id,
                &self.options.max_nodes.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A process group of its own: the signal that a terminal sends for Ctrl-C goes to the program that was
            // started from the terminal and not to the supervisor, the nodes and the agents, so the program can stop
            // the run in an orderly way and keep what it has (D-066). If the program dies, the pipe of the supervisor
            // closes and it tears the network down (D-022), as before.
            .process_group(0);
        // The tools answer in English whatever the language of the user is: the hints below look for their words
        command.env("LC_ALL", "C");
        if let Some(point) = &self.options.fail_at {
            command.env(FAIL_AT, point);
        }
        let mut child = command.spawn().map_err(|error| {
            Self::error(
                "start the supervisor",
                format!("cannot run unshare: {error}"),
            )
        })?;
        let (Some(stdin), Some(stdout), Some(stderr_pipe)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(Self::error(
                "start the supervisor",
                "no pipes to the supervisor",
            ));
        };
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_done = collect(stderr_pipe, Arc::clone(&stderr));
        let child = Arc::new(Mutex::new(child));
        let watchdog = Watchdog::start(Arc::clone(&child), self.options.max_run_time);
        self.session = Some(Session {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
            stderr_done,
            watchdog,
        });
        // The supervisor says whether it is ready, or why it refuses to run
        self.receive("start the supervisor").map(|_| ())
    }

    fn call(&mut self, action: &str, request: &Request) -> Result<Option<u32>, Error> {
        match self.exchange(action, request)? {
            Response::Ok { id } => Ok(id),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    /// Sends a request, starting the supervisor first if it is not running, and returns its answer. An
    /// answer that says `error` is an `Err`.
    fn exchange(&mut self, action: &str, request: &Request) -> Result<Response, Error> {
        if self.session.is_none() {
            self.start()?;
        }
        let line = serde_json::to_string(request)
            .map_err(|error| Self::error(action, format!("cannot write the request: {error}")))?;
        let sent = self.session.as_mut().is_some_and(|session| {
            writeln!(session.stdin, "{line}")
                .and_then(|()| session.stdin.flush())
                .is_ok()
        });
        if !sent {
            return Err(self.ended(action));
        }
        self.receive_response(action)
    }

    fn receive(&mut self, action: &str) -> Result<Option<u32>, Error> {
        match self.receive_response(action)? {
            Response::Ok { id } => Ok(id),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    fn receive_response(&mut self, action: &str) -> Result<Response, Error> {
        let mut line = String::new();
        let read = self
            .session
            .as_mut()
            .map(|session| session.stdout.read_line(&mut line));
        if !matches!(read, Some(Ok(n)) if n > 0) {
            return Err(self.ended(action));
        }
        match serde_json::from_str::<Response>(&line) {
            Ok(Response::Error { message }) => Err(Self::error(action, message)),
            Ok(response) => Ok(response),
            Err(error) => Err(Self::error(
                action,
                format!("cannot read the answer '{}': {error}", line.trim()),
            )),
        }
    }

    /// The error for a supervisor that is gone: why, as far as it can be told.
    fn ended(&mut self, action: &str) -> Error {
        let Some(session) = self.session.take() else {
            return Self::error(action, "the run is not running");
        };
        let mut session = session;
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            let status = session
                .child
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_wait();
            match status {
                Ok(Some(status)) => break Some(status.to_string()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break None,
            }
        };
        let fired = session.watchdog.fired();
        session.watchdog.stop();
        // What the supervisor wrote may still be on its way to the thread that collects it
        let patience = Instant::now() + Duration::from_secs(1);
        while !session.stderr_done.load(Ordering::SeqCst) && Instant::now() < patience {
            std::thread::sleep(Duration::from_millis(5));
        }
        let said = session
            .stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .trim()
            .to_owned();
        let mut message = if fired {
            format!(
                "the run lasted longer than {:?} and the watchdog ended it (SAF-03)",
                self.options.max_run_time
            )
        } else {
            format!(
                "the supervisor of the run ended ({})",
                status.unwrap_or_else(|| "it did not exit".to_owned())
            )
        };
        if !said.is_empty() {
            let _ = write!(message, "; it said: {said}");
        }
        if said.contains("Operation not permitted") || said.contains("uid_map") {
            message.push_str(
                "; the machine does not allow the namespaces of an unprivileged user (see \"Where it can run\" in docs/COVERAGE.md)",
            );
        }
        Self::error(action, message)
    }
}

/// Reads `pipe` into `into` on a thread of its own. The flag that it returns is set when the stream has ended,
/// so that a reader of `into` can wait until everything that was written is there.
fn collect(mut pipe: impl Read + Send + 'static, into: Arc<Mutex<String>>) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let finished = Arc::clone(&done);
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        while let Ok(n) = pipe.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let mut text = into.lock().unwrap_or_else(PoisonError::into_inner);
            text.push_str(&String::from_utf8_lossy(&buffer[..n]));
            if text.len() > STDERR_LIMIT {
                let cut = text.len() - STDERR_LIMIT;
                let cut = (cut..text.len())
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(0);
                text.drain(..cut);
            }
        }
        finished.store(true, Ordering::SeqCst);
    });
    done
}

impl KernelBackend {
    /// A direction with a duplication, and a rate with a jitter or a reordering, or an outage, is two or more `netem`
    /// in one tree, and some kernels refuse a duplicating `netem` next to another one (D-052, D-064). The kernel is
    /// asked once, and only if a scenario has such a direction; where it says no the scenario is refused before
    /// anything is built.
    fn check_duplicate_tree(&self, scenario: &Scenario, refusals: &mut Vec<Refusal>) {
        let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").map_or_else(
            |_| "this kernel".to_owned(),
            |text| format!("the kernel {}", text.trim()),
        );
        refusals.extend(duplicate_tree_refusals(
            scenario,
            || crate::doctor::duplicate_tree_answer(self.probed().steps()),
            &release,
        ));
    }
}

/// The directions of `scenario` that need a duplicating `netem` next to another one (a rate with a jitter or a
/// reordering, or an outage), refused if `ask` says that the kernel refuses it. `ask` is called at most once, and not at all if no direction needs the tree.
fn duplicate_tree_refusals(
    scenario: &Scenario,
    ask: impl FnOnce() -> Option<String>,
    release: &str,
) -> Vec<Refusal> {
    use netscenario_core::timeline::{Action, outage_directions, states_of, steps};
    let outages = outage_directions(scenario);
    let mut wanted: Vec<String> = Vec::new();
    for (index, link) in scenario.topology.links.iter().enumerate() {
        for direction in [Direction::Forward, Direction::Reverse] {
            // A direction is two queues for the whole run if any state needs two (D-059), and has the blackout
            // behind them if it has an outage, so a duplication in any state, also one that has no rate of its own,
            // is a duplicating queue next to another one
            let states = states_of(scenario, index, direction);
            let Ok(layout) =
                crate::netem::Layout::of(&states, outages.contains(&(index, direction)))
            else {
                continue;
            };
            if !layout.is_a_tree() {
                continue;
            }
            let path = if states[0].duplicate.is_some() {
                let name = if link.both.is_some() {
                    "both"
                } else if direction == Direction::Forward {
                    "forward"
                } else {
                    "reverse"
                };
                Some(format!("topology.links[{index}].{name}.duplicate"))
            } else {
                steps(scenario).iter().find_map(|step| match &step.action {
                    Action::Impair {
                        direction: moved,
                        impairments,
                        ..
                    } if step.link == index
                        && *moved == direction
                        && impairments.duplicate.is_some() =>
                    {
                        Some(format!("timeline[{}].set.duplicate", step.event))
                    }
                    _ => None,
                })
            };
            if let Some(path) = path
                && !wanted.contains(&path)
            {
                wanted.push(path);
            }
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(answer) = ask() else {
        return Vec::new();
    };
    wanted
        .into_iter()
        .map(|path| Refusal {
            feature: Feature::Duplication,
            path,
            reason: format!(
                "{release} refuses a duplicating netem next to another netem, which a direction with a rate and a jitter or a reordering, or with an outage, needs: {answer}"
            ),
            fix: "leave out the duplication, the rate, the jitter and the reordering, or the outage in this direction, use a kernel that accepts it (the check was added in 2025 and removed again later; `netscenario doctor` tries it), or the Engine backend (planned for release 0.4)".to_owned(),
        })
        .collect()
}

/// The steps of the timeline that the queues of this backend cannot follow (D-059): the jitter of a direction
/// has a normal distribution in one state and a uniform one in another. `netem` keeps the table of the
/// distribution that was loaded last, so going back would mean building the queue again, which drops the packets
/// in flight and starts its random numbers over: the backend refuses it and names the step that leads to the
/// second distribution.
fn timeline_refusals(scenario: &Scenario) -> Vec<Refusal> {
    use netscenario_core::timeline::{Action, states_of, steps};
    let steps = steps(scenario);
    let mut refusals = Vec::new();
    for (index, link) in scenario.topology.links.iter().enumerate() {
        for direction in [Direction::Forward, Direction::Reverse] {
            let states = states_of(scenario, index, direction);
            if crate::netem::Layout::of(&states, false).is_ok() {
                continue;
            }
            // The distribution the direction begins with, or the first one it gets; the first step that leads to
            // the other one is the one to name
            let mut began = states[0].jitter.as_ref().map(|jitter| jitter.distribution);
            for step in steps.iter().filter(|step| step.link == index) {
                let Action::Impair {
                    direction: moved,
                    impairments,
                    ..
                } = &step.action
                else {
                    continue;
                };
                if *moved != direction {
                    continue;
                }
                let found = impairments
                    .jitter
                    .as_ref()
                    .map(|jitter| jitter.distribution);
                match (began, found) {
                    (None, found) => began = found,
                    (Some(began), Some(found)) if began != found => {
                        refusals.push(Refusal {
                            feature: Feature::ScheduledSteps,
                            path: format!("timeline[{}]", step.event),
                            reason: format!(
                                "the jitter of link '{}' ({direction:?}) has a normal distribution at one time and a uniform one at another, and `netem` keeps the distribution that was loaded last, so the queue would have to be built again, which drops the packets in flight and starts its random numbers over",
                                link.id
                            ),
                            fix: "keep one distribution for the link through the whole run (the amount can change), or use the Engine backend (planned for release 0.4)".to_owned(),
                        });
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    refusals
}

/// The places where `scenario` asks for a jitter with the distribution `normal`: in a link and in a step of the timeline.
fn normal_jitter_paths(scenario: &Scenario) -> Vec<String> {
    use netscenario_core::timeline::{Action, steps};
    use netscenario_model::JitterDistribution;
    let normal = |impairments: &Impairments| {
        impairments
            .jitter
            .as_ref()
            .is_some_and(|jitter| jitter.distribution == JitterDistribution::Normal)
    };
    let mut paths = Vec::new();
    for (index, link) in scenario.topology.links.iter().enumerate() {
        for (name, impairments) in [
            ("forward", &link.forward),
            ("reverse", &link.reverse),
            ("both", &link.both),
        ] {
            if impairments.as_ref().is_some_and(normal) {
                paths.push(format!(
                    "topology.links[{index}].{name}.jitter.distribution"
                ));
            }
        }
    }
    for step in steps(scenario) {
        if let Action::Impair { impairments, .. } = &step.action
            && normal(impairments)
        {
            let path = format!("timeline[{}].set", step.event);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// The jitters with the distribution `normal` of `scenario`, refused if `ask` says that `tc` finds no table for it
/// (the file `normal.dist`). `ask` is called at most once, and not at all if the scenario has no such jitter.
/// `tc_lib_dir` is the value of `TC_LIB_DIR`, which the fix names if it is set (D-068).
fn normal_table_refusals(
    scenario: &Scenario,
    ask: impl FnOnce() -> Option<String>,
    tc_lib_dir: Option<&str>,
) -> Vec<Refusal> {
    let wanted = normal_jitter_paths(scenario);
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(answer) = ask() else {
        return Vec::new();
    };
    let fix = match tc_lib_dir {
        Some(dir) => format!(
            "TC_LIB_DIR is set to {dir}: put normal.dist there or point it to the directory that has it; or use the distribution `uniform`"
        ),
        None => "install the iproute2 package that has the tables (usually in /usr/lib/tc or /usr/lib/<arch>/tc), or set TC_LIB_DIR to the directory with normal.dist; or use the distribution `uniform`".to_owned(),
    };
    wanted
        .into_iter()
        .map(|path| Refusal {
            feature: Feature::Jitter,
            path,
            reason: format!(
                "a jitter with the distribution `normal` needs a table that `tc` cannot find on this machine: {answer}"
            ),
            fix: fix.clone(),
        })
        .collect()
}

/// The workloads of `scenario` that send without the checksum of UDP, refused if `ask` says that a node has no
/// `CAP_NET_RAW`. `ask` is called at most once, and not at all if no workload needs the raw socket.
fn raw_socket_refusals(scenario: &Scenario, ask: impl FnOnce() -> Option<String>) -> Vec<Refusal> {
    let wanted: Vec<String> = scenario
        .workloads
        .iter()
        .enumerate()
        .filter(|(_, workload)| {
            workload
                .as_udp_cbr()
                .is_some_and(|workload| !workload.udp_checksum.is_on())
        })
        .map(|(index, _)| format!("workloads[{index}].udp-checksum"))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(answer) = ask() else {
        return Vec::new();
    };
    wanted
        .into_iter()
        .map(|path| Refusal {
            feature: Feature::UdpWithoutChecksum,
            path,
            reason: format!(
                "a stream without the checksum of UDP is sent through a raw socket, and a node of this machine cannot open one: {answer}"
            ),
            fix: "run netscenario as root, or with CAP_NET_RAW among the ambient capabilities (`setpriv --ambient-caps +net_raw`), or without `sudo` in a user namespace, which has it (`netscenario doctor` tries it); or leave out `udp-checksum: false`, or use the Engine backend (planned for release 0.4)".to_owned(),
        })
        .collect()
}

impl Backend for KernelBackend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn capabilities(&self) -> Capabilities {
        capabilities_probed(&self.environment, self.probed())
    }

    fn check_values(&self, scenario: &Scenario) -> Result<(), CapabilityError> {
        let mut refusals = Vec::new();
        for (index, link) in scenario.topology.links.iter().enumerate() {
            for (name, impairments) in [
                ("forward", &link.forward),
                ("reverse", &link.reverse),
                ("both", &link.both),
            ] {
                let Some(impairments) = impairments else {
                    continue;
                };
                let base = format!("topology.links[{index}].{name}");
                let times = [
                    (Feature::FixedDelay, "delay", impairments.delay),
                    (
                        Feature::Jitter,
                        "jitter.amount",
                        impairments.jitter.as_ref().map(|j| j.amount),
                    ),
                ];
                for (feature, key, time) in times {
                    let Some(time) = time else { continue };
                    if !time.as_nanos().is_multiple_of(1000) {
                        refusals.push(Refusal {
                            feature,
                            path: format!("{base}.{key}"),
                            reason: format!("netem works in whole microseconds and {time} is not a multiple of 1us"),
                            fix: "use a multiple of 1us, or the Engine backend (planned for release 0.4)".to_owned(),
                        });
                    }
                }
            }
        }
        self.check_duplicate_tree(scenario, &mut refusals);
        refusals.extend(timeline_refusals(scenario));
        refusals.extend(raw_socket_refusals(scenario, || {
            crate::doctor::raw_socket_answer(self.probed().steps())
        }));
        refusals.extend(normal_table_refusals(
            scenario,
            || self.probed().refused("tables").map(str::to_owned),
            std::env::var("TC_LIB_DIR").ok().as_deref(),
        ));
        if refusals.is_empty() {
            Ok(())
        } else {
            Err(CapabilityError::new(BACKEND, refusals))
        }
    }

    fn create_node(&mut self, name: &Name) -> Result<NodeHandle, Error> {
        let action = format!("create the node '{name}'");
        let id = self.call(
            &action,
            &Request::CreateNode {
                name: name.to_string(),
            },
        )?;
        id.map(NodeHandle)
            .ok_or_else(|| Self::error(&action, "the supervisor gave no handle"))
    }

    fn create_link(&mut self, name: &Name, between: [NodeHandle; 2]) -> Result<LinkHandle, Error> {
        let action = format!("create the link '{name}'");
        let request = Request::CreateLink {
            name: name.to_string(),
            between: [between[0].0, between[1].0],
        };
        let id = self.call(&action, &request)?;
        id.map(LinkHandle)
            .ok_or_else(|| Self::error(&action, "the supervisor gave no handle"))
    }

    fn apply(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), Error> {
        let request = Request::Apply {
            link: link.0,
            direction: Way::from(direction),
            impairments: impairments.clone(),
            seed,
        };
        self.call("apply the impairments of a link", &request)
            .map(|_| ())
    }

    fn plan_states(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        states: &[Impairments],
    ) -> Result<(), Error> {
        let request = Request::Plan {
            link: link.0,
            direction: Way::from(direction),
            states: states.to_vec(),
        };
        self.call("plan the states of a direction", &request)
            .map(|_| ())
    }

    fn plan_outage(&mut self, link: LinkHandle, direction: Direction) -> Result<(), Error> {
        let request = Request::PlanOutage {
            link: link.0,
            direction: Way::from(direction),
        };
        self.call("plan an outage of a direction", &request)
            .map(|_| ())
    }

    fn change(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
    ) -> Result<(), Error> {
        let request = Request::Change {
            link: link.0,
            direction: Way::from(direction),
            impairments: impairments.clone(),
        };
        self.call("change the impairments of a direction", &request)
            .map(|_| ())
    }

    fn set_outage(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        on: bool,
    ) -> Result<(), Error> {
        let request = Request::SetOutage {
            link: link.0,
            direction: Way::from(direction),
            on,
        };
        self.call(
            if on {
                "begin an outage"
            } else {
                "end an outage"
            },
            &request,
        )
        .map(|_| ())
    }

    fn set_link(&mut self, link: LinkHandle, up: bool) -> Result<(), Error> {
        let request = Request::SetLink { link: link.0, up };
        self.call(
            if up {
                "bring a link up"
            } else {
                "take a link down"
            },
            &request,
        )
        .map(|_| ())
    }

    fn queue_packets(
        &mut self,
        link: LinkHandle,
        direction: Direction,
    ) -> Result<Option<u64>, Error> {
        let action = "count the packets of a queue";
        let request = Request::QueuePackets {
            link: link.0,
            direction: Way::from(direction),
        };
        match self.exchange(action, &request)? {
            Response::Count { packets } => Ok(Some(packets)),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    fn queue_backlog(
        &mut self,
        link: LinkHandle,
        direction: Direction,
    ) -> Result<Option<u64>, Error> {
        let action = "count the packets in a queue";
        let request = Request::QueueBacklog {
            link: link.0,
            direction: Way::from(direction),
        };
        match self.exchange(action, &request)? {
            Response::Count { packets } => Ok(Some(packets)),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    fn node_counts(
        &mut self,
        link: LinkHandle,
        direction: Direction,
    ) -> Result<Option<NodeCounts>, Error> {
        let action = "count the frames of a node";
        let request = Request::NodeCounts {
            link: link.0,
            direction: Way::from(direction),
        };
        match self.exchange(action, &request)? {
            Response::NodeCounts {
                frames_received,
                receive_buffer_drops,
            } => Ok(Some(NodeCounts::new(frames_received, receive_buffer_drops))),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    fn address(&self, link: LinkHandle, end: usize) -> Result<std::net::Ipv4Addr, Error> {
        if end > 1 {
            return Err(Self::error(
                "find an address",
                format!("a link has two ends, not {}", end + 1),
            ));
        }
        Ok(link_address(link.0, end))
    }

    fn describe(&self) -> BackendInfo {
        let mut details = std::collections::BTreeMap::new();
        details.insert(
            "tc".to_owned(),
            match &self.environment.tc {
                Tool::Found(version) => format!("iproute2 {version}"),
                Tool::Unreadable(printed) => format!("unreadable: {printed}"),
                Tool::Missing => "missing".to_owned(),
            },
        );
        details.insert(
            "rights".to_owned(),
            if self.own_rights {
                "own rights"
            } else {
                "user namespace"
            }
            .to_owned(),
        );
        BackendInfo {
            name: BACKEND.to_owned(),
            details,
        }
    }

    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &Program,
    ) -> Result<ProgramHandle, Error> {
        let action = "start a program";
        let request = Request::StartProgram {
            node: node.0,
            argv: program.argv.clone(),
            memory_limit: program.memory_limit,
        };
        let id = self.call(action, &request)?;
        id.map(ProgramHandle)
            .ok_or_else(|| Self::error(action, "the supervisor gave no handle"))
    }

    fn poll_program(
        &mut self,
        program: ProgramHandle,
        wait: Duration,
    ) -> Result<ProgramState, Error> {
        let action = "ask a program what it did";
        let request = Request::Poll {
            program: program.0,
            wait_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
        };
        match self.exchange(action, &request)? {
            Response::Program {
                lines,
                exit,
                stderr,
                truncated,
            } => Ok(ProgramState {
                lines,
                exit: exit.map(|exit| match exit {
                    Exit::Code(code) => ProgramExit::Code(code),
                    Exit::Signal(signal) => ProgramExit::Signal(signal),
                }),
                stderr,
                truncated,
            }),
            other => Err(Self::error(
                action,
                format!("the supervisor gave an answer that was not asked for: {other:?}"),
            )),
        }
    }

    fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error> {
        let request = Request::SendLine {
            program: program.0,
            line: line.to_owned(),
        };
        self.call("write to a program", &request).map(|_| ())
    }

    fn notes(&mut self) -> Vec<String> {
        if self.session.is_none() {
            return Vec::new();
        }
        match self.exchange("ask for the notes of the run", &Request::Notes) {
            Ok(Response::Notes { lines }) => lines,
            _ => Vec::new(),
        }
    }

    fn teardown(&mut self) -> Result<(), Error> {
        if self.session.is_none() {
            return Ok(());
        }
        let result = self.call("tear down the run", &Request::Teardown);
        let Some(mut session) = self.session.take() else {
            // The supervisor was gone before it could answer, which means the kernel has cleaned up
            return Ok(());
        };
        session.watchdog.stop();
        let child = Arc::clone(&session.child);
        let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
        }
        result.map(|_| ())
    }
}

impl Drop for KernelBackend {
    fn drop(&mut self) {
        let _ = self.teardown();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const HEAD: &str = "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n";
    const WORKLOAD: &str = "workloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

    fn scenario_of(link: &str) -> Scenario {
        Scenario::from_yaml(&format!("{HEAD}{link}{WORKLOAD}"), "t.yaml").unwrap()
    }

    /// The jitter with the distribution `normal` is refused where `tc` finds no table (D-068): in a link and in a step,
    /// with the answer, and the kernel is not asked for a scenario that has none.
    #[test]
    fn a_normal_jitter_is_refused_where_tc_finds_no_table_for_it() {
        let normal =
            "      forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal } }\n";
        let refusals = normal_table_refusals(
            &scenario_of(normal),
            || Some("No such file or directory".to_owned()),
            None,
        );
        assert_eq!(refusals.len(), 1);
        assert_eq!(
            refusals[0].path,
            "topology.links[0].forward.jitter.distribution"
        );
        assert!(refusals[0].reason.contains("No such file or directory"));
        assert!(refusals[0].fix.contains("TC_LIB_DIR"));
        // The directory the user set is named
        let set = normal_table_refusals(
            &scenario_of(normal),
            || Some("x".to_owned()),
            Some("/nowhere"),
        );
        assert!(
            set[0].fix.contains("TC_LIB_DIR is set to /nowhere"),
            "{}",
            set[0].fix
        );
        // A step that leads to one is named by its place
        let in_step = Scenario::from_yaml(
            &format!(
                "{HEAD}      forward: {{ delay: 20ms }}\n{WORKLOAD}timeline:\n  - {{ at: 2s, link: l, set: {{ forward: {{ jitter: {{ amount: 2ms, distribution: normal }} }} }} }}\n"
            ),
            "t.yaml",
        )
        .unwrap();
        let refusals = normal_table_refusals(&in_step, || Some("gone".to_owned()), None);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert_eq!(refusals[0].path, "timeline[0].set");
        // Canaries: a uniform jitter, none, and a table that is there ask nothing or say nothing
        let uniform = scenario_of("      forward: { delay: 20ms, jitter: { amount: 5ms } }\n");
        assert_eq!(
            normal_table_refusals(&uniform, || panic!("asked"), None),
            Vec::new()
        );
        assert_eq!(
            normal_table_refusals(&scenario_of(normal), || None, None),
            Vec::new()
        );
    }

    const REFUSED: &str = "Error: netem: cannot mix duplicating netems with other netems in tree.";

    /// D-052: a direction that needs the tree is refused with the answer of the kernel, where the kernel says no.
    #[test]
    fn a_duplicating_tree_is_refused_where_the_kernel_refuses_it() {
        let scenario = scenario_of(
            "      forward: { delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit, duplicate: 5% }\n      reverse: { delay: 20ms, reorder: { share: 25% }, rate: 2mbit, duplicate: 5% }\n",
        );
        let refusals =
            duplicate_tree_refusals(&scenario, || Some(REFUSED.to_owned()), "the kernel 7.1.0");
        assert_eq!(refusals.len(), 2);
        assert_eq!(refusals[0].path, "topology.links[0].forward.duplicate");
        assert_eq!(refusals[1].path, "topology.links[0].reverse.duplicate");
        assert!(
            refusals[0].reason.contains("the kernel 7.1.0")
                && refusals[0].reason.contains("cannot mix")
        );
        assert!(refusals[0].fix.contains("doctor"));
    }

    /// A duplication that a step leads to counts too: a direction that is two queues for the run (a rate with a jitter
    /// at some time) has the duplicating queue next to another one in every state, so the kernel is asked, and
    /// the step is named.
    #[test]
    fn a_duplication_that_a_step_leads_to_in_two_queues_asks_the_kernel() {
        let with = |link: &str, timeline: &str| {
            Scenario::from_yaml(
                &format!("{HEAD}{link}{WORKLOAD}timeline:\n{timeline}"),
                "t.yaml",
            )
            .unwrap()
        };
        let two_queues = "      forward: { delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit }\n";
        let refusals = duplicate_tree_refusals(
            &with(
                two_queues,
                "  - { at: 2s, link: l, set: { forward: { duplicate: 5% } } }\n",
            ),
            || Some(REFUSED.to_owned()),
            "the kernel 7.1.0",
        );
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert_eq!(refusals[0].path, "timeline[0].set.duplicate");
        // The same duplication in a direction that stays one queue asks nothing
        let one_queue = duplicate_tree_refusals(
            &with(
                "      forward: { delay: 20ms }\n",
                "  - { at: 2s, link: l, set: { forward: { duplicate: 5% } } }\n",
            ),
            || panic!("the kernel is asked"),
            "k",
        );
        assert_eq!(one_queue, Vec::new());
    }

    /// A jitter that is normal at one time and uniform at another cannot be had in the queues of this backend: the step
    /// that leads to the second distribution is named. The same distribution all the time, or none in between, is
    /// fine.
    #[test]
    fn a_timeline_that_changes_the_distribution_of_a_jitter_is_refused_at_the_step() {
        let with = |link: &str, timeline: &str| {
            Scenario::from_yaml(
                &format!("{HEAD}{link}{WORKLOAD}timeline:\n{timeline}"),
                "t.yaml",
            )
            .unwrap()
        };
        let normal =
            "      forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal } }\n";
        let refusals = timeline_refusals(&with(
            normal,
            "  - { at: 2s, link: l, set: { forward: { delay: 30ms } } }\n  - { at: 4s, link: l, set: { forward: { jitter: { amount: 5ms } } } }\n",
        ));
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert_eq!(refusals[0].path, "timeline[1]");
        assert!(
            refusals[0]
                .reason
                .contains("normal distribution at one time")
        );
        // Canaries: the amount may change, the jitter may go away and come back in the same distribution, and a
        // direction that begins without a jitter may take one
        for timeline in [
            "  - { at: 2s, link: l, set: { forward: { jitter: { amount: 2ms, distribution: normal } } } }\n",
            "  - { at: 2s, link: l, set: { forward: { jitter: null } } }\n  - { at: 4s, link: l, set: { forward: { jitter: { amount: 1ms, distribution: normal } } } }\n",
        ] {
            assert_eq!(
                timeline_refusals(&with(normal, timeline)),
                Vec::new(),
                "{timeline}"
            );
        }
        assert_eq!(
            timeline_refusals(&with(
                "      forward: { delay: 20ms }\n",
                "  - { at: 2s, link: l, set: { forward: { jitter: { amount: 2ms } } } }\n"
            )),
            Vec::new()
        );
    }

    /// A stream without the checksum of UDP needs a raw socket in the node, and a machine whose nodes cannot
    /// open one refuses it before anything is built, with the capability named. A stream with the checksum does not
    /// even ask.
    #[test]
    fn a_stream_without_the_checksum_is_refused_where_a_node_has_no_raw_socket() {
        let with = |options: &str| {
            Scenario::from_yaml(
                &format!("{HEAD}{WORKLOAD}  - {{ id: v, kind: udp-cbr, from: b, to: a, rate: 1mbit, packet-size: 1200b{options} }}\n"),
                "t.yaml",
            )
            .unwrap()
        };
        let refusals = raw_socket_refusals(&with(", udp-checksum: false"), || {
            Some("CAP_NET_RAW is not among the effective capabilities of a node".to_owned())
        });
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].path, "workloads[1].udp-checksum");
        assert_eq!(refusals[0].feature, Feature::UdpWithoutChecksum);
        assert!(refusals[0].reason.contains("CAP_NET_RAW"));
        assert!(refusals[0].fix.contains("net_raw") && refusals[0].fix.contains("doctor"));
        // Canary: where the node has the capability nothing is refused, and a plain stream does not ask
        assert_eq!(
            raw_socket_refusals(&with(", udp-checksum: false"), || None),
            Vec::new()
        );
        assert_eq!(
            raw_socket_refusals(&with(", udp-checksum: true"), || panic!("asked")),
            Vec::new()
        );
    }

    /// Canary: a kernel that accepts it refuses nothing, and a direction that does not need two queues does
    /// not even ask: a duplication alone, a duplication with a jitter but no rate, a rate and a jitter without
    /// it, a duplication with a rate and a delay.
    #[test]
    fn nothing_is_refused_that_does_not_need_the_tree_or_where_the_kernel_accepts_it() {
        let needs = scenario_of(
            "      both: { delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit, duplicate: 5% }\n",
        );
        assert_eq!(duplicate_tree_refusals(&needs, || None, "k"), Vec::new());
        for link in [
            "      both: { duplicate: 5% }\n",
            "      both: { delay: 20ms, jitter: { amount: 5ms }, duplicate: 5% }\n",
            "      both: { delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit }\n",
            "      both: { delay: 20ms, rate: 2mbit, duplicate: 5% }\n",
        ] {
            let scenario = scenario_of(link);
            let refusals = duplicate_tree_refusals(
                &scenario,
                || panic!("the kernel is asked for {link}"),
                "k",
            );
            assert_eq!(refusals, Vec::new(), "{link}");
        }
    }
}
