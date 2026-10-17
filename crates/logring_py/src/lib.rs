//! Python module for `logring` (`logring_export` underneath), built with PyO3 + maturin: `open()`, tracks,
//! recordings, markers, `Track.rows()`/`to_numpy()`/`to_pandas()`. See `AGENTS.md` and `FEATURES.md` (PC-1,
//! P2628#16oct-1001h).

mod convert;
mod track;

use convert::to_pyerr;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use track::PyTrack;

#[pyclass(name = "LogFile")]
pub struct PyLogFile {
    tracks: Vec<PyTrack>,
    sessions: Vec<logring_export::SessionRecord>,
    markers: Vec<logring_export::Marker>,
    recordings: Vec<logring_export::Recording>,
}

#[pymethods]
impl PyLogFile {
    /// Every decoded track; one without a resolved type still appears (`Track.resolved` is `False`, no rows).
    fn tracks(&self) -> Vec<PyTrack> {
        self.tracks.clone()
    }

    /// The track with this dotted name (`logring info` lists them), `KeyError` if there is none.
    fn track(&self, name: &str) -> PyResult<PyTrack> {
        self.tracks
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyKeyError, _>(name.to_string()))
    }

    /// One dict per session: `session`, `device_id`, `fw`, `hw`, `bundle_hash` (hex string).
    fn sessions(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for s in &self.sessions {
            let dict = PyDict::new(py);
            dict.set_item("session", s.session)?;
            dict.set_item("device_id", &s.device_id)?;
            dict.set_item("fw", &s.fw)?;
            dict.set_item("hw", &s.hw)?;
            dict.set_item("bundle_hash", format!("{:016x}", s.bundle_hash))?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// One dict per marker: `pos`, `t_ps`, `kind`, `label`, `value`.
    fn markers(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for m in &self.markers {
            let dict = PyDict::new(py);
            dict.set_item("pos", m.pos)?;
            dict.set_item("t_ps", m.t_ps.and_then(|ps| i64::try_from(ps).ok()))?;
            dict.set_item("kind", m.kind)?;
            dict.set_item("label", &m.label)?;
            dict.set_item("value", m.value)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// One dict per recording: `id`, `trigger`, `start_pos`, `start_t_ps`, `stop_pos`, `stop_t_ps`,
    /// `stop_reason` (`None` while still running, or when its stop was lost to a torn write).
    fn recordings(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for r in &self.recordings {
            let dict = PyDict::new(py);
            dict.set_item("id", r.id)?;
            dict.set_item("trigger", r.trigger)?;
            dict.set_item("start_pos", r.start_pos)?;
            dict.set_item(
                "start_t_ps",
                r.start_t_ps.and_then(|ps| i64::try_from(ps).ok()),
            )?;
            dict.set_item("stop_pos", r.stop_pos)?;
            dict.set_item(
                "stop_t_ps",
                r.stop_t_ps.and_then(|ps| i64::try_from(ps).ok()),
            )?;
            dict.set_item("stop_reason", r.stop_reason)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }
}

/// Opens `path`: a ring file, raw image, split `.wlr`, or a directory holding `logring.wlr` (a mounted card's
/// volume, no root).
#[pyfunction]
fn open(path: &str) -> PyResult<PyLogFile> {
    let log = logring_export::open(path).map_err(to_pyerr)?;
    let bundles = logring_export::parse_bundles(&log);
    let tracks = logring_export::decode::tracks(&log, &bundles)
        .iter()
        .map(PyTrack::from_track)
        .collect();
    let markers = logring_export::events::markers(&log);
    let recordings = logring_export::events::recordings(&log);
    Ok(PyLogFile {
        tracks,
        sessions: log.sessions.clone(),
        markers,
        recordings,
    })
}

#[pymodule]
fn logring(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_class::<PyLogFile>()?;
    m.add_class::<PyTrack>()?;
    Ok(())
}
