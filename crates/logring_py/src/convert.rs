//! Error conversion: `logring_export`'s `anyhow`-based errors as a Python `RuntimeError`.

use pyo3::PyErr;
use pyo3::exceptions::PyRuntimeError;

pub fn to_pyerr(e: impl std::fmt::Display) -> PyErr {
    PyErr::new::<PyRuntimeError, _>(e.to_string())
}
