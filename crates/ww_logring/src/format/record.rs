//! Records: a track number, the event's [StreamHead] and its item bytes, with the head delta-coded against the
//! track's previous record in the same flush so a small item on a busy track costs about 4 bytes.
//!
//! ```text
//! byte 0: t t t f f f f f    ttt: track 0..6, 7: a varint of (track - 7) follows
//!                            fffff: FULL POS UNITS LOST T
//! FULL:  a byte with the head's length, then the StreamHead as on the wire (other flags unused)
//! POS:   varint pos; otherwise pos = the track's previous end + lost
//! UNITS: varint units; otherwise 1
//! LOST:  varint lost; otherwise none
//! T:     zigzag varint of the time minus the track's previous time (same prefix and integer kind); otherwise none
//! then:  varint item length, the item
//! 0xff:  padding up to the end of the block (no record)
//! ```
//!
//! The first record of a track in a flush, or one whose time can't be a delta, stores the head in full.

use super::{Dec, Enc, MAX_HEAD};
use wire_weaver::shrink_wrap::{DeserializeShrinkWrap, SerializeShrinkWrap};
use ww_si::ww_numeric::NumericValue;
use ww_stream::{Second, StreamHead};

const FULL: u8 = 0x10;
const POS: u8 = 0x08;
const UNITS: u8 = 0x04;
const LOST: u8 = 0x02;
const T: u8 = 0x01;
const TRACK_EXT: u8 = 7;
/// The padding byte: the rest of the block holds no record.
pub const PAD: u8 = 0xff;
/// Longest record header (everything before the item), bytes.
pub const MAX_REC_HEADER: usize = 1 + 10 + 1 + MAX_HEAD + 10;

/// Per-track state the delta coding runs on, reset at every flush start with [TrackCodec::reset].
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackCodec {
    prev_end: u64,
    /// The last time, when [TimeBase::known]: not an `Option`, whose `None` would sit in a niche of the prefix, so a
    /// fresh codec is all zero bytes and a static of writer buffers stays in `.bss`
    prev_t: TimeBase,
}

impl TrackCodec {
    pub const EMPTY: TrackCodec = TrackCodec {
        prev_end: 0,
        prev_t: TimeBase::ZERO,
    };

    fn prev_t(&self) -> Option<TimeBase> {
        self.prev_t.known.then_some(self.prev_t)
    }

    fn set_prev_t(&mut self, t: Option<TimeBase>) {
        self.prev_t = t.unwrap_or(TimeBase::ZERO);
    }
}

/// A time a delta can be coded against: an integer `Second` of 32 or 64 bits.
#[derive(Debug, Clone, Copy)]
struct TimeBase {
    prefix: ww_si::Prefix,
    wide: bool,
    value: u64,
    /// A time is there (false only in [TimeBase::ZERO])
    known: bool,
}

impl TimeBase {
    const ZERO: TimeBase = TimeBase {
        prefix: ww_si::Prefix::Unit,
        wide: false,
        value: 0,
        known: false,
    };
}

impl Default for TimeBase {
    fn default() -> Self {
        TimeBase::ZERO
    }
}

fn base_of(t: Option<Second>) -> Option<TimeBase> {
    let t = t?;
    let (wide, value) = match t.value {
        NumericValue::U32(v) => (false, v as u64),
        NumericValue::U64(v) => (true, v),
        _ => return None,
    };
    Some(TimeBase {
        prefix: t.prefix,
        wide,
        value,
        known: true,
    })
}

impl TrackCodec {
    /// At a flush start: the track's next position is `next`, no time known.
    pub fn reset(&mut self, next: u64) {
        self.prev_end = next;
        self.set_prev_t(None);
    }

    fn after(&mut self, head: &StreamHead) {
        self.prev_end = head.end();
        self.set_prev_t(base_of(head.t));
    }
}

