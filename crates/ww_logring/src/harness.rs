//! The mount-on-garbage harness (`std`): what the fuzz target `fuzz/fuzz_targets/mount.rs` and the corruption
//! test run. Takes any bytes as a ring image, mounts, logs, remounts and scans; nothing may panic.

use crate::scan::Scan;
use crate::tests_support::block_on;
use crate::{Config, Durability, FullPolicy, Logring, SessionInfo, TrackDesc};
use ww_stream::store::log::SimSd;

const BUF: usize = 1024;
const TRACKS: usize = 6;
/// The image size the harness uses: the fuzzer's input is laid over the start of it.
pub const IMAGE: usize = 4096 * 8;

static TRACKS_DESC: [TrackDesc<'static>; 2] = [
    TrackDesc {
        path: "can[0].rx",
        crate_name: "ww_can_bus",
        crate_version: "0.3.0",
        type_path: "Frame",
        signature: [1; 8],
        global_id: None,
        units: "frames",
        durability: Durability::Normal,
        rate: None,
    },
    TrackDesc {
        path: "events",
        crate_name: "fw",
        crate_version: "1.0.0",
        type_path: "Event",
        signature: [2; 8],
        global_id: None,
        units: "",
        durability: Durability::Immediate,
        rate: None,
    },
];

/// The session the harness logs with.
pub fn info() -> SessionInfo<'static> {
    SessionInfo {
        api_crate: "fw",
        api_major: 1,
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
    let medium = SimSd::from_image(image, 1);
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
    }
    found
}
