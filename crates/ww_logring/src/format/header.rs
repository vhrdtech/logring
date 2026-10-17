//! Segment heads (seq, session, the track table with each track's released position) and flush heads (device time,
//! each track's next position): what makes a segment self-describing and a flush decodable on its own.

use super::{Dec, Enc, MAX_TIME, fnv1a};
use wire_weaver::shrink_wrap::{DeserializeShrinkWrap, SerializeShrinkWrap};
use ww_stream::Second;

/// Longest string a [TrackDesc] field may have.
pub const MAX_STR: usize = 48;
/// Longest body of an encoded [TrackEntry]: the fixed fields, `released`, the strings, the stream session and its
/// first segment.
const MAX_TRACK_BODY: usize = 4 + 8 + 8 + 4 + 4 + 10 + 5 * (1 + MAX_STR) + 5 + 10;
/// Longest encoded [TrackEntry] (its length prefix included).
pub const MAX_TRACK_ENTRY: usize = 2 + MAX_TRACK_BODY;
/// Bytes of the fixed part of a segment head (after the kind byte).
pub const SEG_FIXED: usize = 8 + 4 + 4 + 1;
/// A flush that starts a segment: a segment head, then the flush head.
pub const KIND_SEGMENT: u8 = 2;
/// A plain flush.
pub const KIND_FLUSH: u8 = 1;

/// When a track's records reach the medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Durability {
    /// With the next flush (`flush_bytes` / `flush_interval` / `sync`).
    #[default]
    Normal = 0,
    /// A flush right after every record (markers, errors).
    Immediate = 1,
}

/// A track as the firmware declares it: a stream of a `#[ww_trait]` in the session's API bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackDesc<'a> {
    /// Resource path in the device API with indices: `can[1].rx`.
    pub path: &'a str,
    /// The item type: crate, version and type path, its `ww_self` signature, its `ww_global` id when registered.
    pub crate_name: &'a str,
    pub crate_version: &'a str,
    pub type_path: &'a str,
    pub signature: [u8; 8],
    pub global_id: Option<u32>,
    /// Units of the item's position counter, as text (`bytes`, `frames`, `samples`); empty when unknown.
    pub units: &'a str,
    pub durability: Durability,
    /// Nominal items per second, for sizing and the view; `None` when unknown.
    pub rate: Option<u32>,
}

impl<'a> TrackDesc<'a> {
    /// A track with a path only (type unknown), `Normal` durability.
    pub const fn new(path: &'a str) -> Self {
        TrackDesc {
            path,
            crate_name: "",
            crate_version: "",
            type_path: "",
            signature: [0; 8],
            global_id: None,
            units: "",
            durability: Durability::Normal,
            rate: None,
        }
    }

    /// Every string within [MAX_STR].
    pub fn is_valid(&self) -> bool {
        [
            self.path,
            self.crate_name,
            self.crate_version,
            self.type_path,
            self.units,
        ]
        .iter()
        .all(|s| s.len() <= MAX_STR)
    }

    /// The stable key: the same track across sessions, firmware updates that keep the path, and split files.
    /// FNV-1a of the API root crate's name, its major version (4 bytes little endian) and the path.
    pub fn key(&self, api_crate: &str, api_major: u32) -> u64 {
        fnv1a(&[
            api_crate.as_bytes(),
            &api_major.to_le_bytes(),
            self.path.as_bytes(),
        ])
    }
}

/// A track table entry as stored: a [TrackDesc] with its number in the session and its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackEntry<'a> {
    /// Track number in the session (1 and up; 0 is the system track and is never in the table).
    pub number: u32,
    /// 0: this device. Reserved for several devices logging into one card.
    pub source: u8,
    pub key: u64,
    pub desc: TrackDesc<'a>,
    /// Position released by the host (acknowledged) when the segment was opened.
    pub released: u64,
    /// The track's stream session (`ww_stream`'s, set by [StreamStore::reset](ww_stream::store::StreamStore::reset)
    /// through `TrackStore`), 0 before the first.
    pub stream_session: u32,
    /// Data seq of the segment the stream session started in: positions count from 0 there and grow in every
    /// segment after it.
    pub stream_start: u64,
}

