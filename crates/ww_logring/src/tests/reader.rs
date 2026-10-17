//! The indexed reader against the sequential scan: walks, seeks by track position and by time, summaries,
//! recordings, a ring read from an image file.

use super::writer::{BUF, Ev, Gen, TRACKS, cfg, mount, scan, size, us, workload};
use super::{Counting, Rng, block_on};
use crate::format::{Dec, Role};
use crate::image::ImageFile;
use crate::reader::{AllTracks, Reader, TrackSummary, Walker, time_ps};
use crate::scan::Scan;
use crate::system::SystemEvent;
use crate::writer::MAX_ITEM;

/// Tracks the reader tests decode (the logs they read have fewer).
const N: usize = 8;
use crate::{Logring, StreamHead};
use ww_stream::store::LogMedium;
use ww_stream::store::log::SimSd;

/// A data record as the scan saw it, with its track's stable key (0: the system track).
#[derive(Debug, Clone, PartialEq)]
struct Ref {
    seq: u64,
    session: u32,
    key: u64,
    head: StreamHead,
    item: Vec<u8>,
}

fn reference(scan: &Scan) -> Vec<Ref> {
    scan.records()
        .map(|(s, r)| Ref {
            seq: s.seq,
            session: s.session,
            key: if r.track == 0 {
                0
            } else {
                s.tracks
                    .iter()
                    .find(|t| t.number == r.track)
                    .map(|t| t.key)
                    .unwrap()
            },
            head: r.head,
            item: r.item.clone(),
        })
        .collect()
}

fn walk_all<M: LogMedium>(rd: &mut Reader, m: &mut M) -> Vec<Ref>
where
    M::Error: core::fmt::Debug,
{
    let oldest = rd.range(Role::Data).oldest;
    let mut w = Walker::at_segment(AllTracks::<N>::default(), Role::Data, oldest);
    let mut item = vec![0u8; MAX_ITEM];
    let mut out = Vec::new();
    while let Some(f) = block_on(w.next(rd, m, &mut item)).unwrap() {
        out.push(Ref {
            seq: f.seq,
            session: f.session,
            key: w.decode.key(f.track).unwrap(),
            head: f.head,
            item: item[..f.item_len].to_vec(),
        });
    }
    out
}

/// Three sessions of the random workload on a ring that went round.
fn three_sessions() -> SimSd {
    let cfg = cfg(4096, 1024);
    let mut lr = mount(SimSd::new(size(&cfg, 40), 11), cfg);
    let mut g = Gen::new(11, 1500);
    let mut pushed: Vec<Ev> = Vec::new();
    for _ in 0..3 {
        workload(&mut lr, &mut g, 500, &mut pushed).unwrap();
        block_on(lr.sync()).unwrap();
        let image = lr.into_medium().image();
        lr = mount(SimSd::from_image(image, 12), cfg);
    }
    block_on(lr.sync()).unwrap();
    assert!(lr.superblock().segments_reused > 0, "the ring went round");
    lr.into_medium()
}

#[test]
fn walk_matches_the_scan() {
    let mut m = three_sessions();
    let want = reference(&scan(&mut m));
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let got = walk_all(&mut rd, &mut m);
    assert_eq!(got.len(), want.len());
    assert!(got == want, "walk differs from the scan");
    assert_eq!(rd.torn(), 0);
}

