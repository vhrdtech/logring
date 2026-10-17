//! The fuzz harness over real images with random damage: bit flips, blocks zeroed or filled, slices moved,
//! truncation. No panic, ever; and most damaged images still mount as the same log.

use super::{Rng, block_on};
use crate::harness::{IMAGE, mount_garbage};
use crate::{Config, Durability, FullPolicy, Logring, SessionInfo, TrackDesc};
use crate::{FullVersion, UNib32};
use wire_weaver::ww_version::Version;
use ww_stream::store::log::SimSd;

#[test]
fn corrupted_images_never_panic() {
    // a real image: a few sessions of logging
    let tracks = [
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
    let info = SessionInfo {
        api: FullVersion::new("fw", Version::new(1, 0, 0)),
        device_id: "fuzz",
        fw: "fw 1.0.0",
        hw: "none",
        tracks: &tracks,
        bundle: Some(b"a small bundle"),
    };
    let cfg = Config {
        segment_size: 4096,
        meta_segments: 2,
        flush_bytes: 1024,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 3,
        ..Config::default()
    };
    let mut image = vec![0u8; IMAGE];
    let mut rng = Rng::new(5);
    for _ in 0..3 {
        let mut lr = block_on(Logring::<_, 1024, 6>::mount(
            SimSd::from_image(image, rng.next()),
            cfg,
            info,
        ))
        .unwrap();
        for i in 0..60u64 {
            let len = rng.range(1, 300) as usize;
            let item: Vec<u8> = (0..len).map(|j| (i * 3 + j as u64) as u8).collect();
            block_on(lr.push(1 + (i % 2) as u32, &item, None)).unwrap();
        }
        block_on(lr.sync()).unwrap();
        image = lr.into_medium().image();
    }
    assert!(mount_garbage(&image), "the image mounts as the log it is");

    let mut found = 0;
    let runs = 400;
    for _ in 0..runs {
        let mut img = image.clone();
        match rng.range(0, 5) {
            0 => {
                for _ in 0..rng.range(1, 20) {
                    let at = rng.range(0, IMAGE as u64 - 1) as usize;
                    img[at] ^= 1 << rng.range(0, 7);
                }
            }
            1 => {
                let b = rng.range(0, IMAGE as u64 / 512 - 1) as usize * 512;
                img[b..b + 512].fill(rng.next() as u8);
            }
            2 => {
                let from = rng.range(0, IMAGE as u64 - 1024) as usize;
                let to = rng.range(0, IMAGE as u64 - 1024) as usize;
                let chunk: Vec<u8> = img[from..from + 1024].to_vec();
                img[to..to + 1024].copy_from_slice(&chunk);
            }
            3 => {
                let n = rng.range(0, IMAGE as u64) as usize;
                img.truncate(n);
            }
            4 => {
                for b in img.iter_mut() {
                    if rng.chance(3) {
                        *b = rng.next() as u8;
                    }
                }
            }
            _ => {
                let at = rng.range(0, IMAGE as u64 - 64) as usize;
                for b in &mut img[at..at + 64] {
                    *b = 0xff;
                }
            }
        }
        if mount_garbage(&img) {
            found += 1;
        }
    }
    assert!(
        found > runs / 2,
        "most damaged images still mount as the log: {found}/{runs}"
    );
    // plain garbage and empty input
    mount_garbage(&[]);
    mount_garbage(&[0xff; 100]);
    let garbage: Vec<u8> = (0..IMAGE).map(|_| rng.next() as u8).collect();
    mount_garbage(&garbage);
}

/// Seed images for the fuzzer: `cargo test -p ww_logring --features std write_fuzz_seeds -- --ignored`, then
/// `cd fuzz && cargo +nightly fuzz run mount corpus/mount seeds`.
#[test]
#[ignore]
fn write_fuzz_seeds() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/seeds");
    std::fs::create_dir_all(dir).unwrap();
    let cfg = crate::harness::config();
    let info = crate::harness::info();
    let mut rng = Rng::new(9);
    let mut image = vec![0u8; IMAGE];
    for seed in 0..3 {
        let mut lr = block_on(Logring::<_, 1024, 6>::mount(
            SimSd::from_image(image, rng.next()),
            cfg,
            info,
        ))
        .unwrap();
        for i in 0..(40 * (seed + 1)) {
            let len = rng.range(1, 200) as usize;
            let item: Vec<u8> = (0..len).map(|j| (i * 5 + j as u64) as u8).collect();
            block_on(lr.push(1 + (i % 2) as u32, &item, None)).unwrap();
            if i % 9 == 0 {
                block_on(lr.marker(1, "m", Some(i as i64))).unwrap();
            }
        }
        if seed == 1 {
            block_on(lr.recording_start(1)).unwrap();
        }
        block_on(lr.sync()).unwrap();
        image = lr.into_medium().image();
        std::fs::write(format!("{dir}/session{}.wlr", seed + 1), &image).unwrap();
    }
}