/// `Some(delta)` when `t` can be coded against `prev`: same prefix, both `U32` or both `U64`.
fn time_delta(prev: Option<TimeBase>, t: Option<Second>) -> Option<i64> {
    let (p, t) = (prev?, base_of(t)?);
    if p.prefix != t.prefix || p.wide != t.wide {
        return None;
    }
    if p.wide {
        Some(t.value.wrapping_sub(p.value) as i64)
    } else {
        Some(t.value as i64 - p.value as i64)
    }
}

fn time_from_delta(prev: TimeBase, delta: i64) -> Option<Second> {
    let value = if prev.wide {
        NumericValue::U64(prev.value.wrapping_add(delta as u64))
    } else {
        NumericValue::U32(u32::try_from(prev.value as i64 + delta).ok()?)
    };
    Some(Second::from_parts(prev.prefix, value))
}

/// Encode a record's header (the item's bytes follow it, `item_len` of them) and advance the track's codec.
/// `None` when the buffer is too small or the head can't be encoded.
pub fn encode_record(
    enc: &mut Enc,
    codec: &mut TrackCodec,
    track: u32,
    head: &StreamHead,
    item_len: usize,
) -> Option<()> {
    let delta = time_delta(codec.prev_t(), head.t);
    let full = head.ext.is_some() || (head.t.is_some() && delta.is_none());
    let mut flags = 0u8;
    if full {
        flags |= FULL;
    } else {
        if head.pos != codec.prev_end + head.lost_units() {
            flags |= POS;
        }
        if head.units != 1 {
            flags |= UNITS;
        }
        if head.lost.is_some() {
            flags |= LOST;
        }
        if head.t.is_some() {
            flags |= T;
        }
    }
    let t_bits = if track < TRACK_EXT as u32 {
        track as u8
    } else {
        TRACK_EXT
    };
    enc.u8((t_bits << 5) | flags)?;
    if track >= TRACK_EXT as u32 {
        enc.uvar((track - TRACK_EXT as u32) as u64)?;
    }
    if full {
        let mut tmp = [0u8; MAX_HEAD];
        let b = head.to_ww_bytes(&mut tmp).ok()?;
        if b.is_empty() || b.len() > MAX_HEAD {
            return None;
        }
        enc.u8(b.len() as u8)?;
        enc.bytes(b)?;
    } else {
        if flags & POS != 0 {
            enc.uvar(head.pos)?;
        }
        if flags & UNITS != 0 {
            enc.uvar(head.units)?;
        }
        if let Some(lost) = head.lost {
            enc.uvar(lost)?;
        }
        if let Some(d) = delta
            && flags & T != 0
        {
            enc.ivar(d)?;
        }
    }
    enc.uvar(item_len as u64)?;
    codec.after(head);
    Some(())
}

/// What [decode_record] found at a record's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decoded {
    /// A record: its track, head and the item's length; the item's bytes come next in the stream.
    Record {
        track: u32,
        head: StreamHead,
        item_len: usize,
    },
    /// The padding byte: nothing more in this block.
    Padding,
}

/// Decode a record's header from `dec` (the item's bytes are left in it), with `codecs` indexed by track number.
/// `None` on malformed bytes, a track outside `codecs`, or a stream that ends inside the header.
pub fn decode_record(dec: &mut Dec, codecs: &mut [TrackCodec]) -> Option<Decoded> {
    decode_record_with(dec, |track| codecs.get_mut(track as usize))
}

