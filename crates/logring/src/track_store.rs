//! [TrackStore]: one track of a [Logring] as `wire_weaver`'s [StreamStore], so a reliable stream
//! (`ww_stream::tx::Reliable<TrackStore>`) is kept on the card and a host that comes back later, after reboots
//! too, backfills from it.
//!
//! Several track stores and the firmware's own logging share one [Logring] through an `embassy-sync` [Mutex] (no
//! heap): each call locks it for one push, read or release, never while the link is busy.
//!
//! - **Positions** are the track's and continue across reboots ([Logring::mount] recovers them by the track's
//!   stable key). The stream session ([StreamStore::reset]) is kept per track in every segment head; a reset
//!   starts the positions at 0 in a new data segment, and the reader only looks at that segment and after it.
//! - **Only what is on the medium is read back** (and reported as `newest`): an event the sender could send from
//!   the stage buffer would be lost by a power cut while the host has it, and the positions after the reboot would
//!   be given to other events. So the live run follows the flushes (`flush_bytes`, `flush_interval`, `Immediate`
//!   tracks, [Logring::sync]); the latency of a live view is the flush interval.
//! - **Clock epochs**: the first event of a track after a mount of an existing log gets `ext.clock_epoch` (the
//!   logring's session) when the sender left it empty, so the host learns where the device's clock restarted.
//! - **Full**: the ring goes round (drop oldest) unless [Config::full](crate::Config) says otherwise; the units
//!   of this track's events that went with an overwritten segment before the host acknowledged them count in
//!   [StoreState::dropped_units]. [release](StreamStore::release) frees no room, it only moves the track's
//!   released position (kept in the segment heads).
//! - [state](StreamStore::state) is not async: it takes the lock when it is free and otherwise returns what it
//!   saw last.
//!
//! Only with the feature `tail-positions` (default on): without it every boot restarts the positions, so a store
//! couldn't be persistent, and it isn't built.

use crate::format::Role;
use crate::reader::{OneTrack, Range, ReadError, Reader, Walker};
use crate::writer::{Error, Logring};
use core::cell::Cell;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use wire_weaver::shrink_wrap::UVlq32;
use ww_stream::StreamHead;
use ww_stream::store::{LogMedium, PushError, StoreState, StoredEvent, StreamStore};

/// How far ahead of the last read a read may be to go on from there instead of seeking (units).
const REUSE_AHEAD: u64 = 1024;

/// One track of a shared [Logring] as a [StreamStore], see the [module docs](self).
pub struct TrackStore<'r, 'a, R: RawMutex, M: LogMedium, const BUF: usize, const TRACKS: usize> {
    lr: &'r Mutex<R, Logring<'a, M, BUF, TRACKS>>,
    track: u32,
    key: u64,
    rd: Reader,
    /// Where the last read left off: the walker, the position after the event it returned, the stream session
    hint: Option<(Walker<OneTrack>, u64, u32)>,
    /// The oldest data seq looked at for the track's oldest position, and that position (a lower bound)
    oldest_seq: u64,
    oldest_pos: u64,
    dropped: u64,
    last: Cell<StoreState>,
}

impl<E> From<ReadError<E>> for Error<E> {
    fn from(e: ReadError<E>) -> Self {
        match e {
            ReadError::Medium(e) => Error::Medium(e),
            ReadError::BufferTooSmall { item_len } => Error::BufferTooSmall { item_len },
            ReadError::NoSuperblock | ReadError::Geometry => Error::Geometry,
        }
    }
}

