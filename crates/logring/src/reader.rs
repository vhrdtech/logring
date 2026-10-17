//! [Reader]: the indexed reader of a ring, `no_std` and without allocation. It reads the blocks of a [LogMedium]
//! through a one-block cache and finds records without reading everything:
//!
//! - **by track position**: a binary search over the data segments by every segment's first flush head (each
//!   track's next position at its start), then over the segment's blocks by their flush (`flush_off` in every
//!   trailer says where a block's flush starts, the flush head gives the positions there, a sparse one with its full head), then a scan of one
//!   flush: [Reader::seek_track];
//! - **by time**: the same over a session's segments and flushes by the device time in the flush heads
//!   ([Reader::seek_time]);
//! - **by recording**: the recordings table in the meta area ([Reader::recordings]), then the records between the
//!   recording's start and stop markers in the system track ([Reader::recording]);
//! - **segment summaries**: per track count, first and last position and time, markers, computed from one
//!   segment ([Reader::summary]).
//!
//! A [Walker] goes on from where a seek left it, record by record and segment by segment, decoding with a
//! [Decode]: [OneTrack] (one track by its stable key, the others stepped over without decoding them) or
//! [AllTracks]. At the end of what is written it stays where it is, so a reader of a ring that is still being
//! written picks up the next records once they reach the medium.
//!
//! Everything a medium holds is untrusted: every loop is bounded by the ring's geometry, malformed bytes end a
//! segment (counted in [Reader::torn]), nothing panics.

use crate::format::header::{
    FlushEntry, FlushHead, KIND_FLUSH, MAX_FLUSH_ENTRY, MAX_FLUSH_HEAD, MAX_SEG_HEAD,
    MAX_TRACK_ENTRY, SEG_FRESH, SegHead, TrackEntry,
};
use crate::format::record::{Decoded, MAX_REC_HEADER, Skipped, TrackCodec, decode_record_with};
use crate::format::{BLOCK, BlockCheck, Dec, NONE, PAYLOAD, Role, Superblock, check_block};
use crate::system::{MetaEvent, SystemEvent};
use crate::writer::Geo;
use ww_si::ww_numeric::NumericValue;
use ww_stream::store::LogMedium;
use ww_stream::{Second, StreamHead};

/// Bytes of a look-ahead buffer: the longest track entry (flush heads are read entry by entry).
const LOOK: usize = MAX_TRACK_ENTRY + 10;
const _: () = assert!(LOOK >= MAX_REC_HEADER && LOOK >= MAX_FLUSH_HEAD && LOOK >= MAX_SEG_HEAD);

/// Errors of a [Reader].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadError<E> {
    /// The medium failed.
    #[error("medium error")]
    Medium(E),
    /// Neither superblock slot holds a valid superblock: no ring on this medium.
    #[error("no superblock")]
    NoSuperblock,
    /// The superblock's geometry doesn't fit the medium.
    #[error("geometry")]
    Geometry,
    /// The item is bigger than the buffer given for it; the walker stays at the record.
    #[error("buffer too small for an item of {item_len} bytes")]
    BufferTooSmall { item_len: usize },
}

/// A place in the payload stream of one segment: the block and the offset in its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub block: u32,
    pub off: usize,
}

impl Place {
    const START: Place = Place { block: 0, off: 0 };

    /// Move on `n` bytes; a place past any segment (an absurd length read from a card) saturates.
    fn advance(&mut self, n: usize) {
        let total = self.off.saturating_add(n);
        let blocks = (total / PAYLOAD).min(u32::MAX as usize) as u32;
        self.block = self.block.saturating_add(blocks);
        self.off = total % PAYLOAD;
    }
}

/// The range of a ring's segments a reader looks at: oldest and newest seq, `None` before the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub oldest: u64,
    pub tail: Option<u64>,
}

impl Range {
    fn contains(&self, seq: u64) -> bool {
        self.tail.is_some_and(|t| seq >= self.oldest && seq <= t)
    }
}

/// The indexed reader, see the [module docs](self). Holds a one-block cache (about 0.6 KB, look-ahead buffers
/// live on the stack of a call); the medium is passed to every call, so a writer can own it between reads.
pub struct Reader {
    geo: Geo,
    data: Range,
    meta: Range,
    buf: [u8; BLOCK],
    /// The valid block in `buf`, and its flush offset
    cached: Option<(Role, u64, u32, u16)>,
    reads: u32,
    torn: u32,
}

/// What a segment head says, and where its first flush head starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegInfo {
    pub seq: u64,
    pub session: u32,
    /// The segment head's flag bits ([SEG_FRESH]).
    pub flags: u32,
    /// Data tracks in its table; track numbers `0..=tracks`.
    pub tracks: u32,
    first_flush: Place,
}

/// How a [Walker] decodes records: which tracks it decodes (the rest are stepped over), which records it returns.
pub trait Decode {
    /// A new segment starts; its track table follows through [entry](Self::entry).
    fn begin_segment(&mut self, seg: &SegInfo);
    /// A track table entry of the segment just begun.
    fn entry(&mut self, entry: &TrackEntry<'_>);
    /// Whether the segment holds anything for this decoder (after its table): `false` skips it.
    fn wants_segment(&self) -> bool;
    /// A flush head starts; `full` when it lists every track. Its entries follow through [flush](Self::flush). A
    /// track a sparse head doesn't list is where the last full head put it (it has no records since).
    fn begin_flush(&mut self, full: bool);
    /// An entry of a flush head: track `track`'s next position at the flush's start. A full head lists every
    /// track, a sparse one the tracks written since the full one, in increasing order.
    fn flush(&mut self, track: u32, next: u64);
    /// The codec of `track` when its records are decoded, `None` to step over them.
    fn codec(&mut self, track: u32) -> Option<&mut TrackCodec>;
    /// Whether a decoded record is returned (or stepped over).
    fn wants(&self, track: u32, head: &StreamHead) -> bool;
    /// After a segment head and its table: whether what the decoder follows ended before this segment (a later
    /// stream of its track starts at or before it). A [Walker] stops there for good.
    fn ended(&self) -> bool {
        false
    }
}

/// One track by its stable key (0: the system track), records at `min_pos` and after; with
/// [in_stream](Self::in_stream) only those of one stream of the track.
#[derive(Debug, Clone, Copy)]
pub struct OneTrack {
    pub key: u64,
    /// Records before this position are stepped over.
    pub min_pos: u64,
    /// Follow the stream current at this data seq only: a segment whose entry for the key has a later
    /// `stream_start` ends it, for the system track a later [SEG_FRESH] segment. `u64::MAX`: every stream.
    stream: u64,
    /// A segment of a later stream was seen
    ended: bool,
    /// The track's number in the current segment, `None` when it isn't in its table
    number: Option<u32>,
    codec: TrackCodec,
    /// The track's position at the last full flush head, and whether the head being read is full
    start: u64,
    full: bool,
    /// The entry of the track in the current segment's table
    entry: Option<EntryFacts>,
}

/// The fields of a track entry a reader keeps (no strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFacts {
    pub number: u32,
    pub key: u64,
    pub released: u64,
    pub stream_session: u32,
    pub stream_start: u64,
}

