//! The Kernel backend as the caller sees it: it starts the supervisor of a run (D-022) and talks to it.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netscenario_core::{
    Backend, BackendInfo, Capabilities, CapabilityError, Direction, Error, Feature, LinkHandle,
    NodeHandle, Program, ProgramExit, ProgramHandle, ProgramState, Refusal,
};
use netscenario_model::{Impairments, Name, Scenario};

use crate::capabilities::{Environment, capabilities};
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
            session: None,
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
            .stderr(Stdio::piped());
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

impl Backend for KernelBackend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn capabilities(&self) -> Capabilities {
        capabilities(&self.environment)
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
