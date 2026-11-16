//! The `logring` extension module: [`logring_py`] as a Python wheel, built by maturin from the `pyproject.toml` next
//! to this crate. Everything the module holds is defined in `logring_py`; the `wire_weaver` wheel has the same API as
//! `wire_weaver.logring`.

use pyo3::prelude::*;

/// The API snapshots of every crate in the build's lock: a bundle's skipped standard types resolve with nothing on
/// disk and no network.
static SNAPSHOTS: ww_snapshots::Set = include!(concat!(env!("OUT_DIR"), "/snapshot_set.rs"));

#[pymodule]
fn logring(m: &Bound<'_, PyModule>) -> PyResult<()> {
    ww_snapshots::embed(&SNAPSHOTS);
    logring_py::init(m)
}
