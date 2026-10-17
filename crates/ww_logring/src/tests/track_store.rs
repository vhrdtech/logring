//! TrackStore under `ww_stream`'s `Reliable` sender: a host that resumes after reboots and power cuts gets every
//! event that reached the medium exactly once, in order; acknowledgements, clock epochs, the ring going round,
//! stream resets.

use super::block_on;
use super::writer::{BUF, TRACKS, cfg, info, size};
use crate::{Config, Logring, TrackStore};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use wire_weaver::shrink_wrap::{UVlq32, UVlq64};
use ww_client_server::StreamSideband;
use ww_stream::store::StreamStore;
use ww_stream::store::log::SimSd;
use ww_stream::tx::Reliable;
use ww_stream::{Opened, Run, StreamCursor, StreamHead};

type Lr = Logring<'static, SimSd, BUF, TRACKS>;
type Shared = Mutex<NoopRawMutex, Lr>;
type Dev<'r> = Reliable<TrackStore<'r, 'static, NoopRawMutex, SimSd, BUF, TRACKS>, 8>;

/// The host's side, simplified: takes events of both runs in order and acknowledges what is contiguous.
#[derive(Default)]
struct Host {
    session: Option<u32>,
    /// Every event taken: position, value, the clock epoch in its head
    got: Vec<(u64, u32, Option<u32>)>,
    live_have: u64,
    live_from: u64,
    backfill_have: Option<u64>,
}

impl Host {
    fn opened(&mut self, o: Opened) {
        if self.session != Some(o.session) {
            self.got.clear();
        }
        self.session = Some(o.session);
        self.live_from = o.live_from;
        self.live_have = o.live_from;
        self.backfill_have = o.resume_from;
    }

    /// Where the host resumes: everything before it is here.
    fn cursor(&self) -> StreamCursor {
        let pos = match self.backfill_have {
            Some(b) if b < self.live_from => b,
            _ => self.live_have,
        };
        StreamCursor {
            session: UVlq32(self.session.unwrap_or(0)),
            pos: UVlq64(pos),
        }
    }

    fn take(&mut self, head: &StreamHead, item: &[u8], run: Run) {
        let have = match run {
            Run::Live => &mut self.live_have,
            Run::Backfill => self.backfill_have.as_mut().unwrap(),
        };
        assert_eq!(head.after_previous(), *have, "{run:?} in order: {head:?}");
        *have = head.end();
        let value = u32::from_le_bytes(item.try_into().unwrap());
        let epoch = head.ext.and_then(|e| e.clock_epoch).map(|e| e.0);
        self.got.push((head.pos, value, epoch));
    }

    /// Send what the device has and acknowledge it, until it has nothing more.
    fn drain(&mut self, dev: &mut Dev) {
        loop {
            let mut sent = false;
            for run in [Run::Backfill, Run::Live] {
                let mut buf = [0u8; 64];
                if let Some(e) = block_on(dev.next_event(run, &mut buf)).unwrap() {
                    self.take(&e.head, e.item, run);
                    sent = true;
                }
            }
            let ack = StreamSideband::Ack {
                have: UVlq64(self.live_have),
                backfill: self.backfill_have.map(UVlq64),
                grant: UVlq32(1 << 20),
            };
            block_on(dev.on_sideband(&ack, 0)).unwrap();
            if !sent {
                break;
            }
        }
    }
}

fn sync(lr: &Shared) {
    block_on(async { lr.lock().await.sync().await }).unwrap();
}

fn mount(m: SimSd, c: Config) -> Lr {
    block_on(Lr::mount(m, c, info())).unwrap()
}

