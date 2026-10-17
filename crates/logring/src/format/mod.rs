//! The on-medium format, as `docs/format.md` describes it: a superblock in two slots, segments of self-identifying
//! 512-byte blocks, flushes, records and track tables. Pure encode/decode, no I/O, no allocation; every decoder is
//! bounded and returns `None` or an error on garbage (a card is untrusted input).

pub mod crc;
pub mod header;
pub mod record;

/// Format version, in the superblock.
pub const VERSION: u8 = 1;
/// A block: the unit of the medium's power-cut atomicity and of the format.
pub const BLOCK: usize = 512;
/// Bytes of the block trailer.
pub const TRAILER: usize = 8;
/// Payload bytes per block.
pub const PAYLOAD: usize = BLOCK - TRAILER;
/// Longest encoded [StreamHead](ww_stream::StreamHead) a record stores in full.
pub const MAX_HEAD: usize = 64;
/// Longest encoded device time (`ww_si::Second`) stored in a flush head.
pub const MAX_TIME: usize = 24;
/// Bytes of a superblock slot.
pub const SB_LEN: usize = BLOCK;
/// Magic at the start of a superblock.
pub const SB_MAGIC: [u8; 4] = *b"WLRS";

/// What a segment ring holds: the tag and CRC of its blocks depend on it, so a meta block never reads as a data block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    /// Data segments: the tracks' records.
    Data = 0,
    /// The meta area: sessions, bundles, recordings.
    Meta = 1,
}

/// Meta records a writer writes at most between two superblock writes: the meta position a mount that doesn't
/// parse the meta ring's tail continues at is the superblock's `meta_next` plus this, so it never reuses one.
pub const META_SLACK: u64 = 4;

/// A value that means "none" in a `u64` field of the superblock.
pub const NONE: u64 = u64::MAX;

// ---------------------------------------------------------------------------------------------------------------
// Varints and cursors

/// Writes bytes and LEB128 varints into a slice; `None` when it is full.
pub struct Enc<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Enc<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Enc { buf, pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    pub fn done(self) -> &'a mut [u8] {
        &mut self.buf[..self.pos]
    }

    pub fn u8(&mut self, v: u8) -> Option<()> {
        *self.buf.get_mut(self.pos)? = v;
        self.pos += 1;
        Some(())
    }

    pub fn bytes(&mut self, b: &[u8]) -> Option<()> {
        let end = self.pos.checked_add(b.len())?;
        self.buf.get_mut(self.pos..end)?.copy_from_slice(b);
        self.pos = end;
        Some(())
    }

    pub fn uvar(&mut self, mut v: u64) -> Option<()> {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                return self.u8(b);
            }
            self.u8(b | 0x80)?;
        }
    }

    /// Zigzag varint.
    pub fn ivar(&mut self, v: i64) -> Option<()> {
        self.uvar(((v << 1) ^ (v >> 63)) as u64)
    }

    pub fn u32le(&mut self, v: u32) -> Option<()> {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u64le(&mut self, v: u64) -> Option<()> {
        self.bytes(&v.to_le_bytes())
    }

    /// Length-prefixed bytes (a string, an entry).
    pub fn blob(&mut self, b: &[u8]) -> Option<()> {
        self.uvar(b.len() as u64)?;
        self.bytes(b)
    }
}

/// Reads what [Enc] writes; `None` at the end or on a malformed varint.
#[derive(Clone)]
pub struct Dec<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Dec<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Dec { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn left(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let b = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(b)
    }

