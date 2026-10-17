//! The mount-on-garbage harness (`std`): what the fuzz target `fuzz/fuzz_targets/mount.rs` and the corruption
//! test run. Takes any bytes as a ring image, mounts, logs, remounts and scans; nothing may panic.

use crate::format::Role;
use crate::reader::{AllTracks, Reader, TrackSummary, Walker};
use crate::scan::Scan;
use crate::tests_support::block_on;
use crate::{Config, Durability, FullPolicy, Logring, SessionInfo, TrackDesc};
use crate::{FullVersion, UNib32};
use wire_weaver::ww_version::Version;
use ww_stream::store::log::SimSd;

const BUF: usize = 1024;
const TRACKS: usize = 6;
/// The image size the harness uses: the fuzzer's input is laid over the start of it.
pub const IMAGE: usize = 4096 * 8;

static TRACKS_DESC: [TrackDesc<'static>; 2] = [
    TrackDesc {
        signature: Some([1; 8]),
        global_id: None,
        durability: Durability::Normal,
        rate: None,
        ..TrackDesc::new(&[UNib32(3), UNib32(0), UNib32(1)])
    },
    TrackDesc {
        signature: Some([2; 8]),
        global_id: None,
        durability: Durability::Immediate,
        rate: None,
        ..TrackDesc::new(&[UNib32(5)])
    },
];

/// The session the harness logs with.
pub fn info() -> SessionInfo<'static> {
    SessionInfo {
        api: FullVersion::new("fw", Version::new(1, 0, 0)),
        device_id: "fuzz",
        fw: "fw 1.0.0",
        hw: "none",
        tracks: &TRACKS_DESC,
        bundle: Some(b"a small bundle"),
    }
}

/// The config the harness mounts with.
pub fn config() -> Config {
    Config {
        segment_size: 4096,
        meta_segments: 2,
        flush_bytes: 1024,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 3,
        ..Config::default()
    }
}

/// Mount `data` (padded or cut to [IMAGE] bytes) as a ring, push a few records, sync, remount, push, scan.
/// Returns whether the first mount found a log (not formatted), for the corruption test's statistics.
pub fn mount_garbage(data: &[u8]) -> bool {
    let mut image = vec![0u8; IMAGE];
    let n = data.len().min(IMAGE);
    image[..n].copy_from_slice(&data[..n]);
    let mut medium = SimSd::from_image(image, 1);
    read_everything(&mut medium);
    let cfg = config();
    let Ok(mut lr) = block_on(Logring::<_, BUF, TRACKS>::mount(medium, cfg, info())) else {
        return false;
    };
    let found = !lr.recovery().formatted;
    for i in 0..5u8 {
        let item = [i; 7];
        let _ = block_on(lr.push(1, &item, None));
        let _ = block_on(lr.push(2, &item[..3], None));
    }
    let _ = block_on(lr.marker(1, "m", None));
    let _ = block_on(lr.sync());
    let image = lr.into_medium().image();
    if let Ok(mut lr) = block_on(Logring::<_, BUF, TRACKS>::mount(
        SimSd::from_image(image, 2),
        cfg,
        info(),
    )) {
        let _ = block_on(lr.push(1, &[9; 20], None));
        let _ = block_on(lr.sync());
        let mut medium = lr.into_medium();
        let _ = block_on(Scan::open(&mut medium, None));
        let _ = block_on(Scan::open(&mut medium, Some(4096)));
        read_everything(&mut medium);
    }
    found
}

/// Open `medium` with the indexed reader and use all of it: a walk over everything, seeks by track position and
/// by time, summaries, recordings. Results are ignored; nothing may panic or loop.
pub fn read_everything<M: ww_stream::store::LogMedium>(medium: &mut M) {
    let Ok(mut rd) = block_on(Reader::open(medium, None)) else {
        return;
    };
    let range = rd.range(Role::Data);
    let mut item = vec![0u8; 4096];
    let mut w = Walker::at_segment(AllTracks::<TRACKS>::default(), Role::Data, range.oldest);
    w.truncate = true;
    let mut keys = vec![0u64];
    let mut sessions = vec![];
    while let Ok(Some(f)) = block_on(w.next(&mut rd, medium, &mut item)) {
        keys.extend(w.decode.key(f.track));
        sessions.push(f.session);
    }
    keys.sort();
    keys.dedup();
    for key in keys.iter().take(8) {
        for pos in [0, 1, 5, 1 << 20, u64::MAX] {
            if let Ok(mut w) = block_on(rd.seek_track(medium, *key, 0, pos)) {
                w.truncate = true;
                let _ = block_on(w.next(&mut rd, medium, &mut item));
            }
        }
    }
    sessions.dedup();
    for s in sessions.iter().take(4) {
        let t = ww_stream::Second {
            prefix: ww_si::Prefix::Micro,
            value: ww_si::ww_numeric::NumericValue::U32(1000),
        };
        let _ = block_on(rd.seek_time::<TRACKS, _>(medium, *s, &t));
    }
    if let Some(tail) = range.tail {
        let mut sums = [TrackSummary::default(); TRACKS];
        for seq in range.oldest..=tail.min(range.oldest + 64) {
            let _ = block_on(rd.summary::<TRACKS, _>(medium, seq, &mut sums));
        }
    }
    let mut recs = vec![];
    let _ = block_on(rd.recordings(medium, |r| recs.push(r)));
    for r in recs.iter().take(4) {
        let mut w = rd.recording::<TRACKS>(r);
        w.walker.truncate = true;
        while let Ok(Some(_)) = block_on(w.next(&mut rd, medium, &mut item)) {}
    }
}
