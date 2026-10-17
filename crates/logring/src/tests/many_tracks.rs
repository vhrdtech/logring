//! 1024 data tracks on a simulated SD card: every track round-trips and continues after a remount, the reader
//! seeks a quiet and a busy track, and a few busy tracks pay only for themselves in the flush heads.

use super::block_on;
use super::writer::{Ev, assert_same, scan, scanned, us};
use crate::reader::Reader;
use crate::{Config, FullPolicy, FullVersion, Logring, SessionInfo, StreamHead, TrackDesc, UNib32};
use wire_weaver::ww_version::Version;
use ww_stream::store::log::SimSd;

const N: usize = 1024;
const TRACKS: usize = N + 1;
const BUF: usize = 4096;
type Lr = Logring<'static, SimSd, BUF, TRACKS>;

/// Track `i + 1` is the stream at `[2, i]`, as in `ww_impl!(sensor[]: ...)` at resource 2.
static PATHS: [[UNib32; 2]; N] = {
    let mut p = [[UNib32(0); 2]; N];
    let mut i = 0;
    while i < N {
        p[i] = [UNib32(2), UNib32(i as u32)];
        i += 1;
    }
    p
};

fn info() -> SessionInfo<'static> {
    info_of(&PATHS)
}

fn info_of(paths: &'static [[UNib32; 2]]) -> SessionInfo<'static> {
    let tracks: Vec<TrackDesc<'static>> = paths
        .iter()
        .map(|p| TrackDesc {
            signature: Some([9; 8]),
            ..TrackDesc::new(p)
        })
        .collect();
    SessionInfo {
        api: FullVersion::new("many_fw", Version::new(1, 0, 0)),
        device_id: "many",
        fw: "many_fw 1.0.0",
        hw: "sim",
        tracks: Box::leak(tracks.into_boxed_slice()),
        bundle: Some(b"a bundle"),
    }
}

fn cfg() -> Config {
    Config {
        segment_size: 1 << 20,
        meta_segments: 2,
        flush_bytes: 4096,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 3,
        ..Config::default()
    }
}

fn mount(m: SimSd) -> Box<Lr> {
    Box::new(block_on(Lr::mount_leaked(m, cfg(), info())).unwrap())
}

const SIZE: usize = (1 + 2 + 6) << 20;
const BUSY: usize = 30_000;

/// Bytes the busy part of the workload takes on a log of the three busy tracks alone.
fn three_tracks_alone() -> u64 {
    static THREE: [[UNib32; 2]; 3] = [
        [UNib32(2), UNib32(0)],
        [UNib32(2), UNib32(511)],
        [UNib32(2), UNib32(1023)],
    ];
    let mut lr = Box::new(
        block_on(Lr::mount_leaked(
            SimSd::new(SIZE, 1),
            cfg(),
            info_of(&THREE),
        ))
        .unwrap(),
    );
    let mut next = vec![0u64; 4];
    let mut pushed = Vec::new();
    let before = lr.superblock().bytes_written;
    for k in 0..BUSY {
        push(&mut lr, &mut next, &mut pushed, 1 + (k % 3) as u32, 60);
    }
    block_on(lr.sync()).unwrap();
    lr.superblock().bytes_written - before
}

fn push(lr: &mut Lr, next: &mut [u64], pushed: &mut Vec<Ev>, track: u32, len: usize) {
    let i = pushed.len();
    let ev = Ev {
        track,
        head: StreamHead {
            pos: next[track as usize],
            units: 1,
            lost: None,
            t: Some(us(i as u32 * 10)),
            ext: None,
        },
        item: (0..len).map(|j| (i * 7 + j) as u8).collect(),
    };
    next[track as usize] += 1;
    block_on(lr.push_head(track, &ev.head, &ev.item)).unwrap();
    pushed.push(ev);
}

fn run() {
    let mut lr = mount(SimSd::new(SIZE, 1));
    let mut next = vec![0u64; TRACKS];
    let mut pushed = Vec::new();
    // every track once
    for t in 1..=N as u32 {
        push(&mut lr, &mut next, &mut pushed, t, 4);
    }
    block_on(lr.sync()).unwrap();
    // then three busy tracks over more than a segment: from the next segment on, only they are in flush heads
    let busy = [1u32, 512, 1024];
    let before = lr.superblock().bytes_written;
    for k in 0..BUSY {
        push(&mut lr, &mut next, &mut pushed, busy[k % 3], 60);
    }
    block_on(lr.sync()).unwrap();
    let written = lr.superblock().bytes_written - before;
    // the 1021 quiet tracks cost their table in every segment head (about 37 KB per 1 MB segment), a full flush
    // head now and then and a longer track number in the records; a position for every track in every 4 KB flush
    // head would cost over half again
    let alone = three_tracks_alone();
    assert!(
        written * 100 < alone * 110,
        "{written} bytes written, {alone} with the three tracks alone"
    );
    assert!(lr.data_seq() >= 2, "the busy part spans segments");

    // everything is on the card, in order
    let mut m = lr.into_medium();
    let s = scan(&mut m);
    assert!(s.data.iter().all(|seg| !seg.torn_block && !seg.torn_record));
    assert_eq!(s.data[0].tracks.len(), N);
    assert_eq!(s.data[0].tracks[N - 1].path, [2, N as u32 - 1]);
    assert_same(&scanned(&s), &pushed, "1024 tracks");

    // the reader: a quiet track (its one record, in the first segment) and a busy one in the middle
    let key_of = |lr: &Lr, t: u32| lr.track_key(t).unwrap();
    let lr = mount(SimSd::from_image(m.image(), 2));
    for t in 1..=N as u32 {
        assert_eq!(
            lr.track_state(t).unwrap().next,
            if super::FULL { next[t as usize] } else { 0 },
            "track {t} resumed"
        );
    }
    let (quiet, mid) = (key_of(&lr, 700), key_of(&lr, 512));
    let mut m = lr.into_medium();
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let mut item = [0u8; 64];
    let mut w = block_on(rd.seek_track(&mut m, quiet, 0, 0)).unwrap();
    let f = block_on(w.next(&mut rd, &mut m, &mut item))
        .unwrap()
        .unwrap();
    assert_eq!((f.track, f.head.pos), (700, 0));
    let want = &pushed
        .iter()
        .filter(|e| e.track == 512)
        .nth(6000)
        .unwrap()
        .head;
    let mut w = block_on(rd.seek_track(&mut m, mid, 0, want.pos)).unwrap();
    let f = block_on(w.next(&mut rd, &mut m, &mut item))
        .unwrap()
        .unwrap();
    assert_eq!(&f.head, want);
}

/// What one more track costs in a writer's RAM (`docs/format.md` §9 says it).
#[test]
fn ram_per_track() {
    let one = core::mem::size_of::<Logring<'static, SimSd, 512, 1>>();
    let many = core::mem::size_of::<Logring<'static, SimSd, 512, TRACKS>>();
    let per = (many - one) / N;
    assert!(per <= 152, "{per} bytes per track");
}

#[test]
fn a_thousand_tracks() {
    // the writer for 1025 tracks keeps ~160 bytes per track: more than a test thread's stack in a debug build
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
