//! The Python bindings (PYT-01): the native module `netscenario._native`.
//!
//! The Python package `netscenario` (in `python/netscenario`) wraps this module with typed, documented
//! Python. The module is built with maturin; the semantics live in the Rust crates and nowhere else (D-002).

use pyo3::prelude::*;

/// The version of the native library, the same as the version of the package.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(version, module)?)?;
    Ok(())
}
