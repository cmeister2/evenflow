//! Optional native bindings sharing the Rust runner and command implementation.

use std::ffi::OsString;
use std::path::PathBuf;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

/// Analyze files without rendering diagnostics or changing process state.
///
/// # Errors
///
/// Returns a Python runtime error if the report cannot be serialized.
#[pyfunction]
fn check_json(
    py: Python<'_>,
    working_directory: PathBuf,
    paths: Vec<PathBuf>,
    strict: bool,
) -> PyResult<String> {
    let report = py.detach(|| crate::runner::run(&working_directory, &paths, strict));
    serde_json::to_string(&report).map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

/// Run the same command implementation as the standalone Rust executable.
#[pyfunction]
fn execute(py: Python<'_>, arguments: Vec<OsString>) -> i32 {
    py.detach(|| crate::cli::execute(arguments))
}

/// Initialize the internal native module used by the public Python package.
///
/// # Errors
///
/// Returns a Python error if module registration fails.
#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add_function(wrap_pyfunction!(check_json, module)?)?;
    module.add_function(wrap_pyfunction!(execute, module)?)?;
    Ok(())
}