    pub fn uvar(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            if i == 9 && b > 1 {
                return None;
            }
            v |= ((b & 0x7f) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    pub fn ivar(&mut self) -> Option<i64> {
        let v = self.uvar()?;
        Some(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    pub fn u32le(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    pub fn u64le(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }

    pub fn blob(&mut self) -> Option<&'a [u8]> {
        let n = self.uvar()?;
        self.bytes(usize::try_from(n).ok()?)
    }

    pub fn str(&mut self) -> Option<&'a str> {
        core::str::from_utf8(self.blob()?).ok()
    }
}

/// Bytes a LEB128 varint of `v` takes.
pub const fn uvar_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------------------------------------------
// Blocks

/// Tag of the blocks of segment `seq` of `role` in log `log_id`: a fast check that a block belongs to this lap of
/// the ring; the CRC settles it.
pub fn block_tag(log_id: u32, role: Role, seq: u64) -> u16 {
    let x =
        (seq ^ ((log_id as u64) << 20) ^ ((role as u64) << 60)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let tag = (x >> 48) as u16;
    // never the erased pattern of a NOR flash, nor all zeros
    match tag {
        0xffff => 0x5a5a,
        0x0000 => 0xa5a5,
        t => t,
    }
}

/// CRC of a block: over the log id, role, segment `seq`, the block's index in the segment, its payload and the
/// first 4 bytes of the trailer, so a block moved to another place, lap or ring never checks out.
pub fn block_crc(log_id: u32, role: Role, seq: u64, index: u32, block: &[u8; BLOCK]) -> u32 {
    let mut d = crc::Digest::new();
    d.update(&log_id.to_le_bytes());
    d.update(&[role as u8]);
    d.update(&seq.to_le_bytes());
    d.update(&index.to_le_bytes());
    d.update(&block[..BLOCK - 4]);
    d.finalize()
}

/// Fill in the trailer of `block`: `flush_off` blocks since the start of its flush.
pub fn seal_block(
    block: &mut [u8; BLOCK],
    log_id: u32,
    role: Role,
    seq: u64,
    index: u32,
    flush_off: u16,
) {
    let tag = block_tag(log_id, role, seq);
    block[PAYLOAD..PAYLOAD + 2].copy_from_slice(&tag.to_le_bytes());
    block[PAYLOAD + 2..PAYLOAD + 4].copy_from_slice(&flush_off.to_le_bytes());
    let crc = block_crc(log_id, role, seq, index, block);
    block[BLOCK - 4..].copy_from_slice(&crc.to_le_bytes());
}

/// What a block's place holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockCheck {
    /// A block of this segment: `flush_off` blocks since its flush started.
    Ok { flush_off: u16 },
    /// Its tag says this segment, its CRC says no: a write the power cut, or a damaged block.
    Torn,
    /// Nothing of this segment: an earlier lap, erased bytes, garbage.
    Other,
}

/// Check the trailer of the block at `index` of segment `seq`.
pub fn check_block(
    block: &[u8; BLOCK],
    log_id: u32,
    role: Role,
    seq: u64,
    index: u32,
) -> BlockCheck {
    let tag = u16::from_le_bytes([block[PAYLOAD], block[PAYLOAD + 1]]);
    if tag != block_tag(log_id, role, seq) {
        return BlockCheck::Other;
    }
    let crc = u32::from_le_bytes(block[BLOCK - 4..].try_into().unwrap());
    if crc != block_crc(log_id, role, seq, index, block) {
        return BlockCheck::Torn;
    }
    BlockCheck::Ok {
        flush_off: u16::from_le_bytes([block[PAYLOAD + 2], block[PAYLOAD + 3]]),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Superblock

/// The superblock: geometry, counters and hints, in slot A (offset 0) and slot B (half a segment in), the one with
/// the higher `write_seq` and a good CRC wins. Fixed little-endian layout, one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    /// One more at every write; the slot it goes to is `write_seq % 2`.
    pub write_seq: u64,
    /// Identifies this log: new at every format, so blocks of an earlier log on the same medium never check out.
    pub log_id: u32,
    /// Bytes per segment (one allocation unit of the card).
    pub segment_size: u32,
    /// Segments of the meta area, after the superblock's segment 0.
    pub meta_segments: u32,
    /// Data segments, after the meta area.
    pub data_segments: u32,
    /// Binding to the ring file's place on the card (volume serial, first cluster, cluster count: `ExfatRing`,
    /// LOG-3); zero when the medium is raw.
    pub binding: [u8; 16],
    /// Session the next mount gets.
    pub next_session: u32,
    /// Id the next recording gets (1 at format; 0 is "none").
    pub next_recording: u32,
    /// Highest data segment seq that was opened when this was written, [NONE] before the first.
    pub data_tail_seq: u64,
    /// Highest meta segment seq opened, [NONE] before the first.
    pub meta_tail_seq: u64,
    /// Hash of the API bundle stored last and the meta seq its record is in ([NONE]: no bundle stored).
    pub bundle_hash: u64,
    pub bundle_meta_seq: u64,
    /// Recording running when this was written (0: none) and the data segment it started in.
    pub recording_id: u32,
    pub recording_start_seq: u64,
    /// Data bytes written in all (counts laps); data segments the ring went round and reused; meta segments reused.
    pub bytes_written: u64,
    pub segments_reused: u64,
    pub meta_reused: u32,
    /// Best `flush_bytes` the card check measured at format (0: not measured), LOG-3.
    pub flush_bytes: u32,
    /// The position the next meta record gets, as of this write. Positions written after it are at most
    /// [META_SLACK] further on.
    pub meta_next: u64,
}

impl Superblock {
    pub fn encode(&self) -> [u8; SB_LEN] {
        let mut b = [0u8; SB_LEN];
        b[0..4].copy_from_slice(&SB_MAGIC);
        b[4] = VERSION;
        b[8..16].copy_from_slice(&self.write_seq.to_le_bytes());
        b[16..20].copy_from_slice(&self.log_id.to_le_bytes());
        b[20..24].copy_from_slice(&self.segment_size.to_le_bytes());
        b[24..28].copy_from_slice(&self.meta_segments.to_le_bytes());
        b[28..32].copy_from_slice(&self.data_segments.to_le_bytes());
        b[32..48].copy_from_slice(&self.binding);
        b[48..52].copy_from_slice(&self.next_session.to_le_bytes());
        b[52..56].copy_from_slice(&self.next_recording.to_le_bytes());
        b[56..64].copy_from_slice(&self.data_tail_seq.to_le_bytes());
        b[64..72].copy_from_slice(&self.meta_tail_seq.to_le_bytes());
        b[72..80].copy_from_slice(&self.bundle_hash.to_le_bytes());
        b[80..88].copy_from_slice(&self.bundle_meta_seq.to_le_bytes());
        b[88..92].copy_from_slice(&self.recording_id.to_le_bytes());
        b[96..104].copy_from_slice(&self.recording_start_seq.to_le_bytes());
        b[104..112].copy_from_slice(&self.bytes_written.to_le_bytes());
        b[112..120].copy_from_slice(&self.segments_reused.to_le_bytes());
        b[120..124].copy_from_slice(&self.meta_reused.to_le_bytes());
        b[124..128].copy_from_slice(&self.flush_bytes.to_le_bytes());
        b[128..136].copy_from_slice(&self.meta_next.to_le_bytes());
        let crc = crc::checksum(&b[..SB_LEN - 4]);
        b[SB_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// `None` unless magic, version and CRC check out.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < SB_LEN || b[0..4] != SB_MAGIC || b[4] != VERSION {
            return None;
        }
        let b = &b[..SB_LEN];
        if crc::checksum(&b[..SB_LEN - 4]) != u32_at(b, SB_LEN - 4) {
            return None;
        }
        Some(Superblock {
            write_seq: u64_at(b, 8),
            log_id: u32_at(b, 16),
            segment_size: u32_at(b, 20),
            meta_segments: u32_at(b, 24),
            data_segments: u32_at(b, 28),
            binding: b[32..48].try_into().unwrap(),
            next_session: u32_at(b, 48),
            next_recording: u32_at(b, 52),
            data_tail_seq: u64_at(b, 56),
            meta_tail_seq: u64_at(b, 64),
            bundle_hash: u64_at(b, 72),
            bundle_meta_seq: u64_at(b, 80),
            recording_id: u32_at(b, 88),
            recording_start_seq: u64_at(b, 96),
            bytes_written: u64_at(b, 104),
            segments_reused: u64_at(b, 112),
            meta_reused: u32_at(b, 120),
            flush_bytes: u32_at(b, 124),
            meta_next: u64_at(b, 128),
        })
    }
}

pub(crate) fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

pub(crate) fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

/// FNV-1a, 64 bit: the stable key of a track and the hash of a bundle. Documented in `docs/format.md` so PC tools
/// compute the same.
pub const fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut i = 0;
    while i < parts.len() {
        h = fnv1a_from(h, parts[i]);
        i += 1;
    }
    h
}

/// FNV-1a, 64 bit, going on from `h` (what [fnv1a] returned for the parts before).
pub const fn fnv1a_from(mut h: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
        i += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip() {
        let mut buf = [0u8; 64];
        let mut e = Enc::new(&mut buf);
        for v in [0u64, 1, 127, 128, 300, 1 << 56, u64::MAX] {
            e.uvar(v).unwrap();
        }
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -300] {
            e.ivar(v).unwrap();
        }
        let n = e.len();
        let mut d = Dec::new(&buf[..n]);
        for v in [0u64, 1, 127, 128, 300, 1 << 56, u64::MAX] {
            assert_eq!(d.uvar(), Some(v));
            assert_eq!(uvar_len(v), {
                let mut b = [0u8; 10];
                let mut e = Enc::new(&mut b);
                e.uvar(v).unwrap();
                e.len()
            });
        }
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -300] {
            assert_eq!(d.ivar(), Some(v));
        }
        assert_eq!(d.u8(), None);
        // an overlong varint is refused
        assert_eq!(Dec::new(&[0xff; 11]).uvar(), None);
        assert_eq!(Dec::new(&[0x80; 10]).uvar(), None);
    }

    #[test]
    fn superblock_round_trip_and_crc() {
        let sb = Superblock {
            write_seq: 7,
            log_id: 0xdead_beef,
            segment_size: 4096,
            meta_segments: 2,
            data_segments: 10,
            binding: [3; 16],
            next_session: 4,
            next_recording: 5,
            data_tail_seq: 11,
            meta_tail_seq: NONE,
            bundle_hash: 1,
            bundle_meta_seq: 2,
            recording_id: 9,
            recording_start_seq: 10,
            bytes_written: 1 << 40,
            segments_reused: 3,
            meta_reused: 1,
            flush_bytes: 65536,
            meta_next: (1 << 33) + 5,
        };
        let mut b = sb.encode();
        assert_eq!(Superblock::decode(&b), Some(sb));
        b[40] ^= 1;
        assert_eq!(Superblock::decode(&b), None);
        assert_eq!(Superblock::decode(&b[..100]), None);
    }

    #[test]
    fn block_seal_and_check() {
        let mut b = [0x11u8; BLOCK];
        seal_block(&mut b, 1, Role::Data, 5, 3, 2);
        assert_eq!(
            check_block(&b, 1, Role::Data, 5, 3),
            BlockCheck::Ok { flush_off: 2 }
        );
        assert_eq!(check_block(&b, 1, Role::Meta, 5, 3), BlockCheck::Other);
        assert_eq!(check_block(&b, 1, Role::Data, 6, 3), BlockCheck::Other);
        assert_eq!(check_block(&b, 1, Role::Data, 5, 4), BlockCheck::Torn);
        b[10] ^= 1;
        assert_eq!(check_block(&b, 1, Role::Data, 5, 3), BlockCheck::Torn);
        assert_ne!(
            block_tag(1, Role::Data, 5),
            block_tag(1, Role::Data, 5 + 1000)
        );
    }
}
