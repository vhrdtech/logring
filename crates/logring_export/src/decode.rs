//! Grouping a [LogFile](crate::open::LogFile)'s raw records by track, resolving each track's item type from its
//! session's bundle, and decoding its records to [ValueOwned] (dynamic decode, `ww_self::value`): the shared
//! step behind CSV, MCAP and MF4 export.

use crate::open::LogFile;
use crate::schema::{self, Bundle};
use logring::scan::TrackInfo;
use logring::system::SystemEvent;
use std::collections::BTreeMap;
use ww_self::{ApiBundleOwned, TypeOwned, ValueOwned};
use ww_stream::StreamHead;

/// A human-readable name for a track without a resolved type, so export and `logring info` always have
/// something to show: `key.<stable key, hex>`.
pub fn fallback_name(key: u64) -> String {
    format!("key.{key:016x}")
}

/// One decoded record of a track: its position in the stream, device time (when the device had the clock set)
/// and value.
pub struct Item {
    pub head: StreamHead,
    pub value: ValueOwned,
}

/// A track ready for export: its description, resolved type (`None` when no session's bundle describes it),
/// and every record that decoded cleanly. Malformed or unresolvable records are only counted
/// ([Track::decode_errors]), never fatal: a card is untrusted input.
pub struct Track<'b> {
    pub info: TrackInfo,
    pub ty: Option<&'b TypeOwned>,
    /// The bundle `ty` was resolved from (`des_shrink_wrap_dyn` needs it again for any `OutOfLine`/`Instance`
    /// type it refers to); `None` exactly when `ty` is `None`.
    pub bundle: Option<&'b ApiBundleOwned>,
    /// The dotted resource name ([schema::Resolved::name]), or [fallback_name] when `ty` is `None`.
    pub name: String,
    pub items: Vec<Item>,
    /// Records present on the medium whose item didn't decode against `ty` (or whose type couldn't be
    /// resolved at all): a torn write, a stale bundle, or a path this tool's resolver doesn't support yet.
    pub decode_errors: u64,
}

/// Every user track (number > 0) of `log`, decoded against the session's bundle; `bundles` are `log.bundles`
/// already parsed with [schema::parse]. System track events (markers, session starts, recordings, drops) are
/// not included: see [system_events].
pub fn tracks<'b>(log: &LogFile, bundles: &'b BTreeMap<u64, Bundle>) -> Vec<Track<'b>> {
    let mut by_key: BTreeMap<u64, Track<'b>> = BTreeMap::new();
    for info in log.tracks.values() {
        // the first session bundle that resolves this track's path wins; a path is stable across sessions
        // that keep it (LOG-1), so any session logged under the current API will do
        let mut found = None;
        for session in &log.sessions {
            let Some(bundle) = bundles.get(&session.bundle_hash) else {
                continue;
            };
            if let Ok(r) = schema::resolve_track_type(&bundle.api, &info.path) {
                found = Some((r, &bundle.api));
                break;
            }
        }
        let (ty, bundle, name) = match found {
            Some((r, bundle)) => (Some(r.ty), Some(bundle), r.name),
            None => (None, None, fallback_name(info.key)),
        };
        by_key.insert(
            info.key,
            Track {
                info: info.clone(),
                ty,
                bundle,
                name,
                items: Vec::new(),
                decode_errors: 0,
            },
        );
    }
    for (seg, rec) in log.scan.records() {
        if rec.track == 0 {
            continue;
        }
        let Some(key) = seg
            .tracks
            .iter()
            .find(|t| t.number == rec.track)
            .map(|t| t.key)
        else {
            continue;
        };
        let Some(track) = by_key.get_mut(&key) else {
            continue;
        };
        match (track.ty, track.bundle) {
            (Some(ty), Some(bundle)) => {
                match ValueOwned::des_shrink_wrap_dyn(&rec.item, ty, bundle) {
                    Ok(value) => track.items.push(Item {
                        head: rec.head,
                        value,
                    }),
                    Err(_) => track.decode_errors += 1,
                }
            }
            _ => track.decode_errors += 1,
        }
    }
    by_key.into_values().collect()
}

/// Every system track (track 0) event across the ring, in file order; malformed ones are skipped.
pub fn system_events(log: &LogFile) -> Vec<SystemEvent<'_>> {
    let mut out = Vec::new();
    for (_, rec) in log.scan.records() {
        if rec.track != 0 {
            continue;
        }
        let mut dec = logring::format::Dec::new(&rec.item);
        if let Some(ev) = SystemEvent::decode(&mut dec) {
            out.push(ev);
        }
    }
    out
}
