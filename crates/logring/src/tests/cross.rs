//! Cards written by one build, mounted by the other: each build writes the same boots into an image
//! (`testdata/cross-full.wlr` with the feature `tail-positions`, `testdata/cross-small.wlr` without), checked in, so
//! a test run of either build reads and mounts what the other one really wrote. Both images end in a record a
//! power cut tore. Regenerate after a format change: `cargo test -p logring --features std write_cross_image --
//! --ignored`, then the same with `--no-default-features --features std`.

use super::reader::{Ref, reference, walk_all};
use super::writer::{Lr, cfg, info, meta_positions, scan};
use super::{FULL, block_on};
use crate::format::header::SEG_FRESH;
use crate::reader::Reader;
use crate::scan::Scan;
use crate::{Buffers, Config, Error, STORE_SESSION};
use std::collections::HashMap;
use ww_stream::store::log::{SimError, SimSd};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");
/// Boots before the cut one (which is boot `BOOTS`), and records per boot.
const BOOTS: u8 = 3;
const PER_BOOT: u32 = 40;
/// Records the cut boot pushes after its sync; the power fails in one of them.
const TAIL: u32 = 30;

fn config() -> Config {
    cfg(4096, 1024)
}

fn image_size() -> usize {
    super::writer::size(&config(), 40)
}

fn file(full: bool) -> String {
    format!("{DIR}/cross-{}.wlr", if full { "full" } else { "small" })
}

/// Record `k` of boot `boot`: its track and item (1-400 bytes, so some records span blocks).
fn record(boot: u8, k: u32) -> (u32, Vec<u8>) {
    let len = 1 + (k as usize * 37 + boot as usize * 11) % 400;
    let item = (0..len)
        .map(|j| (j as u32 * 3 + k + boot as u32 * 50) as u8)
        .collect();
    (1 + k % 3, item)
}

/// Every record the boots push, in order: [PER_BOOT] per boot, the cut boot's included, then its [TAIL].
fn pushed() -> Vec<(u32, Vec<u8>)> {
    let mut out: Vec<_> = (0..=BOOTS)
        .flat_map(|b| (0..PER_BOOT).map(move |k| record(b, k)))
        .collect();
    out.extend((0..TAIL).map(|k| record(BOOTS, PER_BOOT + k)));
    out
}