impl EntryFacts {
    fn of(e: &TrackEntry<'_>) -> Self {
        EntryFacts {
            number: e.number,
            key: e.key,
            released: e.released,
            stream_session: e.stream_session,
            stream_start: e.stream_start,
        }
    }
}

impl OneTrack {
    pub fn new(key: u64, min_pos: u64) -> Self {
        OneTrack {
            key,
            min_pos,
            stream: u64::MAX,
            ended: false,
            number: None,
            codec: TrackCodec::default(),
            start: 0,
            full: false,
            entry: None,
        }
    }

    /// Only the stream of the track that is current at data seq `seq` (its `stream_start` is at or before `seq`):
    /// a stream reset ([Logring::reset_track](crate::Logring::reset_track)) restarts the positions, so a later
    /// stream has the same positions again.
    pub fn in_stream(key: u64, min_pos: u64, seq: u64) -> Self {
        OneTrack {
            stream: seq,
            ..Self::new(key, min_pos)
        }
    }

    /// The track's entry in the current segment (`None` for the system track or before a segment).
    pub fn entry(&self) -> Option<EntryFacts> {
        self.entry
    }
}

impl Decode for OneTrack {
    fn begin_segment(&mut self, seg: &SegInfo) {
        self.number = (self.key == 0).then_some(0);
        self.entry = None;
        // the system track's positions restart in a FRESH segment
        if self.key == 0 && seg.flags & SEG_FRESH != 0 && seg.seq > self.stream {
            self.ended = true;
            self.number = None;
        }
    }

    fn entry(&mut self, e: &TrackEntry<'_>) {
        if self.key != 0 && e.key == self.key && self.number.is_none() {
            if e.stream_start > self.stream {
                self.ended = true;
                return;
            }
            self.number = Some(e.number);
            self.entry = Some(EntryFacts::of(e));
        }
    }

    fn wants_segment(&self) -> bool {
        self.number.is_some()
    }

    fn begin_flush(&mut self, full: bool) {
        self.full = full;
        self.codec.reset(self.start);
    }

    fn flush(&mut self, track: u32, next: u64) {
        if Some(track) == self.number {
            self.codec.reset(next);
            if self.full {
                self.start = next;
            }
        }
    }

    fn codec(&mut self, track: u32) -> Option<&mut TrackCodec> {
        (Some(track) == self.number).then_some(&mut self.codec)
    }

    fn wants(&self, _track: u32, head: &StreamHead) -> bool {
        head.pos >= self.min_pos
    }

    fn ended(&self) -> bool {
        self.ended
    }
}

/// Every track numbered below `N` (the ones above are stepped over); the keys of the current segment's tracks by
/// number.
#[derive(Debug, Clone)]
pub struct AllTracks<const N: usize> {
    codecs: [TrackCodec; N],
    keys: [u64; N],
    /// Every track's position at the last full flush head, and whether the head being read is full
    starts: [u64; N],
    full: bool,
    n: u32,
}

impl<const N: usize> Default for AllTracks<N> {
    fn default() -> Self {
        AllTracks {
            codecs: [TrackCodec::default(); N],
            keys: [0; N],
            starts: [0; N],
            full: false,
            n: 1,
        }
    }
}

impl<const N: usize> AllTracks<N> {
    /// The stable key of track `number` in the current segment (0 for the system track).
    pub fn key(&self, number: u32) -> Option<u64> {
        (number < self.n)
            .then(|| self.keys.get(number as usize).copied())
            .flatten()
    }
}

impl<const N: usize> Decode for AllTracks<N> {
    fn begin_segment(&mut self, seg: &SegInfo) {
        self.n = seg.tracks + 1;
        self.keys = [0; N];
    }

    fn entry(&mut self, e: &TrackEntry<'_>) {
        if let Some(k) = self.keys.get_mut(e.number as usize) {
            *k = e.key;
        }
    }

    fn wants_segment(&self) -> bool {
        true
    }

    fn begin_flush(&mut self, full: bool) {
        self.full = full;
        for (c, s) in self.codecs.iter_mut().zip(self.starts.iter()) {
            c.reset(*s);
        }
    }

    fn flush(&mut self, track: u32, next: u64) {
        if let Some(c) = self.codecs.get_mut(track as usize) {
            c.reset(next);
            if self.full {
                self.starts[track as usize] = next;
            }
        }
    }

    fn codec(&mut self, track: u32) -> Option<&mut TrackCodec> {
        if track < self.n {
            self.codecs.get_mut(track as usize)
        } else {
            None
        }
    }

    fn wants(&self, _track: u32, _head: &StreamHead) -> bool {
        true
    }
}

/// A record a [Walker] returned: where it is, its track and head, the item's length (its bytes are in the buffer
/// given to [Walker::next]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    pub role: Role,
    pub seq: u64,
    pub session: u32,
    /// Track number in its segment.
    pub track: u32,
    pub head: StreamHead,
    pub item_len: usize,
    /// Where the record starts.
    pub at: Place,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkState {
    /// At the start of segment `seq`
    SegStart,
    /// Inside segment `seq` at `place`
    InSeg,
    /// Past `last`
    Done,
}

/// Goes through the records of a ring from where it was put, see the [module docs](self).
#[derive(Debug, Clone)]
pub struct Walker<D> {
    pub decode: D,
    role: Role,
    seq: u64,
    /// The last seq to visit (inclusive), `None`: up to the tail
    last: Option<u64>,
    state: WalkState,
    session: u32,
    place: Place,
    /// First block of the flush being read
    flush_block: u32,
    /// Copy only what fits the buffer (the full length is in [Found::item_len]) instead of failing.
    pub truncate: bool,
}

impl<D: Decode> Walker<D> {
    /// From the start of segment `seq` of `role`.
    pub fn at_segment(decode: D, role: Role, seq: u64) -> Self {
        Walker {
            decode,
            role,
            seq,
            last: None,
            state: WalkState::SegStart,
            session: 0,
            place: Place::START,
            flush_block: 0,
            truncate: false,
        }
    }

