//! A sequential scan of a whole ring (`std`): every segment, flush and record, for tests, `logring check` and the
//! fuzz target. The indexed reader (LOG-2) is the one for the device and for big cards; this one reads everything.

use crate::format::header::{
    Durability, KIND_FLUSH, MAX_SEG_HEAD, MAX_TRACK_ENTRY, SegHead, TrackEntry,
};
use crate::format::record::{Decoded, MAX_REC_HEADER, TrackCodec, decode_record};
use crate::format::{BLOCK, BlockCheck, Dec, Role, Superblock, check_block};
use crate::stream::BlockStream;
use std::vec::Vec;
use ww_stream::StreamHead;
use ww_stream::store::LogMedium;

/// A track table entry, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub number: u32,
    pub key: u64,
    pub released: u64,
    pub stream_session: u32,
    pub stream_start: u64,
    /// The stream's path in the device API, wire_weaver ids.
    pub path: Vec<u32>,
    pub signature: Option<[u8; 8]>,
    pub global_id: Option<u32>,
    pub durability: Durability,
}

/// A record as scanned.
#[derive(Debug, Clone, PartialEq)]
pub struct Rec {
    pub track: u32,
    pub head: StreamHead,
    pub item: Vec<u8>,
    /// Index of the flush in its segment.
    pub flush: u32,
    /// Block of the segment and offset in its payload where the item starts.
    pub at: (u32, usize),
}

/// One segment as scanned.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentScan {
    pub seq: u64,
    pub session: u32,
    /// The segment head's flag bits (`SEG_FRESH`).
    pub flags: u32,
    pub tracks: Vec<TrackInfo>,
    pub records: Vec<Rec>,
    /// Valid blocks from the start.
    pub blocks: u32,
    pub flushes: u32,
    /// The segment ended in a torn block (bad CRC) or inside a record / header.
    pub torn_block: bool,
    pub torn_record: bool,
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub superblock: Superblock,
    /// Data segments in seq order, every one with a valid block 0.
    pub data: Vec<SegmentScan>,
    /// Meta segments in seq order.
    pub meta: Vec<SegmentScan>,
}

/// Errors of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScanError<E> {
    #[error("medium error")]
    Medium(E),
    #[error("no superblock")]
    NoSuperblock,
    #[error("geometry")]
    Geometry,
}

impl Scan {
    /// Scan the ring on `medium`. `segment_size`: the segment size to look for slot B at; `None` tries slot A only
    /// and then the power-of-two sizes from 2 KB to 64 MB.
    pub async fn open<M: LogMedium>(
        medium: &mut M,
        segment_size: Option<u32>,
    ) -> Result<Self, ScanError<M::Error>> {
        let mut block = [0u8; BLOCK];
        let mut best: Option<Superblock> = None;
        medium
            .read(0, &mut block)
            .await
            .map_err(ScanError::Medium)?;
        if let Some(sb) = Superblock::decode(&block) {
            best = Some(sb);
        }
        let candidates: Vec<u64> = match segment_size {
            Some(s) => vec![s as u64],
            None => best
                .map(|b| vec![b.segment_size as u64])
                .unwrap_or_else(|| (11..=26).map(|i| 1u64 << i).collect()),
        };
        for seg in candidates {
            if seg / 2 + BLOCK as u64 > medium.capacity() {
                continue;
            }
            medium
                .read(seg / 2, &mut block)
                .await
                .map_err(ScanError::Medium)?;
            if let Some(sb) = Superblock::decode(&block)
                && best.is_none_or(|b| sb.write_seq > b.write_seq)
            {
                best = Some(sb);
            }
        }
        let sb = best.ok_or(ScanError::NoSuperblock)?;
        let seg = sb.segment_size as u64;
        let blocks = (seg / BLOCK as u64) as u32;
        if seg == 0 || !seg.is_multiple_of(BLOCK as u64) || blocks < 4 || sb.meta_segments == 0 {
            return Err(ScanError::Geometry);
        }
        let meta_n = sb.meta_segments as u64;
        let data_n = sb.data_segments as u64;
        if medium.capacity() / seg < 1 + meta_n + data_n || data_n < 2 {
            return Err(ScanError::Geometry);
        }
        let mut scan = Scan {
            superblock: sb,
            data: Vec::new(),
            meta: Vec::new(),
        };
        for (role, n) in [(Role::Meta, meta_n), (Role::Data, data_n)] {
            let hint = match role {
                Role::Meta => sb.meta_tail_seq,
                Role::Data => sb.data_tail_seq,
            };
            if hint == u64::MAX {
                continue;
            }
            // every slot may hold a segment of some seq near the hint: the seqs in a window around it
            let lo = (hint + 2).saturating_sub(n + 1);
            let mut found = Vec::new();
            for seq in lo..=hint + n {
                let base = seg * (1 + if role == Role::Meta { 0 } else { meta_n } + seq % n);
                medium
                    .read(base, &mut block)
                    .await
                    .map_err(ScanError::Medium)?;
                if check_block(&block, sb.log_id, role, seq, 0) != BlockCheck::Other {
                    found.push((seq, base));
                }
            }
            found.sort();
            found.dedup();
            // of the seqs sharing a slot keep the highest (the one actually there)
            let mut by_slot: std::collections::BTreeMap<u64, (u64, u64)> = Default::default();
            for (seq, base) in found {
                by_slot.insert(seq % n, (seq, base));
            }
            let mut segs: Vec<(u64, u64)> = by_slot.into_values().collect();
            segs.sort();
            let out = match role {
                Role::Meta => &mut scan.meta,
                Role::Data => &mut scan.data,
            };
            for (seq, base) in segs {
                out.push(
                    scan_segment(medium, &mut block, base, sb.log_id, role, seq, blocks).await?,
                );
            }
        }
        Ok(scan)
    }

