//! PC-side reading and exporting of `logring` cards, images and split files.
//!
//! Opens a mounted card, a ring file copy, a raw image or a split `.wlr` file through `logring` (`std`
//! feature, [open]), decodes tracks dynamically from the stored API bundle ([schema], [decode]; `ww_self` and
//! `wire_weaver_snapshots`, no generated code), and exports CSV ([csv_export]), MCAP ([mcap_export]) and MF4
//! ([mf4_export]). See `AGENTS.md` and `FEATURES.md` (PC-1, P2628#16oct-1001h).

pub mod csv_export;
pub mod decode;
pub mod events;
pub mod mcap_export;
pub mod meta;
pub mod mf4_export;
pub mod open;
pub mod schema;
pub mod value_json;

#[cfg(test)]
mod test_support;

pub use decode::{Item, Track};
pub use events::{Marker, Recording};
pub use meta::{BundleRecord, SessionRecord};
pub use open::{LogFile, OpenError, open};
pub use schema::Bundle;

use std::collections::BTreeMap;

/// Parses every bundle [LogFile::bundles] found, skipping ones that don't decode (never fatal: a card is
/// untrusted input). The result feeds [decode::tracks].
pub fn parse_bundles(log: &LogFile) -> BTreeMap<u64, Bundle> {
    log.bundles
        .iter()
        .filter_map(|(hash, rec)| schema::parse(&rec.bytes).ok().map(|b| (*hash, b)))
        .collect()
}