/// A device that boots, logs whether a host is there or not, and loses power with records in its stage buffer;
/// a host that comes and goes. After every boot the host resumes from where it was.
#[test]
fn reliable_across_reboots_and_power_cuts() {
    let c = cfg(4096, 1024);
    let mut medium = SimSd::new(size(&c, 64), 31);
    let mut host = Host::default();
    let mut value = 0u32;
    // the values that reached the medium, in the order the positions give them
    let mut durable: Vec<u32> = Vec::new();
    for boot in 0..5u32 {
        let lr = Mutex::<NoopRawMutex, _>::new(mount(medium, c));
        {
            let store = block_on(TrackStore::new(&lr, 1)).unwrap();
            let mut dev: Dev = Reliable::new(store).initial_grant(1 << 20);
            dev.continue_stored_session();
            if dev.session().is_none() {
                block_on(dev.start_session(1)).unwrap();
            }
            assert_eq!(
                dev.next_pos(),
                durable.len() as u64,
                "boot {boot}: positions continue"
            );
            let mut pending = Vec::new();
            let mut push = |dev: &mut Dev, n: usize, pending: &mut Vec<u32>| {
                for _ in 0..n {
                    block_on(dev.push(&value.to_le_bytes(), 1, None)).unwrap();
                    pending.push(value);
                    value += 1;
                }
            };
            // logging before the host is there
            push(&mut dev, 40, &mut pending);
            sync(&lr);
            durable.append(&mut pending);
            // the host comes and gets the backlog and the live data
            let o = block_on(dev.resume(host.cursor())).unwrap();
            host.opened(o);
            for _ in 0..6 {
                push(&mut dev, 25, &mut pending);
                // nothing goes out before it is on the medium
                let mut buf = [0u8; 64];
                let before = host.got.len();
                host.drain(&mut dev);
                assert!(host.got.len() - before <= durable.len());
                assert!(
                    block_on(dev.next_event(Run::Live, &mut buf))
                        .unwrap()
                        .is_none()
                );
                sync(&lr);
                durable.append(&mut pending);
                host.drain(&mut dev);
            }
            // the host goes, the device logs on, then a power cut with 7 records not written
            dev.detach();
            push(&mut dev, 30, &mut pending);
            sync(&lr);
            durable.append(&mut pending);
            push(&mut dev, 7, &mut pending);
            let st = dev.store().state();
            assert!(st.persistent);
            assert_eq!(st.next, durable.len() as u64 + 7);
            assert_eq!(
                st.newest,
                Some(durable.len() as u64 - 1),
                "newest on the medium"
            );
        }
        medium = lr.into_inner().into_medium();
    }
    // the last boot's backlog
    let lr = Mutex::<NoopRawMutex, _>::new(mount(medium, c));
    let store = block_on(TrackStore::new(&lr, 1)).unwrap();
    let mut dev: Dev = Reliable::new(store).initial_grant(1 << 20);
    dev.continue_stored_session();
    let o = block_on(dev.resume(host.cursor())).unwrap();
    host.opened(o);
    host.drain(&mut dev);

    assert_eq!(
        host.session,
        Some(1),
        "one stream session across the reboots"
    );
    let mut got = host.got.clone();
    got.sort_by_key(|g| g.0);
    let positions: Vec<u64> = got.iter().map(|g| g.0).collect();
    assert_eq!(
        positions,
        (0..durable.len() as u64).collect::<Vec<_>>(),
        "every position once"
    );
    let values: Vec<u32> = got.iter().map(|g| g.1).collect();
    assert_eq!(values, durable, "every event that reached the medium");
    // the first event of every boot after the first carries the clock epoch (the logring session)
    let epochs: Vec<u32> = got.iter().filter_map(|g| g.2).collect();
    assert_eq!(epochs, vec![2, 3, 4, 5]);
    // acknowledged: nothing left to backfill
    assert_eq!(dev.store().state().oldest, None);
}

/// The host is gone while the ring goes round: the backfill starts at the oldest event kept, the rest counts as
/// dropped and arrives as a gap.
#[test]
fn ring_going_round_drops_unacknowledged() {
    let c = cfg(4096, 1024);
    let lr = Mutex::<NoopRawMutex, _>::new(mount(SimSd::new(size(&c, 8), 32), c));
    let store = block_on(TrackStore::new(&lr, 1)).unwrap();
    let mut dev: Dev = Reliable::new(store).initial_grant(1 << 20);
    block_on(dev.start_session(4)).unwrap();
    let mut host = Host::default();
    let o = block_on(dev.resume(host.cursor())).unwrap();
    host.opened(o);
    for i in 0..10u32 {
        block_on(dev.push(&i.to_le_bytes(), 1, None)).unwrap();
    }
    sync(&lr);
    host.drain(&mut dev);
    assert_eq!(host.got.len(), 10);
    dev.detach();
    // 8 segments of 8 blocks: a few thousand small records go round several times
    for i in 10..4000u32 {
        block_on(dev.push(&i.to_le_bytes(), 1, None)).unwrap();
    }
    sync(&lr);
    let st = dev.store().state();
    let oldest = st.oldest.unwrap();
    assert!(oldest > 10, "{st:?}");
    assert_eq!(st.dropped_units, oldest - 10);
    assert!(dev.stats().lost_store_full >= st.dropped_units);
    let o = block_on(dev.resume(host.cursor())).unwrap();
    assert_eq!(o.resume_from, Some(oldest));
    host.opened(o);
    host.backfill_have = Some(oldest);
    host.drain(&mut dev);
    // the live run starts with the newest, the backfill fills in from the oldest kept
    let mut values: Vec<u32> = host.got.iter().map(|g| g.1).collect();
    values.sort();
    let want: Vec<u32> = (0..10).chain(oldest as u32..4000).collect();
    assert_eq!(values, want);
}