    /// Every data record of every data segment, oldest first.
    pub fn records(&self) -> impl Iterator<Item = (&SegmentScan, &Rec)> {
        self.data
            .iter()
            .flat_map(|s| s.records.iter().map(move |r| (s, r)))
    }
}

async fn scan_segment<M: LogMedium>(
    medium: &mut M,
    buf: &mut [u8; BLOCK],
    base: u64,
    log_id: u32,
    role: Role,
    seq: u64,
    blocks: u32,
) -> Result<SegmentScan, ScanError<M::Error>> {
    let mut out = SegmentScan {
        seq,
        session: 0,
        flags: 0,
        tracks: Vec::new(),
        records: Vec::new(),
        blocks: 0,
        flushes: 0,
        torn_block: false,
        torn_record: false,
    };
    let mut stream = BlockStream::new(medium, buf, base, log_id, role, seq, blocks, 0);
    let mut peek = [0u8; crate::writer::PEEK];
    let mut codecs = Vec::new();
    // every track's position at the segment's first flush head, and where its records got to
    let mut starts = Vec::new();
    let mut nexts = Vec::new();
    let mut n_tracks = 1usize;
    let mut torn = false;
    // the segment head
    let n = stream
        .peek(&mut peek[..MAX_SEG_HEAD])
        .await
        .map_err(ScanError::Medium)?;
    let mut d = Dec::new(&peek[..n]);
    match SegHead::decode(&mut d) {
        Some(sh) => {
            out.session = sh.session;
            out.flags = sh.flags;
            n_tracks = sh.tracks as usize + 1;
            stream.skip(d.pos()).await.map_err(ScanError::Medium)?;
            for _ in 0..sh.tracks {
                let n = stream
                    .peek(&mut peek[..MAX_TRACK_ENTRY + 10])
                    .await
                    .map_err(ScanError::Medium)?;
                let mut d = Dec::new(&peek[..n]);
                let Some(e) = TrackEntry::decode(&mut d) else {
                    torn = true;
                    break;
                };
                out.tracks.push(TrackInfo {
                    number: e.number,
                    key: e.key,
                    released: e.released,
                    stream_session: e.stream_session,
                    stream_start: e.stream_start,
                    path: e.desc.path.iter().flatten().map(|id| id.0).collect(),
                    signature: e.desc.signature,
                    global_id: e.desc.global_id,
                    durability: e.desc.durability,
                });
                stream.skip(d.pos()).await.map_err(ScanError::Medium)?;
            }
        }
        _ => torn = true,
    }
    let mut flush_start = 0u32;
    let mut first = true;
    while !torn {
        // a flush head: at the segment start, and at every block whose flush offset is 0
        let at_start = stream.at_block_start();
        let off = stream.flush_off().await.map_err(ScanError::Medium)?;
        let Some(off) = off else { break };
        if first || (at_start && off == 0 && stream.block() != flush_start) {
            if !first {
                let n = stream
                    .peek(&mut peek[..1])
                    .await
                    .map_err(ScanError::Medium)?;
                if n == 0 || peek[0] != KIND_FLUSH {
                    torn = true;
                    break;
                }
                stream.skip(1).await.map_err(ScanError::Medium)?;
            }
            flush_start = stream.block();
            first = false;
            if codecs.is_empty() {
                codecs = vec![TrackCodec::default(); n_tracks];
                starts = vec![None; n_tracks];
                nexts = vec![0u64; n_tracks];
            }
            let mut entries = Vec::new();
            let fh = stream
                .flush_head(|track, p| {
                    entries.push((track as usize, p));
                    (track as usize) < n_tracks
                })
                .await
                .map_err(ScanError::Medium)?;
            let Some(fh) = fh else {
                torn = true;
                break;
            };
            // a full head lists every track; a sparse one every track written since its full head, and the
            // others are where that one put them
            let full = fh.full_back == 0;
            let mut listed = vec![false; n_tracks];
            for &(track, p) in &entries {
                listed[track] = true;
                nexts[track] = p;
                if full {
                    starts[track] = Some(p);
                }
            }
            let complete = (0..n_tracks)
                .all(|i| listed[i] || (!full && starts[i].is_some_and(|s| s == nexts[i])));
            if !complete || (out.flushes == 0 && !full) {
                torn = true;
                break;
            }
            for (c, n) in codecs.iter_mut().zip(nexts.iter()) {
                c.reset(*n);
            }
            out.flushes += 1;
        }
        let n = stream
            .peek(&mut peek[..MAX_REC_HEADER])
            .await
            .map_err(ScanError::Medium)?;
        if n == 0 {
            break;
        }
        let mut d = Dec::new(&peek[..n]);
        match decode_record(&mut d, &mut codecs) {
            Some(Decoded::Padding) => stream.skip_block(),
            Some(Decoded::Record {
                track,
                head,
                item_len,
            }) => {
                stream.skip(d.pos()).await.map_err(ScanError::Medium)?;
                let at = (stream.block(), stream.offset());
                let mut item = vec![0u8; item_len];
                if !stream.read(&mut item).await.map_err(ScanError::Medium)? {
                    torn = true;
                    break;
                }
                nexts[track as usize] = head.end();
                out.records.push(Rec {
                    track,
                    head,
                    item,
                    flush: out.flushes - 1,
                    at,
                });
            }
            None => {
                torn = true;
                break;
            }
        }
    }
    out.torn_record = torn;
    out.torn_block = stream.ended() == Some(BlockCheck::Torn);
    out.blocks = stream.block().min(blocks);
    Ok(out)
}