impl<'a> TrackEntry<'a> {
    /// Encode as one length-prefixed entry.
    pub fn encode(&self, enc: &mut Enc) -> Option<()> {
        let mut tmp = [0u8; MAX_TRACK_BODY];
        let mut e = Enc::new(&mut tmp);
        e.u8(self.number.try_into().ok()?)?;
        e.u8(self.source)?;
        e.u8(self.desc.durability as u8)?;
        e.u8(0)?; // compression, none
        e.u64le(self.key)?;
        e.bytes(&self.desc.signature)?;
        e.u32le(self.desc.global_id.unwrap_or(0))?;
        e.u32le(self.desc.rate.unwrap_or(0))?;
        e.uvar(self.released)?;
        for s in [
            self.desc.path,
            self.desc.crate_name,
            self.desc.crate_version,
            self.desc.type_path,
            self.desc.units,
        ] {
            if s.len() > MAX_STR {
                return None;
            }
            e.blob(s.as_bytes())?;
        }
        e.uvar(self.stream_session as u64)?;
        e.uvar(self.stream_start)?;
        let n = e.len();
        enc.blob(&tmp[..n])
    }

    /// Decode one entry; fields a later version appends are ignored.
    pub fn decode(dec: &mut Dec<'a>) -> Option<Self> {
        let body = dec.blob()?;
        let mut d = Dec::new(body);
        let number = d.u8()? as u32;
        let source = d.u8()?;
        let durability = match d.u8()? {
            0 => Durability::Normal,
            1 => Durability::Immediate,
            _ => return None,
        };
        let _compression = d.u8()?;
        let key = d.u64le()?;
        let signature = d.bytes(8)?.try_into().ok()?;
        let global_id = d.u32le()?;
        let rate = d.u32le()?;
        let released = d.uvar()?;
        let path = d.str()?;
        let crate_name = d.str()?;
        let crate_version = d.str()?;
        let type_path = d.str()?;
        let units = d.str()?;
        // appended fields: a reader of an entry without them takes the defaults
        let stream_session = if d.left() > 0 {
            u32::try_from(d.uvar()?).ok()?
        } else {
            0
        };
        let stream_start = if d.left() > 0 { d.uvar()? } else { 0 };
        let desc = TrackDesc {
            path,
            crate_name,
            crate_version,
            type_path,
            signature,
            global_id: (global_id != 0).then_some(global_id),
            units,
            durability,
            rate: (rate != 0).then_some(rate),
        };
        if number == 0 || !desc.is_valid() {
            return None;
        }
        Some(TrackEntry {
            number,
            source,
            key,
            desc,
            released,
            stream_session,
            stream_start,
        })
    }
}

/// The fixed part of a segment head: what a binary search over segments reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegHead {
    pub seq: u64,
    pub session: u32,
    /// Data tracks in the table (numbers 1..=tracks).
    pub tracks: u8,
}

impl SegHead {
    /// The kind byte and the fixed part; the track entries follow, then the flush head.
    pub fn encode(&self, enc: &mut Enc) -> Option<()> {
        enc.u8(KIND_SEGMENT)?;
        enc.u64le(self.seq)?;
        enc.u32le(self.session)?;
        enc.u32le(0)?;
        enc.u8(self.tracks)
    }

    /// Decode the kind byte and the fixed part, `None` when it isn't a segment head.
    pub fn decode(dec: &mut Dec) -> Option<Self> {
        if dec.u8()? != KIND_SEGMENT {
            return None;
        }
        let seq = dec.u64le()?;
        let session = dec.u32le()?;
        let _flags = dec.u32le()?;
        let tracks = dec.u8()?;
        Some(SegHead {
            seq,
            session,
            tracks,
        })
    }
}

/// The head of a flush, after the segment head when the flush starts a segment: the device time last seen and every
/// track's next position (track 0 first), the base the flush's records are delta-coded against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlushHead {
    pub t: Option<Second>,
}

impl FlushHead {
    /// Encode `t` and `next` for tracks `0..next.len()`. Writes the kind byte only when `plain` (a flush that
    /// doesn't start a segment).
    pub fn encode(&self, enc: &mut Enc, plain: bool, next: &[u64]) -> Option<()> {
        if plain {
            enc.u8(KIND_FLUSH)?;
        }
        encode_time(enc, self.t)?;
        enc.uvar(next.len() as u64)?;
        for &n in next {
            enc.uvar(n)?;
        }
        Some(())
    }

