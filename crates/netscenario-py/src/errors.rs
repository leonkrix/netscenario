//! The errors of the core as the exceptions of the Python package.
//!
//! The exception classes are defined in Python (`netscenario/errors.py`), where they are typed and documented.
//! The native code does not build them itself: it hands a kind, a message and the details as plain tuples to
//! the function `netscenario.errors.from_native`, which makes the instance. That keeps the classes in one
//! place and the native module free of knowledge about them.

use netscenario_core::{CapabilityError, Error};
use pyo3::prelude::*;
use pyo3::types::PyList;

/// Makes the exception of `kind` from a message and details, by asking the Python package.
fn exception<'py>(py: Python<'py>, kind: &str, message: &str, details: Bound<'py, PyAny>) -> PyErr {
    let made = py
        .import("netscenario.errors")
        .and_then(|module| module.getattr("from_native"))
        .and_then(|function| function.call1((kind, message, details)));
    match made {
        Ok(instance) => PyErr::from_value(instance),
        // The package could not be imported: that error is more useful than ours
        Err(error) => error,
    }
}

/// A problem as the tuple `(message, path, line, column, help)` that the Python side takes apart.
type ProblemTuple = (
    String,
    Option<String>,
    Option<usize>,
    Option<usize>,
    Option<String>,
);

/// A scenario that is not valid: every problem with its place.
pub(crate) fn scenario(py: Python<'_>, error: &netscenario_model::Error) -> PyErr {
    let problems: Vec<ProblemTuple> = error
        .problems()
        .iter()
        .map(|problem| {
            (
                problem.message.clone(),
                problem.path.clone(),
                problem.location.map(|l| l.line),
                problem.location.map(|l| l.column),
                problem.help.clone(),
            )
        })
        .collect();
    match PyList::new(py, problems) {
        Ok(details) => exception(py, "scenario", &error.render(), details.into_any()),
        Err(error) => error,
    }
}

/// A scenario that the backend cannot reproduce exactly: every refusal, rendered with the lines of the text the
/// scenario was read from.
fn capability(py: Python<'_>, error: &CapabilityError, source_name: &str, source: &str) -> PyErr {
    let rendered = error.render(source_name, source);
    let message = if rendered.trim().is_empty() {
        error.to_string()
    } else {
        rendered
    };
    let refusals: Vec<(String, String, String, String)> = error
        .refusals()
        .iter()
        .map(|refusal| {
            (
                refusal.feature.id().to_owned(),
                refusal.path.clone(),
                refusal.reason.clone(),
                refusal.fix.clone(),
            )
        })
        .collect();
    match PyList::new(py, refusals) {
        Ok(list) => match (error.backend(), list).into_pyobject(py) {
            Ok(details) => exception(py, "capability", &message, details.into_any()),
            Err(error) => error,
        },
        Err(error) => error,
    }
}

/// An error of the core. `source_name` and `source` are the scenario as the person wrote it, for the lines.
pub(crate) fn core(py: Python<'_>, error: &Error, source_name: &str, source: &str) -> PyErr {
    let none = || py.None().into_bound(py);
    match error {
        Error::InvalidScenario(_) => exception(
            py,
            "scenario",
            &error.to_string(),
            PyList::empty(py).into_any(),
        ),
        Error::Capability(capability_error) => {
            capability(py, capability_error, source_name, source)
        }
        Error::Backend { .. } => exception(py, "backend", &error.to_string(), none()),
        Error::Cancelled => exception(py, "cancelled", &error.to_string(), none()),
        // `Run`, and whatever the core adds later: the run failed
        _ => exception(py, "run", &error.to_string(), none()),
    }
}
