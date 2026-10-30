//! The Python bindings (PYT-01): the native module `netscenario._native`.
//!
//! The Python package `netscenario` (in `python/netscenario`) wraps this module with typed, documented
//! Python. The module is built with maturin; the semantics live in the Rust crates and nowhere else (D-002).
//! It calls the core in the same process (D-031): a scenario is parsed and checked by the model, a run is
//! `netscenario_core::run` on a thread of its own, and the errors become the exceptions of the package.

mod errors;
mod run;

use netscenario_model::Scenario;
use pyo3::prelude::*;

use crate::run::{PyCancelToken, Request};

/// The version of the native library, the same as the version of the package.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Reads and checks a scenario (YAML or JSON text) and returns it as canonical JSON: the keys in the order of
/// the format, defaults left out. Raises `ScenarioError` with every problem if it is not valid.
#[pyfunction]
fn parse_scenario(py: Python<'_>, text: &str, source_name: &str) -> PyResult<String> {
    let scenario =
        Scenario::from_yaml(text, source_name).map_err(|error| errors::scenario(py, &error))?;
    serde_json::to_string(&scenario).map_err(|error| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("cannot write the scenario: {error}"))
    })
}

/// The ids of the features that a scenario needs from a backend, in the order of their first use.
#[pyfunction]
fn needs(py: Python<'_>, text: &str, source_name: &str) -> PyResult<Vec<String>> {
    let scenario =
        Scenario::from_yaml(text, source_name).map_err(|error| errors::scenario(py, &error))?;
    Ok(netscenario_core::requirements(&scenario)
        .iter()
        .map(|requirement| requirement.feature.id().to_owned())
        .collect())
}

/// The JSON Schema of the scenario format.
#[pyfunction]
fn schema() -> String {
    netscenario_model::json_schema()
}

/// What this machine can do for the Kernel backend, as the JSON of `netscenario doctor --json`. The probe
/// starts processes and takes a moment, so the GIL is given up while it runs.
#[pyfunction]
fn doctor(py: Python<'_>) -> String {
    py.detach(|| netscenario_backend_kernel::doctor::diagnose().to_json())
}

/// Runs a scenario and returns the text of `result.json`.
///
/// `source` is the scenario as it was written (the lines of errors refer to it); `supervisor` is the
/// `netscenario` program and `agent` the `netscenario-agent` program. `observer` is called as `observer(kind,
/// fields)` for each event of the run.
#[pyfunction]
#[pyo3(signature = (source, source_name, supervisor, agent, out_dir, seed=None, settle=None, packets=false, observer=None, cancel=None))]
#[allow(clippy::too_many_arguments)] // the arguments of one call from Python, all of them by name
fn run_scenario(
    py: Python<'_>,
    source: String,
    source_name: String,
    supervisor: String,
    agent: String,
    out_dir: String,
    seed: Option<u64>,
    settle: Option<f64>,
    packets: bool,
    observer: Option<Py<PyAny>>,
    cancel: Option<PyRef<'_, PyCancelToken>>,
) -> PyResult<String> {
    run::execute(
        py,
        Request {
            source,
            source_name,
            supervisor,
            agent,
            out_dir,
            seed,
            settle,
            packets,
            observer,
            cancel: cancel.map(|token| token.token()),
        },
    )
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(version, module)?)?;
    module.add_function(wrap_pyfunction!(parse_scenario, module)?)?;
    module.add_function(wrap_pyfunction!(needs, module)?)?;
    module.add_function(wrap_pyfunction!(schema, module)?)?;
    module.add_function(wrap_pyfunction!(doctor, module)?)?;
    module.add_function(wrap_pyfunction!(run_scenario, module)?)?;
    module.add_class::<PyCancelToken>()?;
    Ok(())
}