#[test]
fn seek_track_finds_the_first_record_at_or_after() {
    let mut m = Counting::new(three_sessions());
    let want = reference(&scan(&mut m.inner));
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let keys: Vec<u64> = {
        let mut k: Vec<u64> = want.iter().map(|r| r.key).collect();
        k.sort();
        k.dedup();
        k
    };
    assert_eq!(keys.len(), 4, "the system track and three data tracks");
    let mut rng = Rng::new(5);
    let mut item = vec![0u8; MAX_ITEM];
    let mut max_reads = 0;
    for &key in &keys {
        let of_key: Vec<&Ref> = want.iter().filter(|r| r.key == key).collect();
        let lo = of_key[0].head.pos;
        let hi = of_key.last().unwrap().head.end();
        for i in 0..60 {
            let pos = match i {
                0 => 0,
                1 => lo,
                2 => hi - 1,
                3 => hi,
                _ => rng.range(lo, hi + 3),
            };
            let before = m.reads;
            let mut w = block_on(rd.seek_track(&mut m, key, 0, pos)).unwrap();
            max_reads = max_reads.max(m.reads - before);
            let mut got = Vec::new();
            for _ in 0..3 {
                match block_on(w.next(&mut rd, &mut m, &mut item)).unwrap() {
                    Some(f) => got.push((f.head, item[..f.item_len].to_vec())),
                    None => break,
                }
            }
            let exp: Vec<_> = of_key
                .iter()
                .filter(|r| r.head.pos >= pos)
                .take(3)
                .map(|r| (r.head, r.item.clone()))
                .collect();
            assert_eq!(got, exp, "key {key:x} at {pos}");
        }
    }
    // binary searches, not a scan: the ring has 40 segments of 8 blocks
    assert!(max_reads < 60, "a seek read {max_reads} blocks");
}

#[test]
fn summaries_match_the_scan() {
    let mut m = three_sessions();
    let s = scan(&mut m);
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let mut tracks = [TrackSummary::default(); N];
    for seg in &s.data {
        let sum = block_on(rd.summary::<N, _>(&mut m, seg.seq, &mut tracks))
            .unwrap()
            .unwrap();
        assert_eq!(sum.session, seg.session);
        assert_eq!(sum.records, seg.records.len() as u64);
        assert_eq!(sum.flushes, seg.flushes, "seq {}", seg.seq);
        assert_eq!(sum.tracks as usize, seg.tracks.len() + 1);
        let markers = seg
            .records
            .iter()
            .filter(|r| {
                r.track == 0
                    && matches!(
                        SystemEvent::decode(&mut Dec::new(&r.item)),
                        Some(SystemEvent::Marker { .. })
                    )
            })
            .count();
        assert_eq!(sum.markers as usize, markers);
        for (n, t) in tracks.iter().enumerate().take(sum.tracks as usize) {
            let recs: Vec<_> = seg.records.iter().filter(|r| r.track == n as u32).collect();
            assert_eq!(t.records as usize, recs.len());
            assert_eq!(t.first_pos, recs.first().map(|r| r.head.pos));
            assert_eq!(t.end_pos, recs.last().map(|r| r.head.end()));
            assert_eq!(
                t.item_bytes,
                recs.iter().map(|r| r.item.len() as u64).sum::<u64>()
            );
            if n > 0 {
                assert_eq!(t.key, seg.tracks[n - 1].key);
            }
        }
    }
}

