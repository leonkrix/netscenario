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
    Backend, Capabilities, CapabilityError, Direction, Error, Feature, LinkHandle, NodeHandle,
    Program, ProgramHandle, Refusal,
};
use netscenario_model::{Impairments, Name, Scenario};

use crate::capabilities::{Environment, capabilities};
use crate::command::{Toolbox, run};
use crate::protocol::{Request, Response, Way};
use crate::supervisor::FAIL_AT;
use crate::tools::{Tool, parse_tc_version};

const BACKEND: &str = "kernel";

/// How the run gets its rights to create namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// As root if the process is root, else in an unprivileged user namespace.
    #[default]
    Auto,
    /// In an unprivileged user namespace, where the process is root only inside.
    Unprivileged,
    /// With the rights of the process, which must be root or have `CAP_NET_ADMIN` and `CAP_SYS_ADMIN`.
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
    watchdog: Watchdog,
}

/// The Kernel backend: one network namespace per node, virtual Ethernet pairs as links and `netem` on each
/// direction. The supervisor starts with the first node, so a scenario that is refused creates nothing.
pub struct KernelBackend {
    options: KernelOptions,
    tools: Toolbox,
    environment: Environment,
    run_id: String,
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
                "{} not found; install iproute2 (ip, tc) and util-linux (unshare, nsenter)",
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
        Ok(Self {
            options,
            tools,
            environment,
            run_id,
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
        let root = match self.options.mode {
            Mode::Auto => running_as_root(),
            Mode::Unprivileged => false,
            Mode::Root => true,
        };
        if !root {
            command.args(["--user", "--map-root-user"]);
        }
        command
            .args([
                "--net",
                "--mount",
                "--pid",
                "--fork",
                "--kill-child",
                "--mount-proc",
                "--",
            ])
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
        collect(stderr_pipe, Arc::clone(&stderr));
        let child = Arc::new(Mutex::new(child));
        let watchdog = Watchdog::start(Arc::clone(&child), self.options.max_run_time);
        self.session = Some(Session {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
            watchdog,
        });
        // The supervisor says whether it is ready, or why it refuses to run
        self.receive("start the supervisor").map(|_| ())
    }

    fn call(&mut self, action: &str, request: &Request) -> Result<Option<u32>, Error> {
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
        self.receive(action)
    }

    fn receive(&mut self, action: &str) -> Result<Option<u32>, Error> {
        let mut line = String::new();
        let read = self
            .session
            .as_mut()
            .map(|session| session.stdout.read_line(&mut line));
        if !matches!(read, Some(Ok(n)) if n > 0) {
            return Err(self.ended(action));
        }
        match serde_json::from_str::<Response>(&line) {
            Ok(Response::Ok { id }) => Ok(id),
            Ok(Response::Error { message }) => Err(Self::error(action, message)),
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

fn collect(mut pipe: impl Read + Send + 'static, into: Arc<Mutex<String>>) {
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
    });
}

/// Whether this process is root, from `/proc/self/status` (no `unsafe` call to `geteuid`).
fn running_as_root() -> bool {
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

    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &Program,
    ) -> Result<ProgramHandle, Error> {
        let action = "start a program";
        let request = Request::StartProgram {
            node: node.0,
            argv: program.argv.clone(),
        };
        let id = self.call(action, &request)?;
        id.map(ProgramHandle)
            .ok_or_else(|| Self::error(action, "the supervisor gave no handle"))
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