    /// Stop after segment `last`.
    pub fn until(mut self, last: u64) -> Self {
        self.last = Some(last);
        self
    }

    /// The segment the walker is in (or starts next).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The session of the segment the walker is in.
    pub fn session(&self) -> u32 {
        self.session
    }

    fn next_segment(&mut self) {
        self.seq += 1;
        self.state = WalkState::SegStart;
        self.place = Place::START;
    }

    /// At the end of what segment `seq` holds: on to the next one, or stay (it is the tail, more may come).
    fn segment_ended(&mut self, range: &Range) -> bool {
        if range.tail.is_some_and(|t| self.seq < t) {
            self.next_segment();
            true
        } else {
            false
        }
    }

    /// The next record the decoder wants, its item copied into `item`. `None` at the end of what is written
    /// (call again later for what comes after) or past [until](Self::until).
    pub async fn next<M: LogMedium>(
        &mut self,
        rd: &mut Reader,
        m: &mut M,
        item: &mut [u8],
    ) -> Result<Option<Found>, ReadError<M::Error>> {
        // every step consumes bytes or a segment: bounded by the ring
        loop {
            let range = rd.range(self.role);
            if self.last.is_some_and(|l| self.seq > l) {
                self.state = WalkState::Done;
            }
            match self.state {
                WalkState::Done => return Ok(None),
                WalkState::SegStart => {
                    if range.tail.is_none_or(|t| self.seq > t) {
                        return Ok(None);
                    }
                    if self.seq < range.oldest {
                        self.seq = range.oldest;
                    }
                    let Some(seg) = rd.segment(m, self.role, self.seq, &mut self.decode).await?
                    else {
                        // block 0 not written yet (the tail) or damaged
                        if self.segment_ended(&range) {
                            continue;
                        }
                        return Ok(None);
                    };
                    if self.decode.ended() {
                        self.state = WalkState::Done;
                        return Ok(None);
                    }
                    if !self.decode.wants_segment() {
                        if self.segment_ended(&range) {
                            continue;
                        }
                        return Ok(None);
                    }
                    let mut place = seg.first_flush;
                    if !rd
                        .flush_head(m, self.role, self.seq, &mut place, false, &mut self.decode)
                        .await?
                    {
                        rd.torn += 1;
                        if self.segment_ended(&range) {
                            continue;
                        }
                        return Ok(None);
                    }
                    self.session = seg.session;
                    self.place = place;
                    self.flush_block = 0;
                    self.state = WalkState::InSeg;
                }
                WalkState::InSeg => {
                    if !range.contains(self.seq) {
                        // overwritten under us: go on from the oldest kept
                        self.next_segment();
                        continue;
                    }
                    if self.place.block >= rd.geo.blocks {
                        self.next_segment();
                        continue;
                    }
                    if self.place.off == 0 && self.place.block != self.flush_block {
                        match rd.load(m, self.role, self.seq, self.place.block).await? {
                            None => {
                                if self.segment_ended(&range) {
                                    continue;
                                }
                                return Ok(None);
                            }
                            Some(0) => {
                                let mut place = self.place;
                                if !rd
                                    .flush_head(
                                        m,
                                        self.role,
                                        self.seq,
                                        &mut place,
                                        true,
                                        &mut self.decode,
                                    )
                                    .await?
                                {
                                    if range.tail.is_some_and(|t| self.seq < t) {
                                        rd.torn += 1;
                                    }
                                    if self.segment_ended(&range) {
                                        continue;
                                    }
                                    return Ok(None);
                                }
                                self.flush_block = self.place.block;
                                self.place = place;
                                continue;
                            }
                            Some(_) => {}
                        }
                    }
                    let mut look = [0u8; MAX_REC_HEADER];
                    let n = rd
                        .peek(m, self.role, self.seq, self.place, &mut look)
                        .await?;
                    if n == 0 {
                        if self.segment_ended(&range) {
                            continue;
                        }
                        return Ok(None);
                    }
                    let mut dec = Dec::new(&look[..n]);
                    let Some(skipped) = skip_or_end(&mut dec) else {
                        if range.tail.is_some_and(|t| self.seq < t) || n == MAX_REC_HEADER {
                            rd.torn += 1;
                        }
                        if self.segment_ended(&range) {
                            continue;
                        }
                        return Ok(None);
                    };
                    let (track, header, item_len) = match skipped {
                        Skipped::Padding => {
                            self.place = Place {
                                block: self.place.block + 1,
                                off: 0,
                            };
                            continue;
                        }
                        Skipped::Record {
                            track,
                            header,
                            item_len,
                        } => (track, header, item_len),
                    };
                    let at = self.place;
                    let saved = self.decode.codec(track).map(|c| *c);
                    let decoded = match self.decode.codec(track) {
                        None => None,
                        Some(codec) => {
                            let mut dec = Dec::new(&look[..n]);
                            match decode_record_with(&mut dec, |_| Some(codec)) {
                                Some(Decoded::Record { head, .. }) => Some(head),
                                _ => {
                                    rd.torn += 1;
                                    if self.segment_ended(&range) {
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                        }
                    };
                    let mut after = at;
                    after.advance(header);
                    let want = decoded.filter(|h| self.decode.wants(track, h));
                    let restore = |w: &mut Self| {
                        if let (Some(s), Some(c)) = (saved, w.decode.codec(track)) {
                            *c = s;
                        }
                    };
                    match want {
                        None => {
                            // step over the item: its last byte has to be there (a record cut short ends the data)
                            if item_len > 0 {
                                let mut end = after;
                                end.advance(item_len - 1);
                                if end.block >= rd.geo.blocks
                                    || rd.load(m, self.role, self.seq, end.block).await?.is_none()
                                {
                                    restore(self);
                                    if self.segment_ended(&range) {
                                        continue;
                                    }
                                    return Ok(None);
                                }
                            }
                            after.advance(item_len);
                            self.place = after;
                        }
                        Some(head) => {
                            if item_len > item.len() && !self.truncate {
                                restore(self);
                                return Err(ReadError::BufferTooSmall { item_len });
                            }
                            let take = item_len.min(item.len());
                            let mut p = after;
                            let ok = rd
                                .read(m, self.role, self.seq, &mut p, &mut item[..take])
                                .await?
                                && (take == item_len || {
                                    let mut end = after;
                                    end.advance(item_len - 1);
                                    end.block < rd.geo.blocks
                                        && rd
                                            .load(m, self.role, self.seq, end.block)
                                            .await?
                                            .is_some()
                                });
                            if !ok {
                                restore(self);
                                if self.segment_ended(&range) {
                                    continue;
                                }
                                return Ok(None);
                            }
                            after.advance(item_len);
                            self.place = after;
                            return Ok(Some(Found {
                                role: self.role,
                                seq: self.seq,
                                session: self.session,
                                track,
                                head,
                                item_len,
                                at,
                            }));
                        }
                    }
                }
            }
        }
    }
}

/// [skip_record](crate::format::record::skip_record).
fn skip_or_end(dec: &mut Dec) -> Option<Skipped> {
    crate::format::record::skip_record(dec)
}

/// Order two device times: integers by their values when the prefixes are the same, else the one with the larger
/// prefix scaled up in u64 (an overflow means it is the larger), other numbers as `f64`. `None` for values that
/// aren't numbers (and NaN). No 128-bit arithmetic: it runs in every seek by time on the device.
pub fn time_cmp(a: &Second, b: &Second) -> Option<core::cmp::Ordering> {
    use core::cmp::Ordering;
    let int = |t: &Second| match t.value {
        NumericValue::U32(v) => Some(v as u64),
        NumericValue::U64(v) => Some(v),
        _ => None,
    };
    let (ea, eb): (i8, i8) = (a.prefix.into(), b.prefix.into());
    match (int(a), int(b)) {
        (Some(x), Some(y)) if ea == eb => Some(x.cmp(&y)),
        (Some(x), Some(y)) => {
            // `hi` has the larger exponent: hi * 10^d against lo
            let (hi, lo, swapped) = if ea > eb { (x, y, false) } else { (y, x, true) };
            let d = (ea as i32 - eb as i32).unsigned_abs();
            let o = match 10u64.checked_pow(d).and_then(|p| hi.checked_mul(p)) {
                Some(h) => h.cmp(&lo),
                None => Ordering::Greater,
            };
            Some(if swapped { o.reverse() } else { o })
        }
        _ => {
            let fa = a.value.to_f64() * libm_pow10(ea as i32);
            let fb = b.value.to_f64() * libm_pow10(eb as i32);
            fa.partial_cmp(&fb)
        }
    }
}

/// `t` is at or before `target`.
fn at_or_before(t: &Second, target: &Second) -> bool {
    time_cmp(t, target).is_some_and(|o| o != core::cmp::Ordering::Greater)
}

/// A device time as picoseconds, for ordering; `None` for values that aren't numbers. The PC side's form;
/// the device orders with [time_cmp].
pub fn time_ps(t: &Second) -> Option<i128> {
    let exp: i8 = t.prefix.into();
    let shift = exp as i32 + 12;
    let int = match t.value {
        NumericValue::U32(v) => Some(v as i128),
        NumericValue::U64(v) => Some(v as i128),
        _ => None,
    };
    match int {
        Some(v) if (0..=26).contains(&shift) => v.checked_mul(10i128.checked_pow(shift as u32)?),
        Some(v) if (-38..0).contains(&shift) => Some(v / 10i128.pow((-shift) as u32)),
        _ => {
            let f = t.value.to_f64() * libm_pow10(shift);
            f.is_finite().then_some(f as i128)
        }
    }
}

fn libm_pow10(e: i32) -> f64 {
    let mut r = 1.0f64;
    let (mut b, mut n) = (if e < 0 { 0.1 } else { 10.0 }, e.unsigned_abs());
    while n > 0 {
        if n & 1 == 1 {
            r *= b;
        }
        b *= b;
        n >>= 1;
    }
    r
}

/// Per-track facts of one segment, [Reader::summary].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackSummary {
    /// The track's stable key (0: the system track).
    pub key: u64,
    pub records: u64,
    pub item_bytes: u64,
    /// Position of the first record and the end of the last one's units.
    pub first_pos: Option<u64>,
    pub end_pos: Option<u64>,
    /// Device times of the first and the last record that carry one.
    pub first_t: Option<Second>,
    pub last_t: Option<Second>,
}

/// One segment summed up, [Reader::summary].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSummary {
    pub seq: u64,
    pub session: u32,
    /// Track numbers `0..tracks` are filled in the slice given to [Reader::summary].
    pub tracks: u32,
    pub records: u64,
    /// Markers (system track) in it.
    pub markers: u32,
    /// Flushes and the blocks they fill.
    pub flushes: u32,
    pub blocks: u32,
}

/// A recording from the meta area's recordings table, [Reader::recordings].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recording {
    pub id: u32,
    /// Trigger kind at the start.
    pub trigger: u8,
    pub session: u32,
    /// The data segment it started in, and the device time then.
    pub start_seq: u64,
    pub start_t: Option<Second>,
    /// Data segment, reason and time of the stop; `None` while running (or when the stop was lost).
    pub stop: Option<(u64, u8, Option<Second>)>,
}

