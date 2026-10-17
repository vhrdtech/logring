//! Boots that don't recover positions, as a small build will mount (S3, [Logring::mount_small]): the session's
//! first segment carries `SEG_FRESH`, every track restarts at 0 (data tracks in a stream session with bit 31),
//! meta positions skip `META_SLACK`. Full and small boots alternate on one ring with a power cut at every write:
//! nothing written is lost, positions only grow within a stream, and the reader never mixes streams.

use super::writer::{BUF, Lr, TRACKS, cfg, info, meta_positions, scan, size};
use super::{Rng, block_on};
use crate::format::header::SEG_FRESH;
use crate::format::{Dec, META_SLACK};
use crate::reader::Reader;
use crate::scan::Scan;
use crate::system::SystemEvent;
use crate::{Buffers, Config, Error, Logring, STORE_SESSION};
use std::collections::{HashMap, HashSet};
use ww_stream::store::LogMedium;
use ww_stream::store::log::{SimError, SimMedium, SimSd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boot {
    Full,
    Small,
}
use Boot::{Full, Small};

/// A writer on `medium` mounted as `boot`; the mount's result beside it (a power cut leaves it unmounted).
fn boot<'a, M: LogMedium>(
    medium: M,
    bufs: &'a mut Buffers<BUF, TRACKS>,
    cfg: Config,
    boot: Boot,
) -> (Lr<'a, M>, Result<(), Error<M::Error>>)
where
    M::Error: core::fmt::Debug,
{
    let mut lr = Logring::new(medium, bufs, cfg, info()).unwrap();
    let r = match boot {
        Full => block_on(lr.mount()),
        Small => block_on(lr.mount_small()),
    };
    (lr, r)
}

/// An event pushed: its track, position and item (unique: the boot and its number in it).
type Pushed = (u32, u64, Vec<u8>);

/// `n` events on tracks 1..=3 with positions from the writer, markers, recordings, flushes and syncs.
fn work<M: LogMedium>(
    lr: &mut Lr<M>,
    rng: &mut Rng,
    boot_no: u8,
    n: u16,
    pushed: &mut Vec<Pushed>,
) -> Result<(), Error<M::Error>> {
    for k in 0..n {
        let t = rng.range(1, 3) as u32;
        let mut item = vec![boot_no, k as u8, (k >> 8) as u8];
        item.resize(rng.range(3, 300) as usize, t as u8);
        let pos = lr.track_state(t).unwrap().next;
        block_on(lr.push(t, &item, None))?;
        pushed.push((t, pos, item));
        if rng.chance(10) {
            block_on(lr.sync())?;
        }
        if rng.chance(3) {
            block_on(lr.marker(1, "m", Some(k as i64)))?;
        }
        if rng.chance(3) {
            if lr.recording().is_some() {
                block_on(lr.recording_stop(2))?;
            } else {
                block_on(lr.recording_start(1))?;
            }
        }
    }
    Ok(())
}

/// The SessionStart record that opens data segment `s`, if it does.
fn session_start(s: &crate::scan::SegmentScan) -> Option<(u32, bool)> {
    let r = s.records.iter().find(|r| r.track == 0)?;
    match SystemEvent::decode(&mut Dec::new(&r.item))? {
        SystemEvent::SessionStart {
            session,
            positions_restarted,
            ..
        } => Some((session, positions_restarted)),
        _ => None,
    }
}

/// What every ring must satisfy, whatever boots wrote it and wherever the power went: meta positions grow and the
/// superblock's `meta_next` is at most `META_SLACK` behind; a session's first segment carries `SEG_FRESH` exactly
/// when its `SessionStart` says positions weren't recovered, and then every data track is a new stream there in a
/// session with bit 31; within a stream (data: key and `stream_start`; system track: up to the next `SEG_FRESH`)
/// positions grow; a stream session with bit 31 names one stream of a key. Returns the data items on the ring.
fn check_ring(s: &Scan, what: &str) -> HashSet<Vec<u8>> {
    let meta = meta_positions(s);
    assert!(
        meta.windows(2).all(|w| w[0] < w[1]),
        "{what}: meta {meta:?}"
    );
    let real = meta.last().map_or(0, |p| p + 1);
    let sb = s.superblock.meta_next;
    assert!(
        sb <= real && real <= sb + META_SLACK,
        "{what}: meta_next {sb}, real {real}"
    );

    let mut items = HashSet::new();
    let mut last: HashMap<(u64, u64), u64> = HashMap::new();
    let mut store_sessions: HashMap<(u64, u32), u64> = HashMap::new();
    let mut system: Option<u64> = None;
    let mut prev_session = None;
    for seg in &s.data {
        let fresh = seg.flags & SEG_FRESH != 0;
        let first = prev_session != Some(seg.session);
        prev_session = Some(seg.session);
        if first {
            // a session's first segment opens with its SessionStart (unless the ring lost it to a cut)
            if let Some((session, restarted)) = session_start(seg) {
                assert_eq!(session, seg.session, "{what}");
                assert_eq!(
                    restarted, fresh,
                    "{what}: seq {} FRESH vs SessionStart",
                    seg.seq
                );
            }
        } else {
            assert!(
                !fresh,
                "{what}: FRESH on seq {}, not a session's first",
                seg.seq
            );
        }
        if fresh {
            system = None;
            for t in &seg.tracks {
                assert_eq!(t.stream_session, STORE_SESSION | seg.session, "{what}");
                assert_eq!(t.stream_start, seg.seq, "{what}");
            }
        }
        for t in &seg.tracks {
            if t.stream_session & STORE_SESSION != 0 {
                let start = *store_sessions
                    .entry((t.key, t.stream_session))
                    .or_insert(t.stream_start);
                assert_eq!(
                    start, t.stream_start,
                    "{what}: a bit-31 session names two streams"
                );
            }
        }
        for r in &seg.records {
            if r.track == 0 {
                assert!(
                    system.is_none_or(|p| r.head.pos > p),
                    "{what}: system track"
                );
                system = Some(r.head.pos);
                continue;
            }
            let t = seg.tracks.iter().find(|t| t.number == r.track).unwrap();
            let l = last.entry((t.key, t.stream_start)).or_insert(0);
            assert!(
                r.head.pos >= *l,
                "{what}: track {} seq {}",
                r.track,
                seg.seq
            );
            *l = r.head.end();
            items.insert(r.item.clone());
        }
    }
    items
}

/// A small boot: `SEG_FRESH`, positions from 0, stream sessions with bit 31, `SessionStart` flag, meta positions
/// past the slack; a full boot after it continues the small boot's streams.
#[test]
fn small_boot_restarts_positions() {
    let cfg = cfg(4096, 1024);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS>::new());
    let mut rng = Rng::new(61);
    let mut pushed = Vec::new();
    let (mut lr, r) = boot(SimSd::new(size(&cfg, 60), 61), &mut bufs, cfg, Full);
    r.unwrap();
    work(&mut lr, &mut rng, 0, 150, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    let full_next: Vec<u64> = (1..4).map(|t| lr.track_state(t).unwrap().next).collect();
    let sb = lr.superblock();
    let image = lr.into_medium().image();

    let (mut lr, r) = boot(SimSd::from_image(image, 62), &mut bufs, cfg, Small);
    r.unwrap();
    let rec = lr.recovery();
    assert!(rec.positions_restarted && !rec.formatted, "{rec:?}");
    assert_eq!(rec.torn_records, 0);
    let session = lr.session();
    assert_eq!(session, sb.next_session);
    // the SessionStart is the system track's position 0
    assert_eq!(lr.track_state(0).unwrap().next, 1);
    let mut small_start = 0;
    for t in 1..4 {
        assert_eq!(lr.track_state(t).unwrap().next, 0, "track {t}");
        let (ss, start) = lr.stream_session(t).unwrap();
        assert_eq!(ss, STORE_SESSION | session);
        small_start = start;
    }
    work(&mut lr, &mut rng, 1, 80, &mut pushed).unwrap();
    block_on(lr.sync()).unwrap();
    let small_next: Vec<u64> = (1..4).map(|t| lr.track_state(t).unwrap().next).collect();
    let image = lr.into_medium().image();

    let (lr, r) = boot(SimSd::from_image(image, 63), &mut bufs, cfg, Full);
    r.unwrap();
    assert!(!lr.recovery().positions_restarted);
    for t in 1..4 {
        assert_eq!(lr.track_state(t).unwrap().next, small_next[t as usize - 1]);
        assert_eq!(
            lr.stream_session(t).unwrap(),
            (STORE_SESSION | session, small_start)
        );
    }
    let mut medium = lr.into_medium();
    let s = scan(&mut medium);
    let items = check_ring(&s, "small boot");
    assert!(
        pushed.iter().all(|p| items.contains(&p.2)),
        "every record is there"
    );
    // the small session: one FRESH segment, its first, and meta positions skipped past the slack
    let fresh: Vec<u64> = s
        .data
        .iter()
        .filter(|d| d.flags & SEG_FRESH != 0)
        .map(|d| d.seq)
        .collect();
    assert_eq!(fresh, [small_start]);
    let first_of_small = s.data.iter().find(|d| d.session == session).unwrap();
    assert_eq!(first_of_small.seq, small_start);
    assert_eq!(session_start(first_of_small), Some((session, true)));
    let meta = meta_positions(&s);
    let i = meta.iter().position(|&p| p >= sb.meta_next).unwrap();
    assert_eq!(meta[i], sb.meta_next + META_SLACK, "{meta:?}");
    assert!(full_next.iter().all(|&n| n > 0));
}

/// Full and small boots in turn: seeks by position on the data tracks and on the system track find the record of
/// the stream asked for, as the scan has it, and the walker stops at the stream's end.
#[test]
fn reader_keeps_streams_apart_across_small_boots() {
    let cfg = cfg(4096, 1024);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS>::new());
    let mut rng = Rng::new(71);
    let mut medium = SimSd::new(size(&cfg, 80), 71);
    for (i, b) in [Full, Small, Full, Small, Small, Full]
        .into_iter()
        .enumerate()
    {
        let (mut lr, r) = boot(medium, &mut bufs, cfg, b);
        r.unwrap();
        work(&mut lr, &mut rng, i as u8, 120, &mut Vec::new()).unwrap();
        block_on(lr.sync()).unwrap();
        medium = lr.into_medium();
    }
    let s = scan(&mut medium);
    check_ring(&s, "reader");
    assert_eq!(s.superblock.segments_reused, 0, "the ring didn't go round");
    // every stream by (key, stream_start), the system track's by the FRESH segment it starts at
    let mut streams: HashMap<(u64, u64), Vec<(u64, Vec<u8>)>> = HashMap::new();
    let mut sys_start = 0;
    for seg in &s.data {
        if seg.flags & SEG_FRESH != 0 {
            sys_start = seg.seq;
        }
        for r in &seg.records {
            let id = if r.track == 0 {
                (0, sys_start)
            } else {
                let t = seg.tracks.iter().find(|t| t.number == r.track).unwrap();
                (t.key, t.stream_start)
            };
            streams
                .entry(id)
                .or_default()
                .push((r.head.pos, r.item.clone()));
        }
    }
    assert!(streams.keys().filter(|k| k.0 == 0).count() >= 3);
    let mut rd = block_on(Reader::open(&mut medium, None)).unwrap();
    let mut buf = vec![0u8; 1024];
    for ((key, start), recs) in &streams {
        let end = recs.last().unwrap().0 + 1;
        for pos in [
            0,
            1,
            recs.len() as u64 / 2,
            end - 1,
            end,
            rng.range(0, end + 3),
        ] {
            let mut w = block_on(rd.seek_track(&mut medium, *key, *start, pos)).unwrap();
            let mut got = Vec::new();
            while let Some(f) = block_on(w.next(&mut rd, &mut medium, &mut buf)).unwrap() {
                got.push((f.head.pos, buf[..f.item_len].to_vec()));
                assert!(
                    got.len() <= recs.len(),
                    "key {key} stream {start} pos {pos}: walked past"
                );
            }
            let want: Vec<_> = recs.iter().filter(|r| r.0 >= pos).cloned().collect();
            assert!(got == want, "key {key} stream {start} pos {pos}");
        }
    }
}