    /// Decode after the kind byte (and the segment head): `t`, then the positions through `next`
    /// (called with the track number and its position).
    pub fn decode(dec: &mut Dec, mut next: impl FnMut(u32, u64) -> Option<()>) -> Option<Self> {
        let t = decode_time(dec)?;
        let n = dec.uvar()?;
        if n > u32::MAX as u64 {
            return None;
        }
        for track in 0..n as u32 {
            let p = dec.uvar()?;
            next(track, p)?;
        }
        Some(FlushHead { t })
    }
}

/// An optional device time: a length byte (0: none) and the `Second`'s ShrinkWrap bytes.
pub fn encode_time(enc: &mut Enc, t: Option<Second>) -> Option<()> {
    match t {
        None => enc.u8(0),
        Some(t) => {
            let mut tmp = [0u8; MAX_TIME];
            let b = t.to_ww_bytes(&mut tmp).ok()?;
            if b.is_empty() || b.len() > MAX_TIME {
                return None;
            }
            enc.u8(b.len() as u8)?;
            enc.bytes(b)
        }
    }
}

pub fn decode_time(dec: &mut Dec) -> Option<Option<Second>> {
    let n = dec.u8()? as usize;
    if n == 0 {
        return Some(None);
    }
    if n > MAX_TIME {
        return None;
    }
    let b = dec.bytes(n)?;
    Some(Some(Second::from_ww_bytes(b).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ww_si::Prefix;
    use ww_si::ww_numeric::NumericValue;

    fn desc() -> TrackDesc<'static> {
        TrackDesc {
            path: "can[1].rx",
            crate_name: "ww_can_bus",
            crate_version: "0.3.1",
            type_path: "Frame",
            signature: [1, 2, 3, 4, 5, 6, 7, 8],
            global_id: Some(77),
            units: "frames",
            durability: Durability::Immediate,
            rate: Some(2000),
        }
    }

    #[test]
    fn track_entry_round_trip() {
        let e = TrackEntry {
            number: 3,
            source: 0,
            key: desc().key("usb_io_fw", 2),
            desc: desc(),
            released: 1234,
            stream_session: 7,
            stream_start: 1 << 40,
        };
        let mut buf = [0u8; MAX_TRACK_ENTRY];
        let mut enc = Enc::new(&mut buf);
        e.encode(&mut enc).unwrap();
        let n = enc.len();
        assert_eq!(TrackEntry::decode(&mut Dec::new(&buf[..n])), Some(e));
        // truncated: refused, not panicking
        for cut in 0..n {
            assert_eq!(TrackEntry::decode(&mut Dec::new(&buf[..cut])), None);
        }
        // a minimal entry
        let e = TrackEntry {
            number: 1,
            source: 0,
            key: 0,
            desc: TrackDesc::new("x"),
            released: 0,
            stream_session: 0,
            stream_start: 0,
        };
        let mut enc = Enc::new(&mut buf);
        e.encode(&mut enc).unwrap();
        let n = enc.len();
        assert_eq!(TrackEntry::decode(&mut Dec::new(&buf[..n])), Some(e));
    }

    #[test]
    fn heads_round_trip() {
        let t = Second {
            prefix: Prefix::Micro,
            value: NumericValue::U32(123_456),
        };
        let mut buf = [0u8; 128];
        let mut enc = Enc::new(&mut buf);
        SegHead {
            seq: 9,
            session: 2,
            tracks: 0,
        }
        .encode(&mut enc)
        .unwrap();
        FlushHead { t: Some(t) }
            .encode(&mut enc, false, &[5, 1 << 40])
            .unwrap();
        FlushHead { t: None }.encode(&mut enc, true, &[]).unwrap();
        let n = enc.len();
        let mut dec = Dec::new(&buf[..n]);
        let sh = SegHead::decode(&mut dec).unwrap();
        assert_eq!(sh.seq, 9);
        let mut got = [0u64; 2];
        let fh = FlushHead::decode(&mut dec, |i, p| {
            got[i as usize] = p;
            Some(())
        })
        .unwrap();
        assert_eq!(fh.t, Some(t));
        assert_eq!(got, [5, 1 << 40]);
        assert_eq!(dec.u8(), Some(KIND_FLUSH));
        let fh = FlushHead::decode(&mut dec, |_, _| None).unwrap();
        assert_eq!(fh.t, None);
        assert_eq!(dec.left(), 0);
    }
}
