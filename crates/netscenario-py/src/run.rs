//! A run for Python (D-031): the core runs on a thread of its own, the calling thread gives up the GIL and
//! looks for interrupts every 100 milliseconds, and an observer can be a Python callable.
//!
//! Why a thread: `netscenario_core::run` blocks for the length of the scenario. A Python program that is
//! blocked in a native call does not see Ctrl-C, and `pytest-timeout` cannot stop it. So the calling thread
//! waits in slices, calls `check_signals` between them, and cancels the run when an interrupt arrives. The run
//! tears the network down and returns, and then the interrupt is raised.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use netscenario_backend_kernel::{KernelBackend, KernelOptions};
use netscenario_core::{CancelToken, Error, RunEvent, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::errors;

/// How long the calling thread waits before it looks for an interrupt.
const SLICE: Duration = Duration::from_millis(100);

/// A token that cancels a run. It can be handed to a run and cancelled from any thread.
#[pyclass(name = "CancelToken", module = "netscenario._native", frozen)]
#[derive(Clone, Default)]
pub(crate) struct PyCancelToken {
    inner: CancelToken,
}

#[pymethods]
impl PyCancelToken {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    /// Asks the run that holds this token to stop. It may be called from any thread, and more than once.
    fn cancel(&self) {
        self.inner.cancel();
    }

    /// Whether `cancel` has been called.
    #[getter]
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl PyCancelToken {
    /// The token of the core, which shares the request with this one.
    pub(crate) fn token(&self) -> CancelToken {
        self.inner.clone()
    }
}

/// A slot that the run puts its outcome into and the calling thread waits on.
type Outcome = Arc<(Mutex<Option<Finished>>, Condvar)>;

/// How the thread of the run ended.
enum Finished {
    /// It returned this.
    Returned(Box<Result<RunResult, Error>>),
    /// It panicked: the text of the panic.
    Panicked(String),
}

/// The event as the name of its kind and its numbers (seconds as floats), for the Python side to make an
/// object of.
fn event_to_python<'py>(
    py: Python<'py>,
    event: &RunEvent,
) -> PyResult<(&'static str, Bound<'py, PyDict>)> {
    let fields = PyDict::new(py);
    let kind = match event {
        RunEvent::Provisioned { nodes, links } => {
            fields.set_item("nodes", nodes)?;
            fields.set_item("links", links)?;
            "provisioned"
        }
        RunEvent::ReceiversReady { count } => {
            fields.set_item("count", count)?;
            "receivers_ready"
        }
        RunEvent::Started { duration } => {
            fields.set_item("duration", duration.as_secs_f64())?;
            "started"
        }
        RunEvent::Progress { elapsed, total } => {
            fields.set_item("elapsed", elapsed.as_secs_f64())?;
            fields.set_item("total", total.as_secs_f64())?;
            "progress"
        }
        RunEvent::Settling { wait } => {
            fields.set_item("wait", wait.as_secs_f64())?;
            "settling"
        }
        RunEvent::Collecting => "collecting",
        RunEvent::TearingDown => "tearing_down",
        RunEvent::Finished => "finished",
        // An event of a later version of the core that this module does not know
        _ => "unknown",
    };
    Ok((kind, fields))
}

/// What the Python call asks for.
pub(crate) struct Request {
    pub(crate) source: String,
    pub(crate) source_name: String,
    pub(crate) supervisor: String,
    pub(crate) agent: String,
    pub(crate) out_dir: String,
    pub(crate) seed: Option<u64>,
    pub(crate) settle: Option<f64>,
    pub(crate) packets: bool,
    pub(crate) observer: Option<Py<PyAny>>,
    pub(crate) cancel: Option<CancelToken>,
}

/// Starts the run on a thread of its own. The outcome appears in the slot that is returned.
fn start_worker(scenario: Scenario, supervisor: String, options: RunOptions) -> Outcome {
    let outcome: Outcome = Arc::new((Mutex::new(None), Condvar::new()));
    let worker_outcome = Arc::clone(&outcome);
    std::thread::spawn(move || {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut backend =
                KernelBackend::new(KernelOptions::for_scenario(&supervisor, &scenario))?;
            run(&mut backend, &scenario, &options)
        }));
        let finished = match ran {
            Ok(result) => Finished::Returned(Box::new(result)),
            Err(panic) => Finished::Panicked(
                panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "the run panicked".to_owned()),
            ),
        };
        let (slot, ready) = &*worker_outcome;
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(finished);
        ready.notify_all();
    });
    outcome
}

/// Waits for the run in slices of 100 ms with the GIL given up. An interrupt cancels the run, which is still
/// waited for; the interrupt is returned next to the outcome.
fn wait_for(py: Python<'_>, outcome: &Outcome, token: &CancelToken) -> (Finished, Option<PyErr>) {
    let mut interrupt: Option<PyErr> = None;
    loop {
        let (slot, ready) = &**outcome;
        let done = py.detach(|| {
            let guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
            let (mut guard, _) = ready
                .wait_timeout_while(guard, SLICE, |finished| finished.is_none())
                .unwrap_or_else(PoisonError::into_inner);
            guard.take()
        });
        if let Some(finished) = done {
            return (finished, interrupt);
        }
        if interrupt.is_none()
            && let Err(error) = py.check_signals()
        {
            token.cancel();
            interrupt = Some(error);
        }
    }
}

/// Runs the scenario and returns the text of `result.json`.
pub(crate) fn execute(py: Python<'_>, request: Request) -> PyResult<String> {
    let Request {
        source,
        source_name,
        supervisor,
        agent,
        out_dir,
        seed,
        settle,
        packets,
        observer,
        cancel,
    } = request;
    let mut scenario =
        Scenario::from_yaml(&source, &source_name).map_err(|error| errors::scenario(py, &error))?;
    if seed.is_some() {
        scenario.seed = seed;
    }
    let token = cancel.unwrap_or_default();

    // What goes wrong in the observer: the first error is kept, the run is cancelled, and it is raised after
    let callback_error: Arc<Mutex<Option<PyErr>>> = Arc::new(Mutex::new(None));
    let mut options = RunOptions::new(&agent, &out_dir).with_cancel(token.clone());
    options.export_packets = packets;
    options.settle = settle.map(Duration::from_secs_f64);
    if let Some(callable) = observer {
        let (error_slot, cancel_on_error) = (Arc::clone(&callback_error), token.clone());
        let notify = move |event: &RunEvent| {
            Python::attach(|py| {
                let called = event_to_python(py, event)
                    .and_then(|(kind, fields)| callable.call1(py, (kind, fields)));
                if let Err(error) = called {
                    let mut slot = error_slot.lock().unwrap_or_else(PoisonError::into_inner);
                    if slot.is_none() {
                        *slot = Some(error);
                    }
                    cancel_on_error.cancel();
                }
            });
        };
        options = options.with_observer(Arc::new(notify));
    }

    let outcome = start_worker(scenario, supervisor, options);
    let (finished, interrupt) = wait_for(py, &outcome, &token);

    if let Some(error) = interrupt {
        return Err(error);
    }
    if let Some(error) = callback_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        return Err(error);
    }
    match finished {
        Finished::Returned(returned) => match *returned {
            Ok(result) => Ok(result.to_json()),
            Err(error) => Err(errors::core(py, &error, &source_name, &source)),
        },
        Finished::Panicked(text) => Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
            "internal error in the native library: {text}"
        ))),
    }
}