impl Reader {
    /// A reader of a ring with geometry `geo` and these ranges (a mounted [Logring](crate::Logring) knows them).
    pub fn new(geo: Geo, data: Range, meta: Range) -> Self {
        Reader {
            geo,
            data,
            meta,
            buf: [0; BLOCK],
            cached: None,
            reads: 0,
            torn: 0,
        }
    }

    /// Open the ring on `medium` read-only: the superblock (slot B at `segment_size / 2`; `None`: the size from
    /// slot A, or the powers of two from 2 KB to 64 MB), then the tail of each ring from the superblock's hint, as
    /// a mount finds it. Writes nothing.
    pub async fn open<M: LogMedium>(
        m: &mut M,
        segment_size: Option<u32>,
    ) -> Result<Self, ReadError<M::Error>> {
        let sb = Self::superblock(m, segment_size).await?;
        let seg = sb.segment_size as u64;
        let blocks = seg / BLOCK as u64;
        let (meta_n, data_n) = (sb.meta_segments as u64, sb.data_segments as u64);
        if seg == 0
            || !seg.is_multiple_of(BLOCK as u64)
            || blocks < 4
            || blocks > u32::MAX as u64
            || meta_n == 0
            || data_n < 2
            || m.capacity() / seg < 1 + meta_n + data_n
        {
            return Err(ReadError::Geometry);
        }
        let geo = Geo {
            log_id: sb.log_id,
            seg,
            blocks: blocks as u32,
            meta_n,
            data_n,
        };
        let empty = Range {
            oldest: 0,
            tail: None,
        };
        let mut rd = Reader::new(geo, empty, empty);
        for role in [Role::Meta, Role::Data] {
            let hint = match role {
                Role::Meta => sb.meta_tail_seq,
                Role::Data => sb.data_tail_seq,
            };
            if hint == NONE {
                continue;
            }
            let tail = rd.find_tail(m, role, hint).await?;
            let range = Range {
                oldest: tail.map_or(0, |t| t.saturating_add(1).saturating_sub(geo.count(role))),
                tail,
            };
            match role {
                Role::Meta => rd.meta = range,
                Role::Data => rd.data = range,
            }
        }
        Ok(rd)
    }

