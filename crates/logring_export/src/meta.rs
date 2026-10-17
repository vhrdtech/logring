//! Sessions and API bundles from the meta area's records, decoded once when a [LogFile](crate::open::LogFile)
//! is opened.

use logring::format::Dec;
use logring::scan::Scan;
use logring::system::MetaEvent;
use ww_version::FullVersionOwned;

/// One session's meta record: who was logging, with which API and bundle.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session: u32,
    pub api_root: u64,
    pub api: FullVersionOwned,
    pub device_id: String,
    pub fw: String,
    pub hw: String,
    pub bundle_hash: u64,
    /// Tracks declared (numbers, not necessarily present if a device logged only some of them).
    pub tracks: u32,
}

/// The raw bytes of one API bundle, keyed by its hash in [SessionRecord::bundle_hash].
#[derive(Debug, Clone)]
pub struct BundleRecord {
    pub hash: u64,
    pub bytes: Vec<u8>,
}

/// Walks every meta segment's records (already decoded by [Scan::open](logring::scan::Scan::open)) and splits
/// them into sessions and bundles; malformed records are skipped (untrusted input, never panics).
pub fn collect(
    scan: &Scan,
) -> (
    Vec<SessionRecord>,
    std::collections::BTreeMap<u64, BundleRecord>,
) {
    let mut sessions = Vec::new();
    let mut bundles = std::collections::BTreeMap::new();
    for seg in &scan.meta {
        for rec in &seg.records {
            let mut dec = Dec::new(&rec.item);
            match MetaEvent::decode(&mut dec) {
                Some(MetaEvent::Session {
                    session,
                    api_root,
                    api,
                    device_id,
                    fw,
                    hw,
                    bundle_hash,
                    tracks,
                }) => {
                    sessions.push(SessionRecord {
                        session,
                        api_root,
                        api: api.into(),
                        device_id: device_id.to_string(),
                        fw: fw.to_string(),
                        hw: hw.to_string(),
                        bundle_hash,
                        tracks,
                    });
                }
                Some(MetaEvent::Bundle { hash, len }) => {
                    let bytes = dec.rest();
                    if bytes.len() == len as usize {
                        bundles.entry(hash).or_insert_with(|| BundleRecord {
                            hash,
                            bytes: bytes.to_vec(),
                        });
                    }
                }
                Some(MetaEvent::Recording { .. }) | None => {}
            }
        }
    }
    (sessions, bundles)
}