/// [decode_record] with the codec of the record's track from `codec_of` (`None`: malformed, not decoded).
pub fn decode_record_with<'c>(
    dec: &mut Dec,
    codec_of: impl FnOnce(u32) -> Option<&'c mut TrackCodec>,
) -> Option<Decoded> {
    let b0 = dec.u8()?;
    if b0 == PAD {
        return Some(Decoded::Padding);
    }
    let mut track = (b0 >> 5) as u32;
    let flags = b0 & 0x1f;
    if track == TRACK_EXT as u32 {
        track = track.checked_add(u32::try_from(dec.uvar()?).ok()?)?;
    }
    let codec = codec_of(track)?;
    let head = if flags & FULL != 0 {
        if flags != FULL {
            return None;
        }
        let n = dec.u8()? as usize;
        if n == 0 || n > MAX_HEAD {
            return None;
        }
        StreamHead::from_ww_bytes(dec.bytes(n)?).ok()?
    } else {
        let mut d = dec.clone();
        let pos_explicit = if flags & POS != 0 {
            Some(d.uvar()?)
        } else {
            None
        };
        let units = if flags & UNITS != 0 { d.uvar()? } else { 1 };
        let lost = if flags & LOST != 0 {
            Some(d.uvar()?)
        } else {
            None
        };
        let t = if flags & T != 0 {
            let delta = d.ivar()?;
            Some(time_from_delta(codec.prev_t()?, delta)?)
        } else {
            None
        };
        *dec = d;
        let pos = match pos_explicit {
            Some(p) => p,
            None => codec.prev_end.checked_add(lost.unwrap_or(0))?,
        };
        if pos > StreamHead::MAX_POS {
            return None;
        }
        StreamHead {
            pos,
            units,
            lost,
            t,
            ext: None,
        }
    };
    let item_len = usize::try_from(dec.uvar()?).ok()?;
    head.pos.checked_add(head.units)?;
    codec.after(&head);
    Some(Decoded::Record {
        track,
        head,
        item_len,
    })
}

/// What [skip_record] found at a record's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// A record of `track`, its header `header` bytes long, the item `item_len` bytes after it.
    Record {
        track: u32,
        header: usize,
        item_len: usize,
    },
    /// The padding byte: nothing more in this block.
    Padding,
}

