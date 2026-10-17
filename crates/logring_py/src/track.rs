//! [PyTrack]: one decoded track, fully owned (everything is converted to plain Rust/JSON values right after
//! decode, in [PyTrack::from_track], so it doesn't carry the bundle's borrow into a `#[pyclass]`, which must be
//! `'static`).

use crate::convert::to_pyerr;
use numpy::PyArray1;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::collections::BTreeSet;

#[derive(Clone)]
struct Row {
    pos: u64,
    t_ps: Option<i64>,
    value: serde_json::Value,
    /// [logring_export::value_json::flatten_numeric] of `value`, kept alongside it so [PyTrack::to_numpy]
    /// doesn't need the original (bundle-borrowing) [ww_self::ValueOwned] it came from.
    numeric: Vec<(String, f64)>,
}

#[pyclass(name = "Track", skip_from_py_object)]
#[derive(Clone)]
pub struct PyTrack {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub path: Vec<u32>,
    /// Whether the bundle resolved this track's item type (an unresolved track has no rows).
    #[pyo3(get)]
    pub resolved: bool,
    #[pyo3(get)]
    pub decode_errors: u64,
    key: u64,
    rows: Vec<Row>,
}

impl PyTrack {
    pub fn from_track(t: &logring_export::Track) -> PyTrack {
        let rows = t
            .items
            .iter()
            .map(|item| {
                let mut numeric = Vec::new();
                logring_export::value_json::flatten_numeric("value", &item.value, &mut numeric);
                Row {
                    pos: item.head.pos,
                    t_ps: item
                        .head
                        .t
                        .as_ref()
                        .and_then(logring::reader::time_ps)
                        .and_then(|ps| i64::try_from(ps).ok()),
                    value: logring_export::value_json::value_to_json(&item.value),
                    numeric,
                }
            })
            .collect();
        PyTrack {
            name: t.name.clone(),
            path: t.info.path.clone(),
            resolved: t.ty.is_some(),
            decode_errors: t.decode_errors,
            key: t.info.key,
            rows,
        }
    }

    /// Column names across every row, in first-seen order.
    fn numeric_columns(&self) -> Vec<String> {
        let mut columns = Vec::new();
        let mut seen = BTreeSet::new();
        for row in &self.rows {
            for (name, _) in &row.numeric {
                if seen.insert(name.clone()) {
                    columns.push(name.clone());
                }
            }
        }
        columns
    }
}

#[pymethods]
impl PyTrack {
    /// The track's stable key (`logring info`'s `key.<16 hex digits>`).
    #[getter]
    fn key(&self) -> String {
        format!("{:016x}", self.key)
    }

    fn __len__(&self) -> usize {
        self.rows.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Track(name={:?}, key={}, {} rows, resolved={})",
            self.name,
            self.key(),
            self.rows.len(),
            self.resolved
        )
    }

    /// Every record as `{"pos", "t_ps", "value"}`, `value` the same shape `logring cat` prints.
    fn rows(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for row in &self.rows {
            let dict = PyDict::new(py);
            dict.set_item("pos", row.pos)?;
            dict.set_item("t_ps", row.t_ps)?;
            dict.set_item(
                "value",
                pythonize::pythonize(py, &row.value).map_err(to_pyerr)?,
            )?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// `pos`, `t_ps` and every numeric leaf field, each a `numpy.float64` array; `nan` where a record didn't
    /// have that field (an `Option` that was `None`, or a field a later firmware version added).
    fn to_numpy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let columns = self.numeric_columns();
        let dict = PyDict::new(py);
        let pos: Vec<f64> = self.rows.iter().map(|r| r.pos as f64).collect();
        dict.set_item("pos", PyArray1::from_vec(py, pos))?;
        let t_ps: Vec<f64> = self
            .rows
            .iter()
            .map(|r| r.t_ps.map(|v| v as f64).unwrap_or(f64::NAN))
            .collect();
        dict.set_item("t_ps", PyArray1::from_vec(py, t_ps))?;
        for name in &columns {
            let col: Vec<f64> = self
                .rows
                .iter()
                .map(|r| {
                    r.numeric
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, v)| *v)
                        .unwrap_or(f64::NAN)
                })
                .collect();
            dict.set_item(name, PyArray1::from_vec(py, col))?;
        }
        Ok(dict)
    }

    /// [to_numpy](Self::to_numpy) as a `pandas.DataFrame` (`pandas` is only imported here, not a hard
    /// dependency of the `logring` package itself).
    fn to_pandas<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let columns = self.to_numpy(py)?;
        let pandas = py.import("pandas")?;
        pandas.call_method1("DataFrame", (columns,))
    }
}