    /// The newer of the two superblock slots.
    pub async fn superblock<M: LogMedium>(
        m: &mut M,
        segment_size: Option<u32>,
    ) -> Result<Superblock, ReadError<M::Error>> {
        let mut block = [0u8; BLOCK];
        let mut best: Option<Superblock> = None;
        m.read(0, &mut block).await.map_err(ReadError::Medium)?;
        if let Some(sb) = Superblock::decode(&block) {
            best = Some(sb);
        }
        // where slot B may be: half the segment size
        let mut sizes = [0u64; 16];
        let n = match (segment_size, best) {
            (Some(s), _) => {
                sizes[0] = s as u64;
                1
            }
            (None, Some(b)) => {
                sizes[0] = b.segment_size as u64;
                1
            }
            (None, None) => {
                for (i, s) in sizes.iter_mut().enumerate() {
                    *s = 1 << (11 + i);
                }
                16
            }
        };
        for &seg in &sizes[..n] {
            if seg / 2 + BLOCK as u64 > m.capacity() || seg < 2 * BLOCK as u64 {
                continue;
            }
            m.read(seg / 2, &mut block)
                .await
                .map_err(ReadError::Medium)?;
            if let Some(sb) = Superblock::decode(&block)
                && best.is_none_or(|b| sb.write_seq > b.write_seq)
            {
                best = Some(sb);
            }
        }
        best.ok_or(ReadError::NoSuperblock)
    }

    /// The ring's geometry.
    pub fn geo(&self) -> Geo {
        self.geo
    }

    /// The seqs of `role`'s ring this reader looks at.
    pub fn range(&self, role: Role) -> Range {
        match role {
            Role::Data => self.data,
            Role::Meta => self.meta,
        }
    }

    /// Set the range of `role` (a writer that went on writing).
    pub fn set_range(&mut self, role: Role, range: Range) {
        match role {
            Role::Data => self.data = range,
            Role::Meta => self.meta = range,
        }
    }

    /// Blocks read from the medium so far.
    pub fn reads(&self) -> u32 {
        self.reads
    }

    /// Malformed records and flush heads met inside the data (not at the end of what is written).
    pub fn torn(&self) -> u32 {
        self.torn
    }

    // -----------------------------------------------------------------------------------------------------------
    // Blocks and bytes

