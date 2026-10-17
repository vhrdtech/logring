//! The writer on simulated SD and NOR media.

use super::{Counting, Rng, block_on};
use crate::format::BLOCK;
use crate::scan::Scan;
use crate::system::SystemEvent;
use crate::{Config, Durability, Error, FullPolicy, Logring, SessionInfo, StreamHead, TrackDesc};
use ww_si::Prefix;
use ww_si::ww_numeric::NumericValue;
use ww_stream::Second;
use ww_stream::store::LogMedium;
use ww_stream::store::log::{SimError, SimMedium, SimNor, SimSd};

const BUF: usize = 1024;
const TRACKS: usize = 8;
type Lr<'a, M> = Logring<'a, M, BUF, TRACKS>;

static BUNDLE: [u8; 3000] = {
    let mut b = [0u8; 3000];
    let mut i = 0;
    while i < 3000 {
        b[i] = (i * 7 % 251) as u8;
        i += 1;
    }
    b
};

static TRACKS_DESC: [TrackDesc<'static>; 3] = [
    TrackDesc {
        path: "can[0].rx",
        crate_name: "ww_can_bus",
        crate_version: "0.3.0",
        type_path: "Frame",
        signature: [1; 8],
        global_id: Some(5),
        units: "frames",
        durability: Durability::Normal,
        rate: Some(1000),
    },
    TrackDesc {
        path: "adc[0].samples",
        crate_name: "usb_io_fw",
        crate_version: "1.0.0",
        type_path: "Samples",
        signature: [2; 8],
        global_id: None,
        units: "sets",
        durability: Durability::Normal,
        rate: None,
    },
    TrackDesc {
        path: "events",
        crate_name: "usb_io_fw",
        crate_version: "1.0.0",
        type_path: "Event",
        signature: [3; 8],
        global_id: None,
        units: "",
        durability: Durability::Immediate,
        rate: None,
    },
];

fn info() -> SessionInfo<'static> {
    SessionInfo {
        api_crate: "usb_io_fw",
        api_major: 1,
        device_id: "rePort-0001",
        fw: "usb_io_fw 1.0.0",
        hw: "rev A",
        tracks: &TRACKS_DESC,
        bundle: Some(&BUNDLE),
    }
}

fn cfg(segment_size: u32, flush_bytes: u32) -> Config {
    Config {
        segment_size,
        meta_segments: 2,
        flush_bytes,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 7,
        ..Config::default()
    }
}

fn size(cfg: &Config, data_segments: u64) -> usize {
    (cfg.segment_size as u64 * (1 + cfg.meta_segments as u64 + data_segments)) as usize
}