impl<'r, 'a, R: RawMutex, M: LogMedium, const BUF: usize, const TRACKS: usize>
    TrackStore<'r, 'a, R, M, BUF, TRACKS>
{
    /// Track `track` (1 and up) of the logring in `lr`. Finds the track's newest record on the medium when the
    /// mount didn't see it (a few block reads).
    pub async fn new(
        lr: &'r Mutex<R, Logring<'a, M, BUF, TRACKS>>,
        track: u32,
    ) -> Result<Self, Error<M::Error>> {
        let mut g = lr.lock().await;
        if track == 0 {
            return Err(Error::Track);
        }
        let key = g.track_key(track).ok_or(Error::Track)?;
        let geo = g.geo();
        let empty = Range {
            oldest: 0,
            tail: None,
        };
        let mut s = TrackStore {
            lr,
            track,
            key,
            rd: Reader::new(geo, empty, empty),
            hint: None,
            oldest_seq: 0,
            oldest_pos: 0,
            dropped: 0,
            last: Cell::new(StoreState::default()),
        };
        s.refresh(&mut g).await?;
        let (_, start) = g.stream_session(track).ok_or(Error::Track)?;
        let next = g.track_state(track).ok_or(Error::Track)?.next;
        if g.durable_newest(track).is_none() && next > 0 {
            // the newest record is in the last flush that starts before `next` (see Reader::seek_track)
            let mut w =
                s.rd.seek_track(g.medium_mut(), key, start, next - 1)
                    .await?;
            let seq = w.seq();
            w = w.until(seq);
            w.decode.min_pos = 0;
            w.truncate = true;
            let mut newest = None;
            let mut none = [0u8; 0];
            while let Some(f) = w.next(&mut s.rd, g.medium_mut(), &mut none).await? {
                newest = Some(f.head.pos);
            }
            if let Some(p) = newest {
                g.note_durable_newest(track, p);
            }
        }
        s.last.set(s.compute(&g));
        Ok(s)
    }

    /// The track's number in the logring.
    pub fn track(&self) -> u32 {
        self.track
    }

    /// The logring this store is a track of.
    pub fn logring(&self) -> &'r Mutex<R, Logring<'a, M, BUF, TRACKS>> {
        self.lr
    }

    /// The reader's view of the ring follows the writer; the track's oldest position follows the ring going round.
    async fn refresh(
        &mut self,
        g: &mut Logring<'a, M, BUF, TRACKS>,
    ) -> Result<(), Error<M::Error>> {
        let oldest = g.oldest_data_seq();
        self.rd.set_range(
            Role::Data,
            Range {
                oldest,
                tail: Some(g.data_seq()),
            },
        );
        let (_, start) = g.stream_session(self.track).ok_or(Error::Track)?;
        if oldest > start && oldest != self.oldest_seq {
            self.oldest_seq = oldest;
            if let Some(p) = self
                .rd
                .track_start(g.medium_mut(), oldest, self.key)
                .await?
            {
                let ts = g.track_state(self.track).ok_or(Error::Track)?;
                let p = p.min(ts.next);
                if p > self.oldest_pos {
                    // what went with the overwritten segments and was not acknowledged
                    self.dropped += p.saturating_sub(self.oldest_pos.max(ts.released));
                    self.oldest_pos = p;
                }
            }
        }
        Ok(())
    }

    fn compute(&self, g: &Logring<'a, M, BUF, TRACKS>) -> StoreState {
        let (session, _) = g.stream_session(self.track).unwrap_or_default();
        let ts = g.track_state(self.track).unwrap_or_default();
        let low = self.oldest_pos.max(ts.released);
        let geo = g.geo();
        let segments = g.data_seq() + 1 - g.oldest_data_seq();
        StoreState {
            session,
            oldest: (low < ts.next).then_some(low),
            newest: g.durable_newest(self.track).filter(|&n| n >= low),
            next: ts.next,
            used_bytes: segments * geo.seg,
            capacity_bytes: geo.data_n * geo.seg,
            persistent: true,
            dropped_units: self.dropped,
        }
    }
}

impl<'r, 'a, R: RawMutex, M: LogMedium, const BUF: usize, const TRACKS: usize> StreamStore
    for TrackStore<'r, 'a, R, M, BUF, TRACKS>
{
    type Error = Error<M::Error>;

    async fn push(&mut self, head: &StreamHead, item: &[u8]) -> Result<(), PushError<Self::Error>> {
        let mut g = self.lr.lock().await;
        let mut head = *head;
        let first = g.track_state(self.track).is_some_and(|t| t.records == 0);
        if first && !g.recovery().formatted && head.ext.is_none_or(|e| e.clock_epoch.is_none()) {
            let mut ext = head.ext.unwrap_or_default();
            ext.clock_epoch = Some(UVlq32(g.session()));
            head.ext = Some(ext);
        }
        let r = g.push_head(self.track, &head, item).await;
        let refreshed = self.refresh(&mut g).await;
        self.last.set(self.compute(&g));
        match r {
            Ok(()) => refreshed.map_err(PushError::Store),
            Err(Error::Full) => Err(PushError::Full),
            Err(Error::TooBig) => Err(PushError::TooBig),
            Err(e) => Err(PushError::Store(e)),
        }
    }

    async fn read<'b>(
        &mut self,
        pos: u64,
        buf: &'b mut [u8],
    ) -> Result<Option<StoredEvent<'b>>, Self::Error> {
        let mut g = self.lr.lock().await;
        self.refresh(&mut g).await?;
        let (session, start) = g.stream_session(self.track).ok_or(Error::Track)?;
        let pos = pos.max(self.oldest_pos);
        self.last.set(self.compute(&g));
        if g.durable_newest(self.track).is_none_or(|n| n < pos) {
            return Ok(None);
        }
        let mut w = match self.hint.take() {
            Some((mut w, after, s))
                if s == session && pos >= after && pos - after <= REUSE_AHEAD =>
            {
                w.decode.min_pos = pos;
                w
            }
            _ => {
                self.rd
                    .seek_track(g.medium_mut(), self.key, start, pos)
                    .await?
            }
        };
        let found = w.next(&mut self.rd, g.medium_mut(), buf).await;
        let found = match found {
            Ok(f) => f,
            Err(e) => {
                self.hint = Some((w, pos, session));
                return Err(e.into());
            }
        };
        match found {
            Some(f) => {
                self.hint = Some((w, f.head.end(), session));
                Ok(Some(StoredEvent {
                    head: f.head,
                    item: &buf[..f.item_len],
                }))
            }
            None => {
                self.hint = Some((w, pos, session));
                Ok(None)
            }
        }
    }

    async fn release(&mut self, pos: u64) -> Result<(), Self::Error> {
        let mut g = self.lr.lock().await;
        g.release(self.track, pos)?;
        self.last.set(self.compute(&g));
        Ok(())
    }

    fn state(&self) -> StoreState {
        match self.lr.try_lock() {
            Ok(g) => {
                let s = self.compute(&g);
                self.last.set(s);
                s
            }
            Err(_) => self.last.get(),
        }
    }

    async fn reset(&mut self, session: u32) -> Result<(), Self::Error> {
        let mut g = self.lr.lock().await;
        g.reset_track(self.track, session).await?;
        self.hint = None;
        self.oldest_pos = 0;
        self.oldest_seq = 0;
        self.dropped = 0;
        self.refresh(&mut g).await?;
        self.last.set(self.compute(&g));
        Ok(())
    }
}