    /// Load block `index` of segment `seq`; its flush offset when it is valid, `None` when it isn't this segment's
    /// (not written yet, an earlier lap, damaged).
    async fn load<M: LogMedium>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        index: u32,
    ) -> Result<Option<u16>, ReadError<M::Error>> {
        if let Some((r, s, i, off)) = self.cached
            && (r, s, i) == (role, seq, index)
        {
            return Ok(Some(off));
        }
        if index >= self.geo.blocks {
            return Ok(None);
        }
        self.cached = None;
        let at = self.geo.base(role, seq) + index as u64 * BLOCK as u64;
        m.read(at, &mut self.buf).await.map_err(ReadError::Medium)?;
        self.reads += 1;
        match check_block(&self.buf, self.geo.log_id, role, seq, index) {
            BlockCheck::Ok { flush_off } => {
                self.cached = Some((role, seq, index, flush_off));
                Ok(Some(flush_off))
            }
            _ => Ok(None),
        }
    }

    /// Copy up to `out.len()` bytes at `place` without consuming them; fewer at the end of the valid blocks.
    async fn peek<M: LogMedium>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        mut place: Place,
        out: &mut [u8],
    ) -> Result<usize, ReadError<M::Error>> {
        let mut n = 0;
        while n < out.len() {
            if self.load(m, role, seq, place.block).await?.is_none() {
                break;
            }
            let take = (PAYLOAD - place.off).min(out.len() - n);
            out[n..n + take].copy_from_slice(&self.buf[place.off..place.off + take]);
            n += take;
            place.advance(take);
        }
        Ok(n)
    }

    /// Copy `out.len()` bytes at `place` and move it past them; `false` when the valid blocks end first.
    async fn read<M: LogMedium>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        place: &mut Place,
        out: &mut [u8],
    ) -> Result<bool, ReadError<M::Error>> {
        let mut done = 0;
        while done < out.len() {
            if self.load(m, role, seq, place.block).await?.is_none() {
                return Ok(false);
            }
            let take = (PAYLOAD - place.off).min(out.len() - done);
            out[done..done + take].copy_from_slice(&self.buf[place.off..place.off + take]);
            done += take;
            place.advance(take);
        }
        Ok(true)
    }

    // -----------------------------------------------------------------------------------------------------------
    // Heads

    /// The head and track table of segment `seq` (fed to `decode`), `None` when its block 0 isn't valid or the
    /// head is malformed.
    pub async fn segment<M: LogMedium, D: Decode>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        decode: &mut D,
    ) -> Result<Option<SegInfo>, ReadError<M::Error>> {
        let mut look = [0u8; LOOK];
        let n = self
            .peek(m, role, seq, Place::START, &mut look[..MAX_SEG_HEAD])
            .await?;
        let mut d = Dec::new(&look[..n]);
        let Some(sh) = SegHead::decode(&mut d) else {
            return Ok(None);
        };
        if sh.seq != seq {
            self.torn += 1;
            return Ok(None);
        }
        let mut place = Place::START;
        place.advance(d.pos());
        let mut info = SegInfo {
            seq,
            session: sh.session,
            flags: sh.flags,
            tracks: sh.tracks,
            first_flush: place,
        };
        decode.begin_segment(&info);
        for _ in 0..sh.tracks {
            let n = self
                .peek(m, role, seq, place, &mut look[..MAX_TRACK_ENTRY + 10])
                .await?;
            let mut d = Dec::new(&look[..n]);
            let Some(e) = TrackEntry::decode(&mut d) else {
                self.torn += 1;
                return Ok(None);
            };
            decode.entry(&e);
            place.advance(d.pos());
        }
        info.first_flush = place;
        Ok(Some(info))
    }

    /// Parse the flush head at `place` (with its kind byte when `plain`, the segment's first one has none) and
    /// move `place` past it; the positions go to `decode` only once the whole head checked out. `false` when it
    /// doesn't.
    async fn flush_head<M: LogMedium, D: Decode>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        place: &mut Place,
        plain: bool,
        decode: &mut D,
    ) -> Result<bool, ReadError<M::Error>> {
        let Some((fh, len)) = self
            .read_flush_head(m, role, seq, *place, plain, |_, _| true)
            .await?
        else {
            return Ok(false);
        };
        decode.begin_flush(fh.full_back == 0);
        self.read_flush_head(m, role, seq, *place, plain, |track, next| {
            decode.flush(track, next);
            true
        })
        .await?;
        place.advance(len);
        Ok(true)
    }

    /// Read the flush head at `place` (with its kind byte when `plain`), its entries one by one into `entry`
    /// (`false`: refuse the head): the head and its length in bytes. `None` when it is malformed or cut short.
    async fn read_flush_head<M: LogMedium>(
        &mut self,
        m: &mut M,
        role: Role,
        seq: u64,
        place: Place,
        plain: bool,
        mut entry: impl FnMut(u32, u64) -> bool,
    ) -> Result<Option<(FlushHead, usize)>, ReadError<M::Error>> {
        let mut b = [0u8; MAX_FLUSH_HEAD];
        let n = self.peek(m, role, seq, place, &mut b).await?;
        let mut d = Dec::new(&b[..n]);
        if plain && d.u8() != Some(KIND_FLUSH) {
            return Ok(None);
        }
        let Some(fh) = FlushHead::decode(&mut d) else {
            return Ok(None);
        };
        let mut len = d.pos();
        let mut prev = None;
        for _ in 0..fh.entries {
            let mut at = place;
            at.advance(len);
            let mut b = [0u8; MAX_FLUSH_ENTRY];
            let n = self.peek(m, role, seq, at, &mut b).await?;
            let mut d = Dec::new(&b[..n]);
            let Some(e) = FlushEntry::decode(&mut d, prev) else {
                return Ok(None);
            };
            if !entry(e.track, e.next) {
                return Ok(None);
            }
            prev = Some(e.track);
            len += d.pos();
        }
        Ok(Some((fh, len)))
    }

    /// The next position of `track` (number in segment `seq`) and the time at the flush that block `index`
    /// belongs to, and where that flush starts; `None` when the block isn't valid. `first` is where the first
    /// flush head of the segment starts (after the segment head and the track table). A track a sparse head
    /// doesn't list is where its full head put it.
    async fn flush_at<M: LogMedium>(
        &mut self,
        m: &mut M,
        seq: u64,
        index: u32,
        first: Place,
        track: u32,
    ) -> Result<Option<(u32, Option<u64>, Option<Second>)>, ReadError<M::Error>> {
        let Some(off) = self.load(m, Role::Data, seq, index).await? else {
            return Ok(None);
        };
        let start = index.saturating_sub(off as u32);
        let place = if start == 0 {
            first
        } else {
            Place {
                block: start,
                off: 0,
            }
        };
        let mut next = None;
        let Some((fh, _)) = self
            .read_flush_head(m, Role::Data, seq, place, start != 0, |tr, p| {
                if tr == track {
                    next = Some(p);
                }
                true
            })
            .await?
        else {
            return Ok(None);
        };
        if next.is_none() && fh.full_back != 0 {
            let Some(at) = start.checked_sub(fh.full_back) else {
                return Ok(None);
            };
            let place = if at == 0 {
                first
            } else {
                Place { block: at, off: 0 }
            };
            self.read_flush_head(m, Role::Data, seq, place, at != 0, |tr, p| {
                if tr == track {
                    next = Some(p);
                }
                true
            })
            .await?;
        }
        Ok(Some((start, next, fh.t)))
    }

    /// The last seq of `role` with a valid block 0, from the superblock's hint (as a mount finds it).
    async fn find_tail<M: LogMedium>(
        &mut self,
        m: &mut M,
        role: Role,
        hint: u64,
    ) -> Result<Option<u64>, ReadError<M::Error>> {
        let mut tail = None;
        for cand in [Some(hint), hint.checked_sub(1)].into_iter().flatten() {
            if self.load(m, role, cand, 0).await?.is_some() {
                tail = Some(cand);
                break;
            }
        }
        let Some(mut seq) = tail else {
            return Ok(None);
        };
        if seq != hint {
            return Ok(Some(seq));
        }
        let n = self.geo.count(role);
        for _ in 0..n {
            let Some(next) = seq.checked_add(1) else {
                break;
            };
            if self.load(m, role, next, 0).await?.is_none() {
                break;
            }
            seq = next;
        }
        Ok(Some(seq))
    }

    // -----------------------------------------------------------------------------------------------------------
    // Seeks

    /// A walker on the track with stable `key` (0: the system track) at its first record at or after `pos` in the
    /// track's stream that is current at data seq `from_seq` (its `stream_start`, normally: positions grow from
    /// there), looking at segments `from_seq` and later; the walker stops at the stream's end. Binary searches over
    /// segments and flushes, see the [module docs](self).
    pub async fn seek_track<M: LogMedium>(
        &mut self,
        m: &mut M,
        key: u64,
        from_seq: u64,
        pos: u64,
    ) -> Result<Walker<OneTrack>, ReadError<M::Error>> {
        let range = self.data;
        let decode = OneTrack::in_stream(key, pos, from_seq);
        let lo = from_seq.max(range.oldest);
        let Some(mut hi) = range.tail.filter(|&t| t >= lo) else {
            return Ok(Walker::at_segment(decode, Role::Data, lo));
        };
        if key == 0 {
            // the system track has no stream_start: its stream ends before the next FRESH segment, which only
            // that segment's own head says
            for seq in lo + 1..=hi {
                if self
                    .segment(m, Role::Data, seq, &mut NoDecode)
                    .await?
                    .is_some_and(|s| s.flags & SEG_FRESH != 0)
                {
                    hi = seq - 1;
                    break;
                }
            }
        }
        // the last segment whose start position of the track is at or before `pos`; a segment without the track
        // (or not readable) counts as after it, which only makes the scan start earlier; a segment of a later
        // stream is after it
        let mut best = lo;
        let (mut a, mut b) = (lo, hi + 1);
        while a < b {
            let mid = a + (b - a) / 2;
            match self.segment_start(m, mid, decode).await? {
                Some((_, _, Some(start))) if start <= pos => {
                    best = mid;
                    a = mid + 1;
                }
                _ => b = mid,
            }
        }
        let Some((seg, number, Some(start))) = self.segment_start(m, best, decode).await? else {
            return Ok(Walker::at_segment(decode, Role::Data, best));
        };
        if start > pos {
            return Ok(Walker::at_segment(decode, Role::Data, best));
        }
        // the last block whose flush starts at or before `pos` for the track
        let (mut lo_b, mut hi_b) = (0u32, self.geo.blocks);
        while hi_b - lo_b > 1 {
            let mid = lo_b + (hi_b - lo_b) / 2;
            match self.flush_at(m, best, mid, seg.first_flush, number).await? {
                Some((_, Some(next), _)) if next <= pos => lo_b = mid,
                Some((fstart, _, _)) if fstart > lo_b => hi_b = fstart,
                _ => hi_b = mid,
            }
        }
        let fstart = match self
            .flush_at(m, best, lo_b, seg.first_flush, number)
            .await?
        {
            Some((s, _, _)) => s,
            None => 0,
        };
        Ok(self
            .walker_at_flush(m, decode, best, fstart)
            .await?
            .unwrap_or_else(|| Walker::at_segment(decode, Role::Data, best)))
    }

    /// A walker over every track at the first flush of `session` whose device time is at or before `t` (the
    /// records from there on are at `t` or a little before; times are compared as numbers, a clock that wraps
    /// inside the session breaks the order). Segments of `from..=to` are searched; `None` when none is of the
    /// session.
    pub async fn seek_time<const N: usize, M: LogMedium>(
        &mut self,
        m: &mut M,
        session: u32,
        t: &Second,
    ) -> Result<Option<Walker<AllTracks<N>>>, ReadError<M::Error>> {
        let Some(tail) = self.data.tail else {
            return Ok(None);
        };
        // the session's segments are consecutive; sessions grow with seq
        let (mut a, mut b) = (self.data.oldest, tail + 1);
        while a < b {
            let mid = a + (b - a) / 2;
            match self.segment(m, Role::Data, mid, &mut NoDecode).await? {
                Some(s) if s.session < session => a = mid + 1,
                _ => b = mid,
            }
        }
        let first = a;
        match self.segment(m, Role::Data, first, &mut NoDecode).await? {
            Some(s) if s.session == session => {}
            _ => return Ok(None),
        }
        // the last segment of the session whose first flush is at or before `t`
        let mut best = first;
        let (mut a, mut b) = (first, tail + 1);
        while a < b {
            let mid = a + (b - a) / 2;
            let Some(s) = self.segment(m, Role::Data, mid, &mut NoDecode).await? else {
                b = mid;
                continue;
            };
            if s.session != session {
                b = mid;
                continue;
            }
            let at = self
                .flush_at(m, mid, 0, s.first_flush, 0)
                .await?
                .and_then(|(_, _, ft)| ft);
            if at.is_some_and(|x| at_or_before(&x, t)) {
                best = mid;
                a = mid + 1;
            } else {
                b = mid;
            }
        }
        let Some(seg) = self.segment(m, Role::Data, best, &mut NoDecode).await? else {
            return Ok(None);
        };
        let (mut lo_b, mut hi_b) = (0u32, self.geo.blocks);
        while hi_b - lo_b > 1 {
            let mid = lo_b + (hi_b - lo_b) / 2;
            match self.flush_at(m, best, mid, seg.first_flush, 0).await? {
                Some((_, _, Some(ft))) if at_or_before(&ft, t) => lo_b = mid,
                Some((fstart, _, _)) if fstart > lo_b => hi_b = fstart,
                _ => hi_b = mid,
            }
        }
        let fstart = match self.flush_at(m, best, lo_b, seg.first_flush, 0).await? {
            Some((s, _, _)) => s,
            None => 0,
        };
        Ok(Some(
            self.walker_at_flush(m, AllTracks::default(), best, fstart)
                .await?
                .unwrap_or_else(|| Walker::at_segment(AllTracks::default(), Role::Data, best)),
        ))
    }

    /// The next position of the track with stable `key` at the start of data segment `seq`: every record of it
    /// in that segment and after is at or after it. `None` when the segment isn't readable or doesn't name the
    /// track.
    pub async fn track_start<M: LogMedium>(
        &mut self,
        m: &mut M,
        seq: u64,
        key: u64,
    ) -> Result<Option<u64>, ReadError<M::Error>> {
        Ok(self
            .segment_start(m, seq, OneTrack::new(key, 0))
            .await?
            .and_then(|(_, _, p)| p))
    }

    /// The segment head of data segment `seq`, the number in it of the track `one` follows and its next position
    /// at the segment's start (`None` when the track isn't in its table, or the segment is of a later stream).
    async fn segment_start<M: LogMedium>(
        &mut self,
        m: &mut M,
        seq: u64,
        mut one: OneTrack,
    ) -> Result<Option<(SegInfo, u32, Option<u64>)>, ReadError<M::Error>> {
        let Some(seg) = self.segment(m, Role::Data, seq, &mut one).await? else {
            return Ok(None);
        };
        let Some(number) = one.number else {
            return Ok(Some((seg, 0, None)));
        };
        let next = match self.flush_at(m, seq, 0, seg.first_flush, number).await? {
            Some((_, next, _)) => next,
            None => None,
        };
        Ok(Some((seg, number, next)))
    }

    /// A walker in data segment `seq` at the flush starting at block `block` (0: the segment's first flush).
    async fn walker_at_flush<M: LogMedium, D: Decode>(
        &mut self,
        m: &mut M,
        mut decode: D,
        seq: u64,
        block: u32,
    ) -> Result<Option<Walker<D>>, ReadError<M::Error>> {
        let Some(seg) = self.segment(m, Role::Data, seq, &mut decode).await? else {
            return Ok(None);
        };
        let mut w = Walker::at_segment(decode, Role::Data, seq);
        if block == 0 {
            return Ok(Some(w));
        }
        let mut place = Place { block, off: 0 };
        // a sparse head: first the positions of the tracks it doesn't list, from its full head
        let Some((fh, _)) = self
            .read_flush_head(m, Role::Data, seq, place, true, |_, _| true)
            .await?
        else {
            return Ok(None);
        };
        if fh.full_back != 0 {
            let Some(at) = block.checked_sub(fh.full_back) else {
                return Ok(None);
            };
            let mut full = if at == 0 {
                seg.first_flush
            } else {
                Place { block: at, off: 0 }
            };
            if !self
                .flush_head(m, Role::Data, seq, &mut full, at != 0, &mut w.decode)
                .await?
            {
                return Ok(None);
            }
        }
        if !self
            .flush_head(m, Role::Data, seq, &mut place, true, &mut w.decode)
            .await?
        {
            return Ok(None);
        }
        w.session = seg.session;
        w.place = place;
        w.flush_block = block;
        w.state = WalkState::InSeg;
        Ok(Some(w))
    }

    // -----------------------------------------------------------------------------------------------------------
    // Summaries and recordings

    /// Sum up data segment `seq`: per track (by number, into `tracks`, tracks numbered `N` and up are stepped over) its records,
    /// positions and times, and the markers. `None` when its block 0 isn't valid.
    pub async fn summary<const N: usize, M: LogMedium>(
        &mut self,
        m: &mut M,
        seq: u64,
        tracks: &mut [TrackSummary],
    ) -> Result<Option<SegmentSummary>, ReadError<M::Error>> {
        let mut all = AllTracks::<N>::default();
        let Some(seg) = self.segment(m, Role::Data, seq, &mut all).await? else {
            return Ok(None);
        };
        let n = (seg.tracks as usize + 1).min(tracks.len());
        for (i, t) in tracks.iter_mut().enumerate().take(n) {
            *t = TrackSummary {
                key: all.key(i as u32).unwrap_or(0),
                ..TrackSummary::default()
            };
        }
        let mut sum = SegmentSummary {
            seq,
            session: seg.session,
            tracks: n as u32,
            records: 0,
            markers: 0,
            flushes: 0,
            blocks: 0,
        };
        let mut w = Walker::at_segment(all, Role::Data, seq).until(seq);
        w.truncate = true;
        let mut item = [0u8; 4];
        let mut last_block = 0;
        while let Some(f) = w.next(self, m, &mut item).await? {
            sum.records += 1;
            last_block = last_block.max(f.at.block);
            if f.track == 0 && item[0] == 2 {
                sum.markers += 1;
            }
            if let Some(t) = tracks
                .get_mut(f.track as usize)
                .filter(|_| (f.track as usize) < n)
            {
                t.records += 1;
                t.item_bytes += f.item_len as u64;
                t.first_pos.get_or_insert(f.head.pos);
                t.end_pos = Some(f.head.end());
                if f.head.t.is_some() {
                    if t.first_t.is_none() {
                        t.first_t = f.head.t;
                    }
                    t.last_t = f.head.t;
                }
            }
        }
        // the flushes: every block whose flush offset is 0, up to the end of what is written
        let mut b = 0;
        while b < self.geo.blocks {
            match self.load(m, Role::Data, seq, b).await? {
                Some(0) => sum.flushes += 1,
                Some(_) => {}
                None => break,
            }
            b += 1;
        }
        sum.blocks = b.max(last_block + 1);
        Ok(Some(sum))
    }

    /// Every recording in the meta area's table, oldest first, to `f` (a start and its stop make one recording).
    /// Recordings whose start went with a reused meta segment are not seen.
    pub async fn recordings<M: LogMedium>(
        &mut self,
        m: &mut M,
        mut f: impl FnMut(Recording),
    ) -> Result<(), ReadError<M::Error>> {
        let Some(tail) = self.meta.tail else {
            return Ok(());
        };
        let mut w =
            Walker::at_segment(OneTrack::new(0, 0), Role::Meta, self.meta.oldest).until(tail);
        w.truncate = true;
        let mut item = [0u8; 256];
        // a start waits for its stop (at most one recording runs at a time)
        let mut open: Option<Recording> = None;
        while let Some(found) = w.next(self, m, &mut item).await? {
            let take = found.item_len.min(item.len());
            let mut d = Dec::new(&item[..take]);
            let Some(MetaEvent::Recording {
                id,
                stopped,
                why,
                session,
                seq,
                t,
            }) = MetaEvent::decode(&mut d)
            else {
                continue;
            };
            if !stopped {
                if let Some(r) = open.take() {
                    f(r);
                }
                open = Some(Recording {
                    id,
                    trigger: why,
                    session,
                    start_seq: seq,
                    start_t: t,
                    stop: None,
                });
            } else if let Some(mut r) = open.take() {
                if r.id == id {
                    r.stop = Some((seq, why, t));
                }
                f(r);
            }
        }
        if let Some(r) = open {
            f(r);
        }
        Ok(())
    }

    /// A walker over the records of recording `rec`: every track from its `RecordingStart` record in the system
    /// track on, up to its `RecordingStop` (see [RecordingWalker]).
    pub fn recording<const N: usize>(&self, rec: &Recording) -> RecordingWalker<N> {
        let mut w = Walker::at_segment(AllTracks::default(), Role::Data, rec.start_seq);
        if let Some((stop, _, _)) = rec.stop {
            w = w.until(stop);
        }
        RecordingWalker {
            walker: w,
            id: rec.id,
            inside: false,
            done: false,
        }
    }
}