#[test]
fn recordings_and_their_records() {
    let mut m = three_sessions();
    let want = reference(&scan(&mut m));
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let mut recs = Vec::new();
    block_on(rd.recordings(&mut m, |r| recs.push(r))).unwrap();
    // the small meta ring (2 segments) went round with the bundles: the newest recordings are left
    assert!(!recs.is_empty());
    let oldest = rd.range(Role::Data).oldest;
    let mut item = vec![0u8; MAX_ITEM];
    let mut checked = 0;
    for r in recs
        .iter()
        .filter(|r| r.start_seq >= oldest && r.stop.is_some())
    {
        let is = |x: &Ref, start: bool| {
            x.key == 0
                && match SystemEvent::decode(&mut Dec::new(&x.item)) {
                    Some(SystemEvent::RecordingStart { id, .. }) => start && id == r.id,
                    Some(SystemEvent::RecordingStop { id, .. }) => !start && id == r.id,
                    _ => false,
                }
        };
        let a = want.iter().position(|x| is(x, true)).unwrap();
        let b = want.iter().position(|x| is(x, false)).unwrap();
        let mut w = rd.recording::<N>(r);
        let mut got = Vec::new();
        while let Some(f) = block_on(w.next(&mut rd, &mut m, &mut item)).unwrap() {
            got.push((f.head, item[..f.item_len].to_vec()));
        }
        let exp: Vec<_> = want[a..=b]
            .iter()
            .map(|x| (x.head, x.item.clone()))
            .collect();
        assert_eq!(got, exp, "recording {}", r.id);
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn seek_time_starts_at_or_before() {
    // monotonic device time in µs on two tracks, three sessions
    let cfg = cfg(4096, 1024);
    let mut lr: Logring<'static, SimSd, BUF, TRACKS> = mount(SimSd::new(size(&cfg, 60), 21), cfg);
    let mut t = 0u32;
    for session in 0..3 {
        for k in 0..400u32 {
            t += 37 + k % 50;
            let item = vec![k as u8; (k % 90) as usize];
            block_on(lr.push(1 + k % 2, &item, Some(us(t)))).unwrap();
        }
        block_on(lr.sync()).unwrap();
        if session < 2 {
            let image = lr.into_medium().image();
            lr = mount(SimSd::from_image(image, 22), cfg);
        }
    }
    let mut m = lr.into_medium();
    let want = reference(&scan(&mut m));
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let mut item = vec![0u8; MAX_ITEM];
    let mut rng = Rng::new(9);
    for session in 1..=3u32 {
        let in_session: Vec<&Ref> = want
            .iter()
            .filter(|r| r.session == session && r.key != 0)
            .collect();
        let lo = time_ps(&in_session[0].head.t.unwrap()).unwrap() / 1_000_000;
        let hi = time_ps(&in_session.last().unwrap().head.t.unwrap()).unwrap() / 1_000_000;
        for _ in 0..30 {
            let target = rng.range(lo as u64, hi as u64) as u32;
            let mut w = block_on(rd.seek_time::<N, _>(&mut m, session, &us(target)))
                .unwrap()
                .unwrap();
            let mut skipped = 0;
            let mut hit = None;
            while let Some(f) = block_on(w.next(&mut rd, &mut m, &mut item)).unwrap() {
                if f.track == 0 {
                    continue;
                }
                let ft = time_ps(&f.head.t.unwrap()).unwrap() / 1_000_000;
                if ft >= target as i128 {
                    hit = Some(f.head);
                    break;
                }
                skipped += 1;
            }
            let exp = in_session
                .iter()
                .find(|r| time_ps(&r.head.t.unwrap()).unwrap() / 1_000_000 >= target as i128)
                .map(|r| r.head);
            assert_eq!(hit, exp, "session {session} at {target}");
            // found from the flush before it, not from the session's start
            assert!(skipped < 200, "{skipped} records before the target");
        }
    }
    assert!(
        block_on(rd.seek_time::<N, _>(&mut m, 9, &us(0)))
            .unwrap()
            .is_none()
    );
}

#[test]
fn reads_a_ring_from_an_image_file() {
    let mut m = three_sessions();
    let want = reference(&scan(&mut m));
    let path = std::env::temp_dir().join(format!("ww_logring-reader-{}.wlr", std::process::id()));
    std::fs::write(&path, m.image()).unwrap();
    let mut file = ImageFile::open(&path).unwrap();
    let mut rd = block_on(Reader::open(&mut file, None)).unwrap();
    let got = walk_all(&mut rd, &mut file);
    std::fs::remove_file(&path).unwrap();
    assert!(got == want, "image walk differs from the scan");
    assert!(block_on(file.write(0, &[0; 512])).is_err(), "read-only");
}

#[test]
fn time_cmp_orders_as_picoseconds_do() {
    use crate::Second;
    use crate::reader::time_cmp;
    use ww_si::Prefix;
    use ww_si::ww_numeric::NumericValue;
    let s = Second::from_parts;
    let mut rng = Rng::new(7);
    let prefixes = [
        Prefix::Unit,
        Prefix::Milli,
        Prefix::Micro,
        Prefix::Nano,
        Prefix::Pico,
    ];
    for _ in 0..20_000 {
        let pick = |rng: &mut Rng| {
            let p = prefixes[rng.range(0, prefixes.len() as u64 - 1) as usize];
            let v = match rng.range(0, 3) {
                0 => NumericValue::U32(rng.range(0, 2000) as u32),
                1 => NumericValue::U32(rng.next() as u32),
                2 => NumericValue::U64(rng.range(0, 1 << 40)),
                _ => NumericValue::U64(rng.next()),
            };
            s(p, v)
        };
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        if let (Some(x), Some(y)) = (time_ps(&a), time_ps(&b)) {
            assert_eq!(time_cmp(&a, &b), Some(x.cmp(&y)), "{a:?} {b:?}");
        }
    }
    let ms = s(Prefix::Milli, NumericValue::U32(1));
    assert_eq!(
        time_cmp(&ms, &s(Prefix::Micro, NumericValue::U32(1000))),
        Some(core::cmp::Ordering::Equal)
    );
    assert_eq!(
        time_cmp(&ms, &s(Prefix::Micro, NumericValue::U32(999))),
        Some(core::cmp::Ordering::Greater)
    );
    // u64::MAX seconds in picoseconds overflows u64: still the larger
    let big = s(Prefix::Unit, NumericValue::U64(u64::MAX));
    assert_eq!(
        time_cmp(&big, &s(Prefix::Pico, NumericValue::U64(u64::MAX))),
        Some(core::cmp::Ordering::Greater)
    );
}

/// A seek by position stays in the stream it is asked for (P2628#17oct-1711): after a reset the track's
/// positions restart at 0, and a seek for an older stream's position finds that stream's record, never a later
/// stream's; the walker stops at the stream's end.
#[test]
fn seek_track_stays_in_its_stream() {
    let cfg = cfg(4096, 1024);
    let mut lr = mount(SimSd::new(size(&cfg, 40), 51), cfg);
    let key = lr.track_key(1).unwrap();
    let item = |v: u32| {
        let mut b = vec![0u8; 100];
        b[..4].copy_from_slice(&v.to_le_bytes());
        b
    };
    let mut streams = Vec::new();
    for (k, (base, n)) in [(0u32, 300u64), (1000, 300), (2000, 100)]
        .into_iter()
        .enumerate()
    {
        if k > 0 {
            block_on(lr.reset_track(1, 4 + k as u32)).unwrap();
        }
        let start = lr.stream_session(1).unwrap().1;
        for i in 0..n as u32 {
            block_on(lr.push(1, &item(base + i), None)).unwrap();
            block_on(lr.push(2, &[7; 30], None)).unwrap();
        }
        block_on(lr.sync()).unwrap();
        streams.push((start, base, n));
    }
    assert!(streams.windows(2).all(|w| w[0].0 < w[1].0));
    let mut m = lr.into_medium();
    let mut rd = block_on(Reader::open(&mut m, None)).unwrap();
    let mut buf = vec![0u8; 200];
    for &(start, base, n) in &streams {
        for pos in [0, 1, 50, 99, 150, n - 1, n, n + 50] {
            let mut w = block_on(rd.seek_track(&mut m, key, start, pos)).unwrap();
            let mut got = Vec::new();
            while let Some(f) = block_on(w.next(&mut rd, &mut m, &mut buf)).unwrap() {
                got.push((f.head.pos, u32::from_le_bytes(buf[..4].try_into().unwrap())));
                assert!(
                    got.len() <= n as usize,
                    "stream {start} pos {pos}: walked past"
                );
            }
            let want: Vec<(u64, u32)> = (pos.min(n)..n).map(|p| (p, base + p as u32)).collect();
            assert_eq!(got, want, "stream {start}, pos {pos}");
        }
    }
}