/// A power cut at every write unit of one boot (its mount, records, recordings, syncs), on a ring full and small
/// boots wrote before, then a boot of either kind: everything that was on the medium before the cut is still there
/// and every ring check holds ([check_ring]).
fn alternating_cuts<const W: usize, const E: usize, const NOR: bool>(
    before: &[Boot],
    cut_boot: Boot,
    after: Boot,
    seed: u64,
) {
    type Sim<const W: usize, const E: usize, const NOR: bool> = SimMedium<W, E, NOR>;
    let cfg = cfg(4096, 1024);
    let size = size(&cfg, 60);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS>::new());
    let mut rng = Rng::new(seed);
    let mut base_pushed = Vec::new();
    let mut medium = Sim::<W, E, NOR>::new(size, seed);
    for (i, &b) in before.iter().enumerate() {
        let (mut lr, r) = boot(medium, &mut bufs, cfg, b);
        r.unwrap();
        work(&mut lr, &mut rng, i as u8, 60, &mut base_pushed).unwrap();
        block_on(lr.sync()).unwrap();
        medium = lr.into_medium();
    }
    let base = medium.image();
    let boot_no = before.len() as u8;
    let ops = {
        let (mut lr, r) = boot(
            Sim::<W, E, NOR>::from_image(base.clone(), seed),
            &mut bufs,
            cfg,
            cut_boot,
        );
        r.unwrap();
        work(
            &mut lr,
            &mut Rng::new(seed + 1),
            boot_no,
            40,
            &mut Vec::new(),
        )
        .unwrap();
        block_on(lr.sync()).unwrap();
        lr.into_medium().ops()
    };
    for cut in 0..ops {
        let what = format!("{before:?} then {cut_boot:?} cut at {cut} then {after:?}");
        let mut m = Sim::<W, E, NOR>::from_image(base.clone(), cut);
        m.cut_power_at(cut);
        let (mut lr, r) = boot(m, &mut bufs, cfg, cut_boot);
        let mut pushed = Vec::new();
        let res = r.and_then(|()| {
            work(&mut lr, &mut Rng::new(seed + 1), boot_no, 40, &mut pushed)?;
            block_on(lr.sync())
        });
        match res {
            Ok(()) | Err(Error::Medium(SimError::PowerCut)) => {}
            Err(e) => panic!("{what}: {e:?}"),
        }
        let durable: Vec<u64> = (0..4).map(|t| lr.durable(t).unwrap_or(0)).collect();
        let image = lr.into_medium().image();
        let (mut lr, r) = boot(
            Sim::<W, E, NOR>::from_image(image, cut),
            &mut bufs,
            cfg,
            after,
        );
        r.unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let rec = lr.recovery();
        assert!(!rec.formatted, "{what}");
        assert_eq!(rec.positions_restarted, after == Small, "{what}");
        if after == Small {
            assert_eq!(rec.torn_records, 0, "{what}: not counted");
            assert!(
                (1..4).all(|t| lr.track_state(t).unwrap().next == 0),
                "{what}"
            );
        }
        let mut more = Vec::new();
        work(&mut lr, &mut Rng::new(seed + 2), boot_no + 1, 15, &mut more).unwrap();
        block_on(lr.sync()).unwrap();
        let mut medium = lr.into_medium();
        let s = scan(&mut medium);
        let items = check_ring(&s, &what);
        for p in base_pushed.iter().chain(&more) {
            assert!(
                items.contains(&p.2),
                "{what}: a record written before is gone"
            );
        }
        for p in &pushed {
            if p.1 < durable[p.0 as usize] {
                assert!(
                    items.contains(&p.2),
                    "{what}: a durable record of the cut boot is gone"
                );
            }
        }
    }
}

#[test]
fn power_cut_full_then_small_sd() {
    alternating_cuts::<512, 512, false>(&[Full], Full, Small, 81);
}

#[test]
fn power_cut_small_then_full_sd() {
    alternating_cuts::<512, 512, false>(&[Full], Small, Full, 82);
}

#[test]
fn power_cut_small_then_small_sd() {
    alternating_cuts::<512, 512, false>(&[Full, Small], Small, Small, 83);
}

#[test]
fn power_cut_full_after_small_sd() {
    alternating_cuts::<512, 512, false>(&[Small, Full, Small], Full, Full, 84);
}

#[test]
fn power_cut_small_and_full_nor() {
    alternating_cuts::<4, 256, true>(&[Full], Small, Full, 85);
    alternating_cuts::<4, 256, true>(&[Small], Full, Small, 86);
}