/// The records of one recording, [Reader::recording]: those after its `RecordingStart` and before its
/// `RecordingStop` in the system track (both markers are returned too).
pub struct RecordingWalker<const N: usize> {
    pub walker: Walker<AllTracks<N>>,
    id: u32,
    inside: bool,
    done: bool,
}

impl<const N: usize> RecordingWalker<N> {
    /// The next record of the recording; `None` at its end or at the end of what is written.
    pub async fn next<M: LogMedium>(
        &mut self,
        rd: &mut Reader,
        m: &mut M,
        item: &mut [u8],
    ) -> Result<Option<Found>, ReadError<M::Error>> {
        while !self.done {
            let Some(f) = self.walker.next(rd, m, item).await? else {
                return Ok(None);
            };
            let ev = (f.track == 0)
                .then(|| SystemEvent::decode(&mut Dec::new(&item[..f.item_len.min(item.len())])))
                .flatten();
            match ev {
                Some(SystemEvent::RecordingStart { id, .. }) if id == self.id => {
                    self.inside = true;
                    return Ok(Some(f));
                }
                Some(SystemEvent::RecordingStop { id, .. }) if id == self.id && self.inside => {
                    self.done = true;
                    return Ok(Some(f));
                }
                _ if self.inside => return Ok(Some(f)),
                _ => {}
            }
        }
        Ok(None)
    }
}

/// Decodes nothing: for reading segment heads only.
struct NoDecode;

impl Decode for NoDecode {
    fn begin_segment(&mut self, _seg: &SegInfo) {}
    fn entry(&mut self, _entry: &TrackEntry<'_>) {}
    fn wants_segment(&self) -> bool {
        true
    }
    fn begin_flush(&mut self, _full: bool) {}
    fn flush(&mut self, _track: u32, _next: u64) {}
    fn codec(&mut self, _track: u32) -> Option<&mut TrackCodec> {
        None
    }
    fn wants(&self, _track: u32, _head: &StreamHead) -> bool {
        false
    }
}
