//! Python module for `logring` (`logring_export` underneath), built with PyO3 + maturin.
//!
//! Scaffold only (P2628#16oct-1016, R0): `open()`, tracks, recordings, `to_pandas()` land in R10
//! (P2628#16oct-1001h). See `AGENTS.md` and `FEATURES.md`.

use pyo3::prelude::*;

#[pymodule]
fn logring(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
