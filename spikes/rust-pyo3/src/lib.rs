//! The native part of the spike: a function that parses a duration with a unit (as the scenario model will),
//! one that releases the interpreter lock while it works (a run takes seconds), and an error type.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Parses "20ms", "1.5s", "250us" into nanoseconds.
#[pyfunction]
fn parse_duration_ns(text: &str) -> PyResult<u64> {
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| PyValueError::new_err(format!("duration '{text}' has no unit")))?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| PyValueError::new_err(format!("duration '{text}': '{number}' is not a number")))?;
    let factor = match unit {
        "ns" => 1.0,
        "us" => 1e3,
        "ms" => 1e6,
        "s" => 1e9,
        other => return Err(PyValueError::new_err(format!("duration '{text}': unknown unit '{other}'"))),
    };
    Ok((value * factor).round() as u64)
}

/// Sleeps without holding the interpreter lock, so other Python threads (a progress display) keep running.
#[pyfunction]
fn busy_wait_ms(py: Python<'_>, ms: u64) -> u64 {
    py.detach(|| {
        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(ms));
        start.elapsed().as_millis() as u64
    })
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_duration_ns, m)?)?;
    m.add_function(wrap_pyfunction!(busy_wait_ms, m)?)?;
    m.add("__rust_version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