fn us(v: u32) -> Second {
    Second {
        prefix: Prefix::Micro,
        value: NumericValue::U32(v),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Ev {
    track: u32,
    head: StreamHead,
    item: Vec<u8>,
}

/// A random multi-track workload: varied sizes (some records span blocks, some are bigger than a stage buffer),
/// positions like a sender's, times mostly as deltas, an occasional `lost`, markers, recordings, syncs.
struct Gen {
    rng: Rng,
    i: u64,
    next: [u64; 4],
    now_ms: u64,
    max_item: usize,
}

impl Gen {
    fn new(seed: u64, max_item: usize) -> Self {
        Gen {
            rng: Rng::new(seed),
            i: 0,
            next: [0; 4],
            now_ms: 0,
            max_item,
        }
    }

    fn event(&mut self) -> Ev {
        let i = self.i;
        self.i += 1;
        let track = self.rng.range(1, 3) as u32;
        let len = match self.rng.range(0, 99) {
            0..=69 => self.rng.range(1, 40),
            70..=94 => self.rng.range(40, 600),
            _ => self.rng.range(600, self.max_item as u64),
        } as usize;
        let lost = self.rng.chance(5).then(|| self.rng.range(1, 9));
        let pos = self.next[track as usize] + lost.unwrap_or(0);
        let units = if self.rng.chance(20) {
            self.rng.range(2, 50)
        } else {
            1
        };
        self.next[track as usize] = pos + units;
        let t = match self.rng.range(0, 99) {
            0..=79 => Some(us((i * 1000 + self.rng.range(0, 999)) as u32)),
            80..=89 => None,
            _ => Some(Second {
                prefix: Prefix::Milli,
                value: NumericValue::U64(i * 3),
            }),
        };
        Ev {
            track,
            head: StreamHead {
                pos,
                units,
                lost,
                t,
                ext: None,
            },
            item: (0..len).map(|j| (i as usize * 13 + j) as u8).collect(),
        }
    }
}

/// Push `n` events (and markers, recordings, syncs along the way); returns the events stored, and the event a
/// power cut interrupted, if one did.
fn workload<M: LogMedium>(
    lr: &mut Lr<M>,
    g: &mut Gen,
    n: usize,
    pushed: &mut Vec<Ev>,
) -> Result<(), Error<M::Error>> {
    for k in 0..n {
        let ev = g.event();
        lr.track_state(ev.track).unwrap();
        block_on(lr.push_head(ev.track, &ev.head, &ev.item))?;
        pushed.push(ev);
        if k % 7 == 3 {
            g.now_ms += 400;
            block_on(lr.flush_if_due(g.now_ms))?;
        }
        if g.rng.chance(2) {
            block_on(lr.sync())?;
        }
        if g.rng.chance(1) {
            block_on(lr.marker(1, "button", Some(k as i64)))?;
        }
        if g.rng.chance(1) {
            if lr.recording().is_some() {
                block_on(lr.recording_stop(2))?;
            } else {
                block_on(lr.recording_start(1))?;
            }
        }
    }
    Ok(())
}

/// Compare two event lists, reporting the first difference briefly.
#[track_caller]
fn assert_same(got: &[Ev], want: &[Ev], what: &str) {
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        assert!(
            g == w,
            "{what}: record {i} differs: got track {} {:?} {} bytes, want track {} {:?} {} bytes",
            g.track,
            g.head,
            g.item.len(),
            w.track,
            w.head,
            w.item.len()
        );
    }
    assert_eq!(got.len(), want.len(), "{what}: lengths");
}

fn scanned(scan: &Scan) -> Vec<Ev> {
    scan.records()
        .filter(|(_, r)| r.track != 0)
        .map(|(_, r)| Ev {
            track: r.track,
            head: r.head,
            item: r.item.clone(),
        })
        .collect()
}

fn scan<M: LogMedium>(medium: &mut M) -> Scan
where
    M::Error: core::fmt::Debug,
{
    block_on(Scan::open(medium, None)).unwrap()
}

fn mount<M: LogMedium>(medium: M, cfg: Config) -> Lr<'static, M>
where
    M::Error: core::fmt::Debug,
{
    block_on(Lr::mount(medium, cfg, info())).unwrap()
}

#[test]
fn round_trip_without_wrap() {
    let cfg = cfg(8192, 2048);
    let mut lr = mount(SimSd::new(size(&cfg, 64), 1), cfg);
    assert!(lr.recovery().formatted);
    assert_eq!(lr.session(), 1);
    let mut g = Gen::new(1, 2000);
    let mut pushed = Vec::new();
    workload(&mut lr, &mut g, 1500, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    let mut medium = lr.into_medium();
    let scan = scan(&mut medium);
    assert_same(&scanned(&scan), &pushed, "round trip");
    // every segment names the tracks, session 1
    for s in &scan.data {
        assert_eq!(s.session, 1);
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.tracks[0].path, "can[0].rx");
        assert_eq!(s.tracks[2].durability, Durability::Immediate);
        assert!(!s.torn_block && !s.torn_record, "{s:?}");
    }
    assert!(scan.data.len() > 10, "the workload spans segments");
    // the system track: a session start first, then markers and recordings
    let sys: Vec<_> = scan
        .records()
        .filter(|(_, r)| r.track == 0)
        .map(|(_, r)| r.item.clone())
        .collect();
    let first = SystemEvent::decode(&mut crate::format::Dec::new(&sys[0])).unwrap();
    assert!(matches!(
        first,
        SystemEvent::SessionStart {
            session: 1,
            formatted: true,
            device_id: "rePort-0001",
            ..
        }
    ));
    assert!(sys.len() > 5);
    // the meta area: a bundle and the session
    let meta: Vec<_> = scan.meta.iter().flat_map(|s| s.records.iter()).collect();
    assert!(meta.len() >= 2);
    assert_eq!(meta[0].item.len(), 1 + 8 + 4 + BUNDLE.len());
    assert_eq!(&meta[0].item[13..], &BUNDLE[..]);
}

#[test]
fn round_trip_with_wrap_keeps_the_newest() {
    let cfg = cfg(4096, 1024);
    let mut lr = mount(Counting::new(SimSd::new(size(&cfg, 12), 2)), cfg);
    let mut g = Gen::new(2, 1500);
    let mut pushed = Vec::new();
    workload(&mut lr, &mut g, 2000, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    assert!(lr.superblock().segments_reused > 0);
    let mut medium = lr.into_medium();
    let scan = scan(&mut medium);
    let got = scanned(&scan);
    assert!(!got.is_empty() && got.len() < pushed.len());
    assert_same(&got, &pushed[pushed.len() - got.len()..], "wrap");
    assert_eq!(scan.data.len(), 12);
}

#[test]
fn remount_continues_positions_in_a_new_session() {
    let cfg = cfg(8192, 1024);
    let mut lr = mount(SimSd::new(size(&cfg, 80), 3), cfg);
    let mut g = Gen::new(3, 1000);
    let mut pushed = Vec::new();
    workload(&mut lr, &mut g, 300, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    let ends: Vec<u64> = (0..4).map(|t| lr.track_state(t).unwrap().next).collect();
    let rec_id = lr.recording();
    let image = lr.into_medium().image();

    let mut lr = mount(SimSd::from_image(image, 4), cfg);
    assert_eq!(lr.session(), 2);
    assert!(!lr.recovery().formatted);
    assert_eq!(lr.recovery().torn_blocks, 0);
    assert_eq!(lr.recovery().torn_records, 0);
    assert_eq!(lr.recording(), rec_id, "a recording survives a reboot");
    // positions continue: track 0 got the session start record of session 2 already
    assert_eq!(lr.track_state(0).unwrap().next, ends[0] + 1);
    for t in 1..4 {
        assert_eq!(
            lr.track_state(t).unwrap().next,
            ends[t as usize],
            "track {t}"
        );
    }
    let mut more = Vec::new();
    workload(&mut lr, &mut g, 100, &mut more).unwrap();
    block_on(lr.sync()).unwrap();
    let mut medium = lr.into_medium();
    let scan = scan(&mut medium);
    assert_eq!(scan.superblock.segments_reused, 0, "the ring didn't wrap");
    let mut all = pushed.clone();
    all.extend(more.iter().cloned());
    assert_same(&scanned(&scan), &all, "remount");
    let sessions: Vec<u32> = scan.data.iter().map(|s| s.session).collect();
    assert!(sessions.contains(&1) && sessions.contains(&2));
    assert!(sessions.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn drop_newest_refuses_when_full() {
    let mut c = cfg(4096, 1024);
    c.full = FullPolicy::DropNewest;
    let mut lr = mount(SimSd::new(size(&c, 6), 5), c);
    let mut g = Gen::new(5, 1000);
    let mut pushed = Vec::new();
    let err = workload(&mut lr, &mut g, 5000, &mut pushed).unwrap_err();
    assert_eq!(err, Error::Full);
    let mut medium = lr.into_medium();
    let scan = scan(&mut medium);
    let got = scanned(&scan);
    // everything stored before the refusal, except what was still in the stage when the segment couldn't open
    assert!(got.len() + 20 >= pushed.len() && got.len() <= pushed.len());
    assert_same(&got, &pushed[..got.len()], "drop newest");
}

#[test]
fn geometry_is_checked() {
    let c = cfg(4096, 1024);
    let r = block_on(Lr::mount(SimSd::new(4096 * 3, 1), c, info()));
    assert!(matches!(r, Err(Error::Geometry)), "too small");
    let mut bad = c;
    bad.segment_size = 4096 + 256;
    let r = block_on(Lr::mount(SimSd::new(size(&c, 8), 1), bad, info()));
    assert!(matches!(r, Err(Error::Geometry)), "not whole blocks");
    // a log laid out differently
    let lr = mount(SimSd::new(size(&c, 8), 1), c);
    let image = lr.into_medium().image();
    let mut other = c;
    other.meta_segments = 1;
    let r = block_on(Lr::mount(SimSd::from_image(image, 1), other, info()));
    assert!(matches!(r, Err(Error::Geometry)));
    // too many tracks for the writer
    let many = [TrackDesc::new("x"); TRACKS];
    let mut i = info();
    i.tracks = &many;
    let r = block_on(Lr::mount(SimSd::new(size(&c, 8), 1), c, i));
    assert!(matches!(r, Err(Error::Session)));
    // a record that can't fit a segment
    let mut lr = mount(SimSd::new(size(&c, 8), 1), c);
    let big = vec![0u8; 4096];
    assert_eq!(block_on(lr.push(1, &big, None)), Err(Error::TooBig));
    assert_eq!(block_on(lr.push(9, &[1], None)), Err(Error::Track));
}

#[test]
fn nor_round_trip_and_remount() {
    let cfg = cfg(4096, 1024);
    let mut lr = mount(SimNor::new(size(&cfg, 10), 1), cfg);
    let mut g = Gen::new(11, 1000);
    let mut pushed = Vec::new();
    workload(&mut lr, &mut g, 600, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    let image = lr.into_medium().image();
    let mut lr = mount(SimNor::from_image(image, 2), cfg);
    assert_eq!(lr.session(), 2);
    let mut more = Vec::new();
    workload(&mut lr, &mut g, 50, &mut more).unwrap();
    block_on(lr.sync()).unwrap();
    let mut medium = lr.into_medium();
    let got = scanned(&scan(&mut medium));
    let mut all = pushed;
    all.extend(more);
    assert_same(&got, &all[all.len() - got.len()..], "nor");
    assert!(got.len() > 100);
}

/// How many block reads a mount may take with `cfg`: the superblocks, then for the data and the meta ring the
/// hint and the one before it, a walk of at most `sb_every + 1` segments, a binary search over the segment's
/// blocks plus the probes past its end, the last block's flush offset, and the last flush's blocks (plus a few
/// for headers straddling blocks).
fn mount_read_bound(cfg: &Config, data_segments: u64) -> u64 {
    let blocks = (cfg.segment_size / BLOCK as u32) as u64;
    let sb_every = (data_segments / 4).clamp(1, 16);
    let log2 = 64 - blocks.leading_zeros() as u64;
    let flush_blocks = cfg.flush_bytes.div_ceil(BLOCK as u32) as u64 + 2;
    let per_ring = 2 + (sb_every + 1) + (log2 + 1) + 4 + 1 + flush_blocks + 4;
    2 + 2 * per_ring
}

/// Mount reads are bounded whatever the workload before: the superblocks, the hint and a short walk, a binary
/// search in the last segment and its last flush, for the data and the meta ring each.
#[test]
fn mount_reads_are_bounded() {
    let cfg = cfg(16384, 4096);
    let segments = 64;
    let bound = mount_read_bound(&cfg, segments);
    let mut lr = mount(Counting::new(SimSd::new(size(&cfg, segments), 9)), cfg);
    let mut g = Gen::new(9, 1500);
    let mut pushed = Vec::new();
    let mut worst = 0;
    for round in 0..6u64 {
        workload(&mut lr, &mut g, 1500, &mut pushed).unwrap();
        if round % 2 == 0 {
            block_on(lr.sync()).unwrap();
        }
        let image = lr.into_medium().inner.image();
        lr = mount(Counting::new(SimSd::from_image(image, round)), cfg);
        let rec = lr.recovery();
        let reads = lr.medium().reads;
        assert_eq!(
            rec.reads as u64, reads,
            "the recovery report counts every read"
        );
        assert!(
            reads <= bound,
            "round {round}: {reads} reads, bound {bound}: {rec:?}"
        );
        worst = worst.max(reads);
    }
    assert!(worst > 10);
    // many sessions without data in between: the walk stays short (the superblock is rewritten at every open)
    for round in 0..40u64 {
        let image = lr.into_medium().inner.image();
        lr = mount(Counting::new(SimSd::from_image(image, 100 + round)), cfg);
        assert!(lr.medium().reads <= bound);
    }
}

/// A power cut at every write unit of a workload, on SD (a cut block write happens or not) and on NOR (a cut
/// write leaves a half-programmed unit): nothing written before the cut is lost, torn blocks and records are
/// counted, positions continue after the last whole record, and logging goes on after the remount.
fn power_cut_everywhere<const W: usize, const E: usize, const NOR: bool>(
    segments: u64,
    n: usize,
    seed: u64,
) {
    type Sim<const W: usize, const E: usize, const NOR: bool> = SimMedium<W, E, NOR>;
    let cfg = cfg(4096, 1024);
    let size = size(&cfg, segments);
    let ops = {
        let mut lr = mount(Sim::<W, E, NOR>::new(size, seed), cfg);
        let mut g = Gen::new(seed, 1200);
        let mut pushed = Vec::new();
        workload(&mut lr, &mut g, n, &mut pushed).unwrap();
        block_on(lr.sync()).unwrap();
        lr.into_medium().ops()
    };
    let mut torn_blocks_seen = 0;
    let mut torn_records_seen = 0;
    let mut beyond_durable = 0;
    let mut rng = Rng::new(seed);
    for cut in 0..ops {
        let what = format!("cut {cut}");
        let mut medium = Sim::<W, E, NOR>::new(size, seed);
        medium.cut_power_at(cut);
        let mut g = Gen::new(seed, 1200);
        let mut pushed = Vec::new();
        let mut durable = [0u64; 4];
        let image = match block_on(Lr::mount(medium, cfg, info())) {
            Ok(mut lr) => {
                let res = workload(&mut lr, &mut g, n, &mut pushed);
                match res {
                    Ok(()) => block_on(lr.sync()).unwrap_or(()),
                    Err(Error::Medium(SimError::PowerCut)) => {}
                    Err(e) => panic!("{what}: {e:?}"),
                }
                for (t, d) in durable.iter_mut().enumerate() {
                    *d = lr.durable(t as u32).unwrap();
                }
                lr.into_medium().image()
            }
            Err(Error::Medium(SimError::PowerCut)) => {
                // cut while formatting or starting the session: the medium holds whatever was written
                let m = Sim::<W, E, NOR>::new(size, seed);
                let _ = m;
                continue;
            }
            Err(e) => panic!("{what}: {e:?}"),
        };
        // the sender saw these events stored; some of them weren't written yet when the power went
        let mut lr = mount(Sim::<W, E, NOR>::from_image(image, rng.next()), cfg);
        let rec = lr.recovery();
        assert!(!rec.formatted, "{what}: the log survived");
        assert!(rec.torn_blocks <= 1, "{what}: {rec:?}");
        assert!(rec.torn_records <= 1, "{what}: {rec:?}");
        torn_blocks_seen += rec.torn_blocks;
        torn_records_seen += rec.torn_records;
        let resumed: Vec<u64> = (0..4).map(|t| lr.track_state(t).unwrap().next).collect();
        let mut more = Vec::new();
        workload(&mut lr, &mut g, 20, &mut more).unwrap();
        block_on(lr.sync()).unwrap();
        let state: Vec<u64> = (0..4).map(|t| lr.track_state(t).unwrap().next).collect();
        let mut medium = lr.into_medium();
        let scan = scan(&mut medium);
        let got = scanned(&scan);
        // old records: a prefix of what was pushed (the ring is big enough not to wrap), covering the durable ones
        let old: Vec<&Ev> = got.iter().take(got.len() - more.len()).collect();
        assert!(old.len() <= pushed.len(), "{what}");
        for (i, ev) in old.iter().enumerate() {
            assert_eq!(**ev, pushed[i], "{what}: record {i}");
        }
        for t in 1..4u32 {
            let last_old = old
                .iter()
                .rev()
                .find(|e| e.track == t)
                .map(|e| e.head.end())
                .unwrap_or(0);
            assert!(
                last_old >= durable[t as usize],
                "{what}: track {t}: {last_old} < durable {}",
                durable[t as usize]
            );
            if last_old > durable[t as usize] {
                beyond_durable += 1;
            }
            // the new session continued where the old records end
            assert_eq!(resumed[t as usize], last_old, "{what}: track {t} resumed");
            let last = got
                .iter()
                .rev()
                .find(|e| e.track == t)
                .map(|e| e.head.end())
                .unwrap_or(0);
            assert_eq!(state[t as usize], last, "{what}: track {t}");
        }
        // the new records are all there, after the old ones
        assert_same(&got[got.len() - more.len()..], &more, &what);
        // no segment is torn except the one the cut hit, and the new session's are clean
        let torn_segments = scan
            .data
            .iter()
            .filter(|s| s.torn_block || s.torn_record)
            .count();
        assert!(torn_segments <= 1, "{what}");
        let last_session = scan.data.last().unwrap().session;
        assert!(
            scan.data
                .iter()
                .filter(|s| s.session == last_session)
                .all(|s| !s.torn_block && !s.torn_record),
            "{what}"
        );
    }
    assert!(torn_records_seen > 0, "some cut left a torn record");
    assert!(
        beyond_durable > 0,
        "some cut came after blocks beyond the last completed write were written"
    );
    if NOR {
        assert!(torn_blocks_seen > 0, "a cut NOR write leaves a torn block");
    } else {
        assert_eq!(torn_blocks_seen, 0, "an SD block write happens or not");
    }
}

#[test]
fn power_cut_sd() {
    power_cut_everywhere::<512, 512, false>(40, 250, 21);
}

#[test]
fn power_cut_nor() {
    power_cut_everywhere::<4, 256, true>(40, 250, 22);
}