/// The track and the lengths of the record at `dec`'s place, without its head's values: what a reader of one
/// track needs to step over the records of the others (it keeps no delta state for them). `None` on malformed
/// bytes or a stream that ends inside the header.
pub fn skip_record(dec: &mut Dec) -> Option<Skipped> {
    let start = dec.pos();
    let b0 = dec.u8()?;
    if b0 == PAD {
        return Some(Skipped::Padding);
    }
    let mut track = (b0 >> 5) as u32;
    let flags = b0 & 0x1f;
    if track == TRACK_EXT as u32 {
        track = track.checked_add(u32::try_from(dec.uvar()?).ok()?)?;
    }
    if flags & FULL != 0 {
        if flags != FULL {
            return None;
        }
        let n = dec.u8()? as usize;
        if n == 0 || n > MAX_HEAD {
            return None;
        }
        dec.bytes(n)?;
    } else {
        for bit in [POS, UNITS, LOST, T] {
            if flags & bit != 0 {
                dec.uvar()?;
            }
        }
    }
    let item_len = usize::try_from(dec.uvar()?).ok()?;
    Some(Skipped::Record {
        track,
        header: dec.pos() - start,
        item_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ww_si::Prefix;
    use ww_stream::HeadExt;

    fn us(v: u32) -> Second {
        Second::from_parts(Prefix::Micro, NumericValue::U32(v))
    }

    fn round_trip(heads: &[(u32, StreamHead)]) -> Vec<usize> {
        let mut buf = [0u8; 1024];
        let mut enc = Enc::new(&mut buf);
        let mut codecs = [TrackCodec::default(); 10];
        let mut lens = Vec::new();
        for (track, head) in heads {
            let before = enc.len();
            encode_record(&mut enc, &mut codecs[*track as usize], *track, head, 3).unwrap();
            enc.bytes(&[1, 2, 3]).unwrap();
            lens.push(enc.len() - before - 3);
        }
        enc.u8(PAD).unwrap();
        let n = enc.len();
        let mut dec = Dec::new(&buf[..n]);
        let mut codecs = [TrackCodec::default(); 10];
        for (track, head) in heads {
            let got = decode_record(&mut dec, &mut codecs).unwrap();
            assert_eq!(
                got,
                Decoded::Record {
                    track: *track,
                    head: *head,
                    item_len: 3
                }
            );
            assert_eq!(dec.bytes(3), Some(&[1u8, 2, 3][..]));
        }
        assert_eq!(decode_record(&mut dec, &mut codecs), Some(Decoded::Padding));
        // stepping over them without the delta state finds the same tracks and lengths
        let mut dec = Dec::new(&buf[..n]);
        for ((track, _), len) in heads.iter().zip(lens.iter()) {
            assert_eq!(
                skip_record(&mut dec),
                Some(Skipped::Record {
                    track: *track,
                    header: *len,
                    item_len: 3
                })
            );
            dec.bytes(3).unwrap();
        }
        assert_eq!(skip_record(&mut dec), Some(Skipped::Padding));
        lens
    }

    #[test]
    fn compact_and_full_heads() {
        let mut h = StreamHead::at(0);
        h.t = Some(us(1000));
        let h2 = StreamHead {
            pos: 1,
            units: 1,
            lost: None,
            t: Some(us(1500)),
            ext: None,
        };
        let h3 = StreamHead {
            pos: 5,
            units: 2,
            lost: Some(3),
            t: Some(us(1400)),
            ext: None,
        };
        let h4 = StreamHead {
            pos: 7,
            units: 1,
            lost: None,
            t: Some(us(1600)),
            ext: Some(HeadExt {
                clock_epoch: Some(wire_weaver::shrink_wrap::UVlq32(4)),
            }),
        };
        let lens = round_trip(&[(1, h), (1, h2), (1, h3), (1, h4), (9, h2), (9, h2)]);
        // the second record on the track is 1 (byte 0) + 2 (delta 500) + 1 (item length) bytes
        assert_eq!(lens[1], 4);
        assert!(lens[0] > lens[1], "the first one stores the full head");
        assert!(lens[2] > lens[1]);
        assert!(lens[3] > lens[1], "ext means a full head");
        // track 9 repeats h2: an explicit pos (1 isn't the previous end), a zero delta, one byte for the track
        assert_eq!(lens[5], 1 + 1 + 1 + 1 + 1);
    }

    #[test]
    fn no_time_and_mixed_kinds() {
        let a = StreamHead::at(10);
        let b = StreamHead::at(11);
        let mut c = StreamHead::at(12);
        c.t = Some(Second::from_parts(Prefix::Milli, NumericValue::U64(7)));
        let mut d = StreamHead::at(13);
        d.t = Some(us(1));
        let mut e = StreamHead::at(14);
        e.t = Some(us(0));
        let lens = round_trip(&[(0, a), (0, b), (0, c), (0, d), (0, e), (6, a)]);
        assert_eq!(lens[0], 3, "pos explicit (not the codec's 0) but no time");
        assert_eq!(
            lens[1], 2,
            "implicit pos, no time: byte 0 and the item length"
        );
        assert_eq!(lens[4], 3, "delta -1 against the previous record: one byte");
    }

    #[test]
    fn garbage_is_refused() {
        let mut codecs = [TrackCodec::default(); 2];
        // a track beyond the table
        assert_eq!(
            decode_record(&mut Dec::new(&[0xe0, 0x05, 0x00]), &mut codecs),
            None
        );
        // FULL with other flags
        assert_eq!(
            decode_record(&mut Dec::new(&[0x1f, 0x01, 0x00, 0x00]), &mut codecs),
            None
        );
        // FULL with a head length of 0 or past the end
        assert_eq!(
            decode_record(&mut Dec::new(&[0x10, 0x00]), &mut codecs),
            None
        );
        assert_eq!(
            decode_record(&mut Dec::new(&[0x10, 0x09, 0x00]), &mut codecs),
            None
        );
        // a delta without a previous time
        assert_eq!(
            decode_record(&mut Dec::new(&[0x01, 0x02, 0x00]), &mut codecs),
            None
        );
        // empty
        assert_eq!(decode_record(&mut Dec::new(&[]), &mut codecs), None);
    }
}