/// One boot with this build's mount: records, markers, a recording started in boot 1 and left running, a sync;
/// the last boot pushes [TAIL] more after the sync with the power failing at write unit `cut`.
fn boot(image: Option<Vec<u8>>, b: u8, cut: Option<u64>) -> (Vec<u8>, u64) {
    let mut medium = match image {
        Some(i) => SimSd::from_image(i, b as u64),
        // a zeroed card, so the checked-in images compress
        None => SimSd::from_image(vec![0; image_size()], 1),
    };
    if let Some(c) = cut {
        medium.cut_power_at(c);
    }
    let mut bufs = Box::new(Buffers::new());
    let mut lr: Lr<SimSd> = Lr::new(medium, &mut bufs, config(), info()).unwrap();
    block_on(lr.mount()).unwrap();
    for k in 0..PER_BOOT {
        let (t, item) = record(b, k);
        block_on(lr.push(t, &item, None)).unwrap();
        if k % 17 == 0 {
            block_on(lr.marker(1, "m", Some(k as i64))).unwrap();
        }
    }
    if b == 1 {
        block_on(lr.recording_start(1)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let synced = lr.medium().ops();
    if b == BOOTS {
        let r: Result<(), Error<SimError>> = (|| {
            for k in 0..TAIL {
                let (t, item) = record(b, PER_BOOT + k);
                block_on(lr.push(t, &item, None))?;
            }
            block_on(lr.sync())
        })();
        match r {
            Ok(()) | Err(Error::Medium(SimError::PowerCut)) => {}
            Err(e) => panic!("{e:?}"),
        }
    }
    (lr.into_medium().image(), synced)
}

/// What this build writes: [BOOTS] boots, then one cut in its tail at the first write unit that leaves a torn
/// record (a deterministic search, so the image is the same at every run).
fn write_image() -> Vec<u8> {
    let mut image = None;
    for b in 0..BOOTS {
        image = Some(boot(image, b, None).0);
    }
    let image = image.unwrap();
    let (_, synced) = boot(Some(image.clone()), BOOTS, None);
    for cut in synced.. {
        let (out, _) = boot(Some(image.clone()), BOOTS, Some(cut));
        let s = scan(&mut SimSd::from_image(out.clone(), 0));
        if s.data.last().unwrap().torn_record {
            return out;
        }
        assert!(cut < synced + 200, "no cut tore a record");
    }
    unreachable!()
}

/// This build's image, for the other build's tests: `cargo test ... write_cross_image -- --ignored`.
#[test]
#[ignore]
fn write_cross_image() {
    std::fs::create_dir_all(DIR).unwrap();
    std::fs::write(file(FULL), write_image()).unwrap();
}

/// The checked-in image of this build is what it writes now: the other build's tests read the real thing.
#[test]
fn cross_image_is_current() {
    let want = std::fs::read(file(FULL)).unwrap();
    assert!(
        write_image() == want,
        "{} is stale: regenerate it (module docs)",
        file(FULL)
    );
}

/// The data records of `s` by track, items only: positions differ between the builds, the items don't.
fn items(s: &Scan) -> HashMap<u32, Vec<Vec<u8>>> {
    let mut by: HashMap<u32, Vec<Vec<u8>>> = HashMap::new();
    for (_, r) in s.records().filter(|(_, r)| r.track != 0) {
        by.entry(r.track).or_default().push(r.item.clone());
    }
    by
}

/// Scan `image` (the ring didn't wrap) and walk it with the reader: the walk is the scan's.
fn read(image: &[u8], what: &str) -> Scan {
    let mut m = SimSd::from_image(image.to_vec(), 0);
    let s = scan(&mut m);
    assert_eq!(
        s.superblock.segments_reused, 0,
        "{what}: the ring didn't wrap"
    );
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let walked: Vec<Ref> = walk_all(&mut rd, &mut m);
    assert!(
        walked == reference(&s),
        "{what}: the reader's walk differs from the scan"
    );
    s
}

/// What the boots wrote is on the card: every synced record, in order per track, then a prefix of the cut tail.
fn check_pushed(s: &Scan, what: &str) {
    let pushed = pushed();
    let synced = ((BOOTS as u32 + 1) * PER_BOOT) as usize;
    let got = items(s);
    for t in 1..=3 {
        let want: Vec<&Vec<u8>> = pushed.iter().filter(|p| p.0 == t).map(|p| &p.1).collect();
        let n_synced = pushed[..synced].iter().filter(|p| p.0 == t).count();
        let have = &got[&t];
        assert!(
            have.len() >= n_synced && have.len() <= want.len(),
            "{what}: track {t}: {} records, {n_synced} synced",
            have.len()
        );
        let bad = have.iter().zip(&want).position(|(a, b)| a != *b);
        assert!(bad.is_none(), "{what}: track {t}: record {bad:?} differs");
    }
}

/// The sessions whose first data segment carries `SEG_FRESH`.
fn fresh_sessions(s: &Scan) -> Vec<u32> {
    let mut out = Vec::new();
    let mut last = None;
    for seg in &s.data {
        if last != Some(seg.session) && seg.flags & SEG_FRESH != 0 {
            out.push(seg.session);
        }
        last = Some(seg.session);
    }
    out
}

/// Read `written_full`'s image (a full build's or a small build's) and mount it with this build's mount, then
/// log on: nothing is lost, the mount does what this build does, and the result reads back whole.
fn mount_image_of(written_full: bool) {
    let what = format!(
        "{} image, {} mount",
        if written_full { "full" } else { "small" },
        if FULL { "full" } else { "small" }
    );
    let image = std::fs::read(file(written_full)).unwrap();
    let before = read(&image, &what);
    check_pushed(&before, &what);
    assert!(
        before.data.last().unwrap().torn_record,
        "{what}: the tail is torn"
    );
    // a small build's sessions after the first open FRESH, every data track in a stream session with bit 31
    let want_fresh: Vec<u32> = if written_full {
        vec![]
    } else {
        (2..=BOOTS as u32 + 1).collect()
    };
    assert_eq!(fresh_sessions(&before), want_fresh, "{what}");
    let ends: Vec<u64> = (0..4)
        .map(|t| {
            before
                .records()
                .filter(|(_, r)| r.track == t)
                .last()
                .map_or(0, |(_, r)| r.head.end())
        })
        .collect();

    let mut bufs = Box::new(Buffers::new());
    let mut lr: Lr<SimSd> =
        Lr::new(SimSd::from_image(image, 5), &mut bufs, config(), info()).unwrap();
    block_on(lr.mount()).unwrap();
    let rec = lr.recovery();
    assert!(!rec.formatted, "{what}");
    assert_eq!(rec.positions_restarted, !FULL, "{what}");
    // a full mount counts the torn tail whoever wrote it; a small one counts no records
    assert_eq!(rec.torn_records, FULL as u32, "{what}: {rec:?}");
    assert_eq!(lr.session(), BOOTS as u32 + 2, "{what}");
    assert!(lr.recording().is_some(), "{what}: the recording runs on");
    for t in 1..4u32 {
        let (session, start) = lr.stream_session(t).unwrap();
        if FULL {
            assert_eq!(
                lr.track_state(t).unwrap().next,
                ends[t as usize],
                "{what}: track {t}"
            );
        } else {
            assert_eq!(lr.track_state(t).unwrap().next, 0, "{what}: track {t}");
            assert_eq!(
                (session, start),
                (STORE_SESSION | lr.session(), lr.data_seq()),
                "{what}"
            );
        }
    }
    let more: Vec<(u32, Vec<u8>)> = (0..40).map(|k| record(9, k)).collect();
    for (t, item) in &more {
        block_on(lr.push(*t, item, None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let after_image = lr.into_medium().image();

    let after = read(&after_image, &what);
    let (old, new) = (items(&before), items(&after));
    for t in 1..=3 {
        let n = old[&t].len();
        assert_eq!(new[&t][..n], old[&t][..], "{what}: track {t} kept");
        let added: Vec<&Vec<u8>> = more.iter().filter(|p| p.0 == t).map(|p| &p.1).collect();
        assert!(new[&t][n..].iter().eq(added), "{what}: track {t} logged on");
    }
    let mut fresh = want_fresh;
    if !FULL {
        fresh.push(BOOTS as u32 + 2);
    }
    assert_eq!(fresh_sessions(&after), fresh, "{what}");
    let meta = meta_positions(&after);
    assert!(meta.windows(2).all(|w| w[0] < w[1]), "{what}: {meta:?}");
    // a store session (bit 31) names one stream of a track, never two
    let mut streams: HashMap<(u64, u32), u64> = HashMap::new();
    for seg in &after.data {
        for t in seg
            .tracks
            .iter()
            .filter(|t| t.stream_session & STORE_SESSION != 0)
        {
            let start = *streams
                .entry((t.key, t.stream_session))
                .or_insert(t.stream_start);
            assert_eq!(
                start, t.stream_start,
                "{what}: session {:x} reused",
                t.stream_session
            );
        }
    }
}

#[test]
fn mounts_a_full_builds_card() {
    mount_image_of(true);
}

#[test]
fn mounts_a_small_builds_card() {
    mount_image_of(false);
}
