//! The Python bindings (PYT-01): the native module `netscenario._native`.
//!
//! The Python package `netscenario` (in `python/netscenario`) wraps this module with typed, documented
//! Python. The module is built with maturin; the semantics live in the Rust crates and nowhere else (D-002).
//! It calls the core in the same process (D-031): a scenario is parsed and checked by the model, a run is
//! `netscenario_core::run` on a thread of its own, and the errors become the exceptions of the package.

mod errors;
mod run;

use netscenario_core::report::{self, Format};
use netscenario_core::{RunResult, RunSetResult};
use netscenario_model::Scenario;
use pyo3::prelude::*;

use crate::run::{PyCancelToken, ReplayRequest, Request, Series};

/// The version of the native library, the same as the version of the package.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Reads and checks a scenario (YAML or JSON text) and returns it as canonical JSON: the keys in the order of
/// the format, defaults left out. Raises `ScenarioError` with every problem if it is not valid.
///
/// The files that the scenario refers to (a table of delay samples) and the files it includes (D-093) are read from `base_dir`
/// (D-083); the canonical JSON holds what is made of them. Without a `base_dir` a scenario that refers to a file is refused,
/// because there is nowhere to look. A path that leaves `base_dir` is refused unless `allow_outside_files`. `params` are the
/// values of the parameters of the scenario, by name.
#[pyfunction]
#[pyo3(signature = (text, source_name, base_dir=None, allow_outside_files=false, params=None))]
fn parse_scenario(
    py: Python<'_>,
    text: &str,
    source_name: &str,
    base_dir: Option<&str>,
    allow_outside_files: bool,
    params: Option<Vec<(String, String)>>,
) -> PyResult<String> {
    let params = params.unwrap_or_default();
    let loaded = if let Some(base) = base_dir {
        netscenario_core::load_scenario(
            text,
            source_name,
            std::path::Path::new(base),
            allow_outside_files,
            &params,
        )
    } else {
        let mut options = netscenario_model::LoadOptions::new();
        for (name, value) in &params {
            options = options.param(name, value);
        }
        Scenario::from_yaml_with(text, source_name, &options)
    };
    let scenario = loaded.map_err(|error| errors::scenario(py, &error))?;
    let wanted = scenario.unresolved_files();
    if !wanted.is_empty() {
        let problems = wanted
            .iter()
            .map(|file| {
                netscenario_model::Problem::new(format!(
                    "the scenario refers to the file '{file}', and there is no directory to look for it in"
                ))
                .with_help("read the scenario with `Scenario.load(path)`, or give `base_dir`")
            })
            .collect();
        if let Some(error) = scenario.problems_in_source(source_name, text, problems) {
            return Err(errors::scenario(py, &error));
        }
    }
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

/// Runs a scenario and returns the text of `result.json`; with `repetitions`, runs it that many times and returns the text of
/// `runset.json` (ASR-02, ASR-03).
///
/// `source` is the scenario as it was written (the lines of errors refer to it) and `canonical` the same scenario in the
/// canonical form with the files it refers to read (D-083), which is what runs; `supervisor` is the
/// `netscenario` program and `agent` the `netscenario-agent` program. `observer` is called as `observer(kind,
/// fields)` for each event of the run.
#[pyfunction]
#[pyo3(signature = (source, canonical, source_name, supervisor, agent, out_dir, seed=None, settle=None, observer=None, cancel=None, bin=None, recovery_basis=None, recovery_baseline=None, recovery_window=None, recovery_fraction=None, working_dir=None, capture=None, packets=None, repetitions=None, statistic=None, confidence=None))]
#[allow(clippy::too_many_arguments)] // the arguments of one call from Python, all of them by name
fn run_scenario(
    py: Python<'_>,
    source: String,
    canonical: String,
    source_name: String,
    supervisor: String,
    agent: String,
    out_dir: String,
    seed: Option<u64>,
    settle: Option<f64>,
    observer: Option<Py<PyAny>>,
    cancel: Option<PyRef<'_, PyCancelToken>>,
    bin: Option<f64>,
    recovery_basis: Option<String>,
    recovery_baseline: Option<u32>,
    recovery_window: Option<u32>,
    recovery_fraction: Option<f64>,
    working_dir: Option<String>,
    capture: Option<Vec<String>>,
    packets: Option<bool>,
    repetitions: Option<u32>,
    statistic: Option<&str>,
    confidence: Option<f64>,
) -> PyResult<String> {
    let set = run::set_options(repetitions, statistic, confidence)?;
    run::execute(
        py,
        Request {
            source,
            canonical,
            source_name,
            supervisor,
            agent,
            out_dir,
            seed,
            settle,
            observer,
            cancel: cancel.map(|token| token.token()),
            working_dir,
            series: Series {
                bin,
                recovery_basis,
                recovery_baseline,
                recovery_window,
                recovery_fraction,
                capture,
                packets,
            },
            set,
        },
    )
}

/// Runs a recorded run again with its recorded seed and returns the comparison with the original as the JSON of
/// `netscenario replay --json` (ENV-03, D-075, D-077). `dir` is the directory of the run; a directory that cannot be replayed is
/// a `ValueError`.
#[pyfunction]
#[pyo3(signature = (dir, supervisor, agent, out_dir, observer=None, cancel=None, working_dir=None))]
#[allow(clippy::too_many_arguments)] // the arguments of one call from Python, all of them by name
fn replay_run(
    py: Python<'_>,
    dir: String,
    supervisor: String,
    agent: String,
    out_dir: String,
    observer: Option<Py<PyAny>>,
    cancel: Option<PyRef<'_, PyCancelToken>>,
    working_dir: Option<String>,
) -> PyResult<String> {
    run::execute_replay(
        py,
        ReplayRequest {
            dir,
            supervisor,
            agent,
            out_dir,
            observer,
            cancel: cancel.map(|token| token.token()),
            working_dir,
        },
    )
}

/// The report of a result directory in a format (`md`, `junit` or `html`), as text (CLI-02, D-076). A directory that is not a
/// result, or a file that cannot be read, is a `ValueError`.
#[pyfunction]
fn make_report(py: Python<'_>, dir: &str, format: &str) -> PyResult<String> {
    let invalid = |text: String| pyo3::exceptions::PyValueError::new_err(text);
    let format: Format = format.parse().map_err(invalid)?;
    // Reading the files and drawing take a moment for a long run, and need no Python
    py.detach(|| report::report(std::path::Path::new(dir), format))
        .map_err(|error| invalid(error.to_string()))
}

/// The properties of a result for a `JUnit` file, as `(name, value)` pairs: the same list as the report (D-077). `text` is the
/// text of a `result.json`.
#[pyfunction]
fn result_properties(text: &str) -> PyResult<Vec<(String, String)>> {
    let result = RunResult::from_json(text).map_err(pyo3::exceptions::PyValueError::new_err)?;
    Ok(report::run_properties(&result))
}

/// The properties of a set of runs for a `JUnit` file: the same list as the report (D-077). `text` is the text of a
/// `runset.json`.
#[pyfunction]
fn runset_properties(text: &str) -> PyResult<Vec<(String, String)>> {
    let set = RunSetResult::from_json(text).map_err(pyo3::exceptions::PyValueError::new_err)?;
    Ok(report::set_properties(&set))
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(version, module)?)?;
    module.add_function(wrap_pyfunction!(parse_scenario, module)?)?;
    module.add_function(wrap_pyfunction!(needs, module)?)?;
    module.add_function(wrap_pyfunction!(schema, module)?)?;
    module.add_function(wrap_pyfunction!(doctor, module)?)?;
    module.add_function(wrap_pyfunction!(run_scenario, module)?)?;
    module.add_function(wrap_pyfunction!(replay_run, module)?)?;
    module.add_function(wrap_pyfunction!(make_report, module)?)?;
    module.add_function(wrap_pyfunction!(result_properties, module)?)?;
    module.add_function(wrap_pyfunction!(runset_properties, module)?)?;
    module.add_class::<PyCancelToken>()?;
    Ok(())
}
