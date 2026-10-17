//! Opening a logring source: a ring file, a raw image, a split `.wlr` (all the same on-medium bytes, read
//! through [ImageFile](logring::image::ImageFile)), or a directory the OS already mounted the card's volume
//! under (the ring file `logring.wlr` found inside it) — no raw device access, no root.

use crate::meta::{self, BundleRecord, SessionRecord};
use logring::image::ImageFile;
use logring::scan::{Scan, ScanError, TrackInfo};
use logring::tests_support::block_on;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The ring file name on an exFAT card ([logring's view](https://github.com/vhrdtech/logring), LOG-3).
pub const RING_NAME: &str = "logring.wlr";

/// Errors opening a logring source.
#[derive(Debug, Error)]
pub enum OpenError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no {RING_NAME} in {0}")]
    NoRingFile(PathBuf),
    #[error("scanning the ring: {0}")]
    Scan(#[from] ScanError<io::Error>),
}

/// A ring, image or split file, scanned once into memory: everything a PC tool needs (sessions, bundles, track
/// descriptions, decoded system and user records) without going back to the medium.
pub struct LogFile {
    pub scan: Scan,
    pub sessions: Vec<SessionRecord>,
    /// The API bundle(s) found in the meta area, by hash (a session only names the hash it was logged with).
    pub bundles: BTreeMap<u64, BundleRecord>,
    /// Every data track seen in any segment, by its stable key, with the fullest [TrackInfo] found for it (the
    /// one with the most of a session's track table attached, in case an older segment's entry is partial).
    pub tracks: BTreeMap<u64, TrackInfo>,
}

/// Opens `path`: a ring/image/split file directly, or a directory (a mounted card's volume, or any folder) that
/// holds `logring.wlr`.
pub fn open(path: impl AsRef<Path>) -> Result<LogFile, OpenError> {
    let path = path.as_ref();
    let ring_path = resolve_ring_path(path)?;
    let mut medium = ImageFile::open(&ring_path).map_err(|source| OpenError::Io {
        path: ring_path.clone(),
        source,
    })?;
    let scan = block_on(Scan::open(&mut medium, None))?;
    let (sessions, bundles) = meta::collect(&scan);
    let tracks = collect_tracks(&scan);
    Ok(LogFile {
        scan,
        sessions,
        bundles,
        tracks,
    })
}

/// `path` itself when it is a file, `path/logring.wlr` when it is a directory (a mounted card or any folder
/// holding a copy of the ring file).
fn resolve_ring_path(path: &Path) -> Result<PathBuf, OpenError> {
    let is_dir = std::fs::metadata(path)
        .map_err(|source| OpenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .is_dir();
    if !is_dir {
        return Ok(path.to_path_buf());
    }
    let candidate = path.join(RING_NAME);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(OpenError::NoRingFile(path.to_path_buf()))
    }
}

/// The union of every data segment's track table, by stable key: later segments (and sessions) only add to or
/// confirm what earlier ones said, since a track's path doesn't change once logged (LOG-1 differences: track
/// identity is numbers only, stable across sessions and firmware updates that keep the path).
fn collect_tracks(scan: &Scan) -> BTreeMap<u64, TrackInfo> {
    let mut tracks = BTreeMap::new();
    for seg in &scan.data {
        for t in &seg.tracks {
            tracks.entry(t.key).or_insert_with(|| t.clone());
        }
    }
    tracks
}