/// A host that opens the stream afresh resets the track: positions from 0 in a new segment, also after a reboot;
/// the records of the session before stay in the ring but aren't read back.
#[test]
fn reset_starts_a_new_stream_session() {
    let c = cfg(4096, 1024);
    let lr = Mutex::<NoopRawMutex, _>::new(mount(SimSd::new(size(&c, 32), 33), c));
    let medium = {
        let store = block_on(TrackStore::new(&lr, 2)).unwrap();
        let mut dev: Dev = Reliable::new(store).initial_grant(1 << 20);
        block_on(dev.start_session(1)).unwrap();
        for i in 0..50u32 {
            block_on(dev.push(&i.to_le_bytes(), 1, None)).unwrap();
        }
        sync(&lr);
        let mut host = Host::default();
        host.opened(block_on(dev.open(9)).unwrap());
        assert_eq!(dev.store().state().session, 9);
        assert_eq!(dev.store().state().next, 0);
        let mut buf = [0u8; 64];
        assert!(
            block_on(dev.store_mut().read(0, &mut buf))
                .unwrap()
                .is_none(),
            "the old session's records aren't read"
        );
        for i in 100..120u32 {
            block_on(dev.push(&i.to_le_bytes(), 1, None)).unwrap();
        }
        sync(&lr);
        host.drain(&mut dev);
        let values: Vec<u32> = host.got.iter().map(|g| g.1).collect();
        assert_eq!(values, (100..120).collect::<Vec<_>>());
        lr.into_inner().into_medium()
    };
    // after a reboot: session 9, positions on from 20, the old records still not read
    let lr = Mutex::<NoopRawMutex, _>::new(mount(medium, c));
    let mut store = block_on(TrackStore::new(&lr, 2)).unwrap();
    let st = store.state();
    assert_eq!((st.session, st.next, st.newest), (9, 20, Some(19)));
    let mut buf = [0u8; 64];
    let e = block_on(store.read(0, &mut buf)).unwrap().unwrap();
    assert_eq!((e.head.pos, e.item), (0, &100u32.to_le_bytes()[..]));
    let e = block_on(store.read(19, &mut buf)).unwrap().unwrap();
    assert_eq!(e.item, &119u32.to_le_bytes()[..]);
    assert!(block_on(store.read(20, &mut buf)).unwrap().is_none());
}

/// After a reboot whose last flush held only other tracks, the store finds the track's newest record on the
/// medium itself.
#[test]
fn finds_the_newest_record_after_a_mount() {
    let c = cfg(4096, 1024);
    let mut lr = mount(SimSd::new(size(&c, 32), 34), c);
    for i in 0..300u32 {
        block_on(lr.push(1, &i.to_le_bytes(), None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    // a few segments of another track after it
    for i in 0..400u32 {
        block_on(lr.push(2, &[i as u8; 20], None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let lr = Mutex::<NoopRawMutex, _>::new(mount(lr.into_medium(), c));
    assert_eq!(block_on(async { lr.lock().await.durable_newest(1) }), None);
    let mut store = block_on(TrackStore::new(&lr, 1)).unwrap();
    let st = store.state();
    assert_eq!((st.next, st.newest, st.oldest), (300, Some(299), Some(0)));
    let mut buf = [0u8; 8];
    let e = block_on(store.read(299, &mut buf)).unwrap().unwrap();
    assert_eq!(e.item, &299u32.to_le_bytes()[..]);
    let e = block_on(store.read(17, &mut buf)).unwrap().unwrap();
    assert_eq!(e.item, &17u32.to_le_bytes()[..]);
    // a buffer too small is an error, and the next read still works
    assert!(matches!(
        block_on(store.read(18, &mut buf[..2])),
        Err(crate::Error::BufferTooSmall { item_len: 4 })
    ));
    let e = block_on(store.read(18, &mut buf)).unwrap().unwrap();
    assert_eq!(e.item, &18u32.to_le_bytes()[..]);
}
