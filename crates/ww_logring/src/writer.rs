//! [Logring]: the writer. Mounts (or formats) a ring on a [LogMedium], takes records for its tracks into a stage
//! buffer, writes them in flushes of whole blocks, keeps the meta area and the superblock, and recovers after a
//! power cut ([Recovery]). Layout: [format](crate::format), `docs/format.md`.

use crate::api_root_id;
pub use crate::format::header::MAX_TRACKS;
use crate::format::header::{
    Durability, FlushEntry, FlushHead, KIND_FLUSH, MAX_FLUSH_ENTRY, MAX_FLUSH_HEAD, MAX_SEG_HEAD,
    MAX_TRACK_ENTRY, STORE_SESSION, SegHead, TrackDesc, TrackEntry,
};
use crate::format::record::{
    Decoded, MAX_REC_HEADER, PAD, Skipped, TrackCodec, decode_record_with, encode_record,
    skip_record,
};
use crate::format::{
    BLOCK, BlockCheck, Dec, Enc, NONE, PAYLOAD, Role, SB_LEN, Superblock, check_block, fnv1a,
    seal_block,
};
use crate::stream::BlockStream;
use crate::system::{MetaEvent, SystemEvent};
use wire_weaver::shrink_wrap::SerializeShrinkWrap;
use wire_weaver::ww_version::FullVersion;
use ww_stream::store::FullPolicy;
use ww_stream::store::LogMedium;
use ww_stream::{Second, StreamHead};

/// Bytes of the look-ahead buffer a mount parses headers through.
pub const PEEK: usize = 1024;
/// Longest API bundle, bytes.
pub const MAX_BUNDLE: usize = 1 << 20;
/// Longest item a record can carry.
pub const MAX_ITEM: usize = 1 << 20;
/// Longest flush head of a session with `n` tracks (the system track included): kind, time, a position per track.
const fn flush_head_max(n: usize) -> u64 {
    (MAX_FLUSH_HEAD + MAX_FLUSH_ENTRY * n) as u64
}
/// A full flush head is written at the latest once the data since the last one reaches this many times its size:
/// it bounds how far back the tracks a sparse head lists were written.
const FULL_EVERY: u64 = 32;
/// [Track::tail_map] of a track the tail segment doesn't name.
const NO_TRACK: u32 = u32::MAX;

/// How a [Logring] lays out and runs its medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Bytes per segment: the card's allocation unit (4 MB when it reports none); a multiple of 512 (and of the
    /// medium's erase size, twice it at least, when it needs erasing). At least 4 blocks.
    pub segment_size: u32,
    /// Segments of the meta area (default 4), at least 1.
    pub meta_segments: u32,
    /// Bytes a flush grows to before it is written (default 64 KB); rounded up to whole blocks, at most the
    /// segment's payload.
    pub flush_bytes: u32,
    /// Milliseconds after which a flush with data is written even if small (default 1000), see
    /// [Logring::flush_if_due].
    pub flush_interval_ms: u32,
    /// What to do when the data ring is full: go round (drop oldest, the default) or stop (drop newest).
    pub full: FullPolicy,
    /// Mixed into a new log's id at format: a per-device or random value.
    pub seed: u32,
    /// Where the ring sits on its card (`ExfatRing`'s binding, LOG-3), zero on a raw medium: written into the
    /// superblock at format, and a mount refuses a log whose superblock holds another one ([Error::Unbound]).
    pub binding: [u8; 16],
    /// The `flush_bytes` the card check measured at format (0: not measured), kept in the superblock for later
    /// mounts to start from.
    pub card_flush_bytes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            segment_size: 4 << 20,
            meta_segments: 4,
            flush_bytes: 64 << 10,
            flush_interval_ms: 1000,
            full: FullPolicy::DropOldest,
            seed: 0,
            binding: [0; 16],
            card_flush_bytes: 0,
        }
    }
}

/// What a session (one mount) tells the ring about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInfo<'a> {
    /// The API root: `<ROOT_TRAIT>_FULL_GID` of the API crate. Its [api_root_id] is part of every track's stable
    /// key; the name and version themselves are stored only in the session's meta record, as a fallback for a host
    /// without the bundle.
    pub api: FullVersion<'a>,
    /// Device id, firmware and hardware descriptions (up to 64 bytes each), stored with the session.
    pub device_id: &'a str,
    pub fw: &'a str,
    pub hw: &'a str,
    /// Data tracks: track `i + 1` is `tracks[i]`; track 0 is the system track.
    pub tracks: &'a [TrackDesc<'a>],
    /// The API bundle the tracks' types come from (`ww_self`), stored once per hash in the meta area.
    pub bundle: Option<&'a [u8]>,
}

/// Errors of a [Logring].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error<E> {
    /// The medium failed. The flush being written is lost; the next record opens a new segment.
    #[error("medium error")]
    Medium(E),
    /// The medium holds a log laid out differently, or the config doesn't fit the medium or the stage buffer.
    #[error("geometry mismatch")]
    Geometry,
    /// The session's tracks: more than the writer holds, or a string too long.
    #[error("invalid session info")]
    Session,
    /// No such track.
    #[error("no such track")]
    Track,
    /// The record (or the bundle) doesn't fit a segment.
    #[error("too big")]
    TooBig,
    /// The data ring is full and the policy is drop newest.
    #[error("full")]
    Full,
    /// A head couldn't be encoded (a position of 2⁵⁷ or more, a time that doesn't fit).
    #[error("head not encodable")]
    Head,
    /// The log on the medium is bound to another place ([Config::binding]): nothing was written.
    #[error("log bound elsewhere")]
    Unbound,
    /// A read got a buffer smaller than the item.
    #[error("buffer too small for an item of {item_len} bytes")]
    BufferTooSmall { item_len: usize },
    /// Not mounted yet: [Logring::mount] or [Logring::format] first (or they failed).
    #[error("not mounted")]
    NotMounted,
    /// [Logring::mount] or [Logring::format] on a writer already mounted.
    #[error("already mounted")]
    Mounted,
    /// A stream session with bit 31 set ([STORE_SESSION]): reserved for sessions the logring starts by itself.
    #[error("stream session with bit 31 set")]
    StoreSession,
}

/// What [Logring::mount] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recovery {
    /// No log was on the medium (or [Logring::format] was called): a new one was started.
    pub formatted: bool,
    /// Blocks of the last segment with a bad CRC where its data ended: the write a power cut interrupted, or
    /// damaged blocks.
    pub torn_blocks: u32,
    /// A record cut short at the end of the last flush (its first bytes were written, the rest not).
    pub torn_records: u32,
    /// The mount didn't recover the tracks' positions from the tail (a small build): every track starts at 0 in
    /// this session's first segment, `torn_records` wasn't counted. Also in the session's `SessionStart`.
    pub positions_restarted: bool,
    /// Data segments found beyond the one the superblock knew of.
    pub segments_walked: u32,
    /// Blocks read from the medium by the mount.
    pub reads: u32,
    /// Session this boot got.
    pub session: u32,
}

/// Per-track counters and positions, [Logring::track_state].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackState {
    /// Where the track's stream continues: right after the newest record's units.
    pub next: u64,
    /// Released by the host up to here.
    pub released: u64,
    /// Records stored this session.
    pub records: u64,
    /// Item bytes stored this session.
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Track {
    pub(crate) key: u64,
    pub(crate) next: u64,
    pub(crate) released: u64,
    records: u64,
    bytes: u64,
    immediate: bool,
    codec: TrackCodec,
    /// Position of the newest record pushed
    newest: Option<u64>,
    /// The stream session and the data seq it started in (positions from 0 there)
    pub(crate) stream_session: u32,
    pub(crate) stream_start: u64,
    /// Found in the tail segment's track table at the mount: positions continue
    known: bool,
    /// Records since the data segment's last full flush head: listed in every sparse head after it
    written: bool,
    /// At the mount: the number of this track in the tail segment's table
    tail_number: u32,
    /// At the mount, by the tail segment's track number (this slot's index): this session's track with that key,
    /// [NO_TRACK] for none. Mount-time scratch kept here so a mount needs no array of `TRACKS` on the stack.
    tail_map: u32,
}

impl Track {
    const EMPTY: Track = Track {
        key: 0,
        next: 0,
        released: 0,
        records: 0,
        bytes: 0,
        immediate: false,
        codec: TrackCodec::EMPTY,
        newest: None,
        stream_session: 0,
        stream_start: 0,
        known: false,
        written: false,
        tail_number: 0,
        tail_map: 0,
    };
}

impl Default for Track {
    fn default() -> Self {
        Track::EMPTY
    }
}

/// Where a ring's segments are: its id, segment size and counts, as the superblock gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geo {
    pub log_id: u32,
    /// Bytes per segment.
    pub seg: u64,
    /// Blocks per segment.
    pub blocks: u32,
    pub meta_n: u64,
    pub data_n: u64,
}

impl Geo {
    /// Byte offset on the medium of segment `seq` of `role`.
    pub fn base(&self, role: Role, seq: u64) -> u64 {
        match role {
            Role::Meta => self.seg * (1 + seq % self.meta_n),
            Role::Data => self.seg * (1 + self.meta_n + seq % self.data_n),
        }
    }

    /// Segments in the ring of `role`.
    pub fn count(&self, role: Role) -> u64 {
        match role {
            Role::Meta => self.meta_n,
            Role::Data => self.data_n,
        }
    }
}

/// A segment ring (data or meta): the segment open for writing and the one the next open gets.
#[derive(Debug, Clone, Copy, Default)]
struct Ring {
    seq: u64,
    open: bool,
    next_seq: u64,
    /// Next block index to write in the open segment
    block: u32,
}

/// What is in the stage buffer: sealed blocks and the one being filled, of one flush of one ring.
#[derive(Debug, Clone, Copy)]
struct Stage {
    role: Role,
    seq: u64,
    /// Segment block index of `stage[0]`
    first: u32,
    /// Sealed blocks in the stage
    blocks: usize,
    /// Payload bytes in the block being filled
    cur: usize,
    flush_start: u32,
    flush_open: bool,
    /// Payload bytes of the flush so far (headers included)
    flush_payload: u32,
}

/// A writer on a [LogMedium], see the [module docs](self).
///
/// `BUF` is the stage buffer, a multiple of 512 bytes, at least 512: whole blocks are assembled in it and written
/// when it is full or the flush ends. `TRACKS` is the most tracks a session may have, the system track included
/// (at most [MAX_TRACKS]).
pub struct Logring<'a, M: LogMedium, const BUF: usize, const TRACKS: usize> {
    medium: M,
    cfg: Config,
    geo: Geo,
    info: SessionInfo<'a>,
    session: u32,
    b: &'a mut Buffers<BUF, TRACKS>,
    n_tracks: usize,
    data: Ring,
    meta: Ring,
    meta_next: u64,
    /// Meta records written since the last superblock write: never more than [META_SLACK](crate::format::META_SLACK)
    #[cfg(debug_assertions)]
    meta_since_sb: u64,
    st: Stage,
    scratch: [u8; BLOCK],
    peek: [u8; PEEK],
    t_last: Option<Second>,
    flush_started_ms: Option<u64>,
    sb: Superblock,
    sb_every: u64,
    recovery: Recovery,
    reads: u32,
    /// The data segment's last full flush head: its flush's first block, the tracks written since (listed in the
    /// sparse heads after it), the entries the sparse heads since listed in all, and the payload bytes since
    full_block: u32,
    since_full_written: usize,
    since_full_listed: usize,
    since_full_bytes: u64,
    /// [Logring::mount] or [Logring::format] succeeded
    mounted: bool,
}

/// The writer's memory that grows with `BUF` and `TRACKS`: the stage buffer and the per-track state. The firmware
/// lends it to [Logring::new], so the writer itself stays small and can be moved; [Buffers::new] is a `const fn`,
/// so the buffers can sit in a `static` (a `static_cell::ConstStaticCell`, say) without ever being on the stack.
/// Until then [Buffers::stage] is free scratch, for `exfat::format_card` for one.
pub struct Buffers<const BUF: usize, const TRACKS: usize> {
    stage: StageBuf<BUF>,
    tracks: [Track; TRACKS],
    /// Every track's `next` as of the last sealed block, and as of the last block written
    sealed_next: [u64; TRACKS],
    durable_next: [u64; TRACKS],
    /// Every track's newest record as of the last sealed block, and as of the last block written
    sealed_newest: [Option<u64>; TRACKS],
    durable_newest: [Option<u64>; TRACKS],
}

impl<const BUF: usize, const TRACKS: usize> Buffers<BUF, TRACKS> {
    /// Zeroed buffers (in `.bss` when in a static).
    pub const fn new() -> Self {
        Buffers {
            stage: StageBuf([0; BUF]),
            tracks: [Track::EMPTY; TRACKS],
            sealed_next: [0; TRACKS],
            durable_next: [0; TRACKS],
            sealed_newest: [None; TRACKS],
            durable_newest: [None; TRACKS],
        }
    }

    /// The stage buffer as scratch, while no [Logring] holds the buffers.
    pub fn stage(&mut self) -> &mut [u8] {
        &mut self.stage.0
    }
}

impl<const BUF: usize, const TRACKS: usize> Default for Buffers<BUF, TRACKS> {
    fn default() -> Self {
        Self::new()
    }
}

/// The stage buffer, aligned so a block device driver can hand it to DMA as it is: a flush goes to the medium
/// as one multi-block write without a copy.
#[repr(C, align(32))]
struct StageBuf<const N: usize>([u8; N]);

impl<const N: usize> core::ops::Deref for StageBuf<N> {
    type Target = [u8; N];
    fn deref(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> core::ops::DerefMut for StageBuf<N> {
    fn deref_mut(&mut self) -> &mut [u8; N] {
        &mut self.0
    }
}

const fn round_up(x: u64, to: u64) -> u64 {
    x.div_ceil(to) * to
}

impl<'a, M: LogMedium, const BUF: usize, const TRACKS: usize> Logring<'a, M, BUF, TRACKS> {
    const CHECK: () = {
        assert!(
            BUF >= BLOCK && BUF.is_multiple_of(BLOCK),
            "BUF: whole blocks"
        );
        assert!(TRACKS >= 1 && TRACKS <= MAX_TRACKS, "TRACKS: 1..=65536");
    };

    /// Open the log on the medium in a new session, or start a new log when there is none. Finds the end of the
    /// log after a power cut, see [Recovery]. Everything the mount wrote is on the medium when it returns. Runs
    /// in place: its future holds only the mount's own state, not a second writer. After an error, start again
    /// from [Logring::new].
    pub async fn mount(&mut self) -> Result<(), Error<M::Error>> {
        if self.mounted {
            return Err(Error::Mounted);
        }
        let mut found = None;
        for slot in 0..2 {
            if let Some(sb) = self.read_superblock(slot).await?
                && found.is_none_or(|f: Superblock| sb.write_seq > f.write_seq)
            {
                found = Some(sb);
            }
        }
        if let Some(sb) = found {
            if sb.segment_size as u64 != self.geo.seg
                || sb.meta_segments as u64 != self.geo.meta_n
                || sb.data_segments as u64 != self.geo.data_n
            {
                return Err(Error::Geometry);
            }
            if sb.binding != self.cfg.binding {
                return Err(Error::Unbound);
            }
        }
        // the mount's own records (session start, the system track) go through the public paths
        self.mounted = true;
        let r = match found {
            None => self.start_log(None).await,
            Some(sb) => self.recover(sb).await,
        };
        self.mounted = r.is_ok();
        r
    }

    /// Start a new, empty log on the medium, whatever it holds, in place like [Logring::mount].
    pub async fn format(&mut self) -> Result<(), Error<M::Error>> {
        if self.mounted {
            return Err(Error::Mounted);
        }
        let mut old = None;
        for slot in 0..2 {
            if let Some(sb) = self.read_superblock(slot).await?
                && old.is_none_or(|f: Superblock| sb.write_seq > f.write_seq)
            {
                old = Some(sb);
            }
        }
        self.mounted = true;
        let r = self.start_log(old).await;
        self.mounted = r.is_ok();
        r
    }

    /// Host side (`std`): [Logring::new] with buffers leaked for the program's life, then [Logring::mount]. For
    /// tests and short-lived tools; a loop that mounts many times lends its own [Buffers] instead.
    #[cfg(feature = "std")]
    pub async fn mount_leaked(
        medium: M,
        cfg: Config,
        info: SessionInfo<'a>,
    ) -> Result<Self, Error<M::Error>> {
        let b = Box::leak(Box::new(Buffers::new()));
        let mut lr = Self::new(medium, b, cfg, info)?;
        lr.mount().await?;
        Ok(lr)
    }

    /// [Logring::mount_leaked] with [Logring::format].
    #[cfg(feature = "std")]
    pub async fn format_leaked(
        medium: M,
        cfg: Config,
        info: SessionInfo<'a>,
    ) -> Result<Self, Error<M::Error>> {
        let b = Box::leak(Box::new(Buffers::new()));
        let mut lr = Self::new(medium, b, cfg, info)?;
        lr.format().await?;
        Ok(lr)
    }

    /// A writer on `medium` with the lent buffers (reset here), not mounted yet: [Logring::mount] or
    /// [Logring::format] next. Checks the config against the medium and the session against `TRACKS`; writes
    /// nothing.
    pub fn new(
        medium: M,
        b: &'a mut Buffers<BUF, TRACKS>,
        cfg: Config,
        info: SessionInfo<'a>,
    ) -> Result<Self, Error<M::Error>> {
        #[allow(clippy::let_unit_value)]
        let () = Self::CHECK;
        let ws = M::WRITE_SIZE as u64;
        let es = if M::NEEDS_ERASE {
            M::ERASE_SIZE as u64
        } else {
            1
        };
        let seg = cfg.segment_size as u64;
        if ws == 0
            || !(BLOCK as u64).is_multiple_of(ws)
            || !seg.is_multiple_of(BLOCK as u64)
            || seg < 4 * BLOCK as u64
            || !seg.is_multiple_of(2 * es)
            || cfg.meta_segments < 1
            || cfg.flush_bytes == 0
        {
            return Err(Error::Geometry);
        }
        let total = medium.capacity() / seg;
        let meta_n = cfg.meta_segments as u64;
        if total < 1 + meta_n + 2 {
            return Err(Error::Geometry);
        }
        let geo = Geo {
            log_id: 0,
            seg,
            blocks: (seg / BLOCK as u64) as u32,
            meta_n,
            data_n: total - 1 - meta_n,
        };
        if info.tracks.len() + 1 > TRACKS || info.tracks.iter().any(|t| !t.is_valid()) {
            return Err(Error::Session);
        }
        // a segment holds at least its head, the track table, a flush head and a record of a block
        let min_payload = (32 + (MAX_TRACK_ENTRY + 10) * info.tracks.len()) as u64
            + flush_head_max(info.tracks.len() + 1)
            + (MAX_REC_HEADER + BLOCK) as u64;
        if (geo.blocks as u64) * (PAYLOAD as u64) < min_payload {
            return Err(Error::Geometry);
        }
        if [info.device_id, info.fw, info.hw]
            .iter()
            .any(|s| s.len() > crate::system::MAX_LABEL)
        {
            return Err(Error::Session);
        }
        if info.bundle.is_some_and(|b| b.len() > MAX_BUNDLE) {
            return Err(Error::TooBig);
        }
        let mut api = [0u8; crate::system::MAX_API];
        if info.api.to_ww_bytes(&mut api).is_err() {
            return Err(Error::Session);
        }
        let api_root = api_root_id(&info.api);
        b.stage.0.fill(0xff);
        b.tracks.fill(Track::EMPTY);
        b.sealed_next.fill(0);
        b.durable_next.fill(0);
        b.sealed_newest.fill(None);
        b.durable_newest.fill(None);
        for (i, d) in info.tracks.iter().enumerate() {
            b.tracks[i + 1].key = d.key(api_root);
            b.tracks[i + 1].immediate = d.durability == Durability::Immediate;
        }
        b.tracks[0].immediate = true;
        Ok(Logring {
            medium,
            cfg,
            geo,
            info,
            session: 0,
            b,
            n_tracks: info.tracks.len() + 1,
            data: Ring::default(),
            meta: Ring::default(),
            meta_next: 0,
            #[cfg(debug_assertions)]
            meta_since_sb: 0,
            st: Stage {
                role: Role::Data,
                seq: 0,
                first: 0,
                blocks: 0,
                cur: 0,
                flush_start: 0,
                flush_open: false,
                flush_payload: 0,
            },
            scratch: [0; BLOCK],
            peek: [0; PEEK],
            t_last: None,
            flush_started_ms: None,
            sb: Superblock {
                write_seq: 0,
                log_id: 0,
                segment_size: cfg.segment_size,
                meta_segments: cfg.meta_segments,
                data_segments: geo.data_n as u32,
                binding: cfg.binding,
                next_session: 1,
                next_recording: 1,
                data_tail_seq: NONE,
                meta_tail_seq: NONE,
                bundle_hash: 0,
                bundle_meta_seq: NONE,
                recording_id: 0,
                recording_start_seq: 0,
                bytes_written: 0,
                segments_reused: 0,
                meta_reused: 0,
                flush_bytes: cfg.card_flush_bytes,
                meta_next: 0,
            },
            sb_every: (geo.data_n / 4).clamp(1, 16),
            recovery: Recovery::default(),
            reads: 0,
            full_block: 0,
            since_full_written: 0,
            since_full_listed: 0,
            since_full_bytes: 0,
            mounted: false,
        })
    }

    // -----------------------------------------------------------------------------------------------------------
    // State

    fn ready(&self) -> Result<(), Error<M::Error>> {
        if self.mounted {
            Ok(())
        } else {
            Err(Error::NotMounted)
        }
    }

    /// What the mount found.
    pub fn recovery(&self) -> Recovery {
        self.recovery
    }

    /// This boot's session (1 and up).
    pub fn session(&self) -> u32 {
        self.session
    }

    /// Positions and counters of track `track` (0: the system track).
    pub fn track_state(&self, track: u32) -> Option<TrackState> {
        if track as usize >= self.n_tracks {
            return None;
        }
        let t = &self.b.tracks[track as usize];
        Some(TrackState {
            next: t.next,
            released: t.released,
            records: t.records,
            bytes: t.bytes,
        })
    }

    /// Position up to which track `track`'s records are on the medium: every record ending at or before it was
    /// written in a completed write; later ones are in the stage buffer (or in a write that failed).
    pub fn durable(&self, track: u32) -> Option<u64> {
        ((track as usize) < self.n_tracks).then(|| self.b.durable_next[track as usize])
    }

    /// The superblock as it was last written.
    pub fn superblock(&self) -> Superblock {
        self.sb
    }

    /// Id of the recording running, if one is.
    pub fn recording(&self) -> Option<u32> {
        (self.sb.recording_id != 0).then_some(self.sb.recording_id)
    }

    /// The config in use.
    pub fn config(&self) -> Config {
        self.cfg
    }

    /// Change the flush thresholds at run time.
    pub fn set_flush(&mut self, flush_bytes: u32, flush_interval_ms: u32) {
        if flush_bytes > 0 {
            self.cfg.flush_bytes = flush_bytes;
        }
        self.cfg.flush_interval_ms = flush_interval_ms;
    }

    /// The medium.
    pub fn medium(&self) -> &M {
        &self.medium
    }

    /// The medium, to hand it back.
    pub fn into_medium(self) -> M {
        self.medium
    }

    /// Data segments the medium holds.
    pub fn data_segments(&self) -> u64 {
        self.geo.data_n
    }

    /// Seq of the data segment being written (the highest so far).
    pub fn data_seq(&self) -> u64 {
        self.data.seq
    }

    /// Where the ring's segments are.
    pub fn geo(&self) -> Geo {
        self.geo
    }

    /// The oldest data seq the medium still holds (`data_seq() - data_segments() + 1`, or 0 before the ring went
    /// round once).
    pub fn oldest_data_seq(&self) -> u64 {
        (self.data.seq + 1).saturating_sub(self.geo.data_n)
    }

    /// Position of track `track`'s newest record on the medium (see [durable](Self::durable)); `None` when it has
    /// none there or the mount didn't see it (it wasn't in the last flush before the reboot).
    pub fn durable_newest(&self, track: u32) -> Option<u64> {
        self.b.durable_newest.get(track as usize).copied().flatten()
    }

    /// The track's stream session and the data seq it started in: positions count from 0 there.
    pub fn stream_session(&self, track: u32) -> Option<(u32, u64)> {
        ((track as usize) < self.n_tracks).then(|| {
            let t = &self.b.tracks[track as usize];
            (t.stream_session, t.stream_start)
        })
    }

    /// The stable key of track `track` (0 for the system track).
    pub fn track_key(&self, track: u32) -> Option<u64> {
        ((track as usize) < self.n_tracks).then(|| self.b.tracks[track as usize].key)
    }

    /// The host acknowledged track `track` up to `pos`: kept in every segment head from the next one on, so it
    /// survives a reboot (up to the last segment opened). Never moves back, never past the track's next position.
    /// It frees no room: the ring keeps everything until it goes round.
    pub fn release(&mut self, track: u32, pos: u64) -> Result<(), Error<M::Error>> {
        self.ready()?;
        self.track(track)?;
        let t = &mut self.b.tracks[track as usize];
        t.released = t.released.max(pos.min(t.next));
        Ok(())
    }

    /// A new stream session `session` on track `track`: positions from 0, nothing released, and a new data segment
    /// whose head carries it, written before this returns (the records of the session before stay in the ring).
    /// `session` has bit 31 clear ([Error::StoreSession] otherwise: [STORE_SESSION]).
    pub async fn reset_track(&mut self, track: u32, session: u32) -> Result<(), Error<M::Error>> {
        self.ready()?;
        self.track(track)?;
        if track == 0 {
            return Err(Error::Track);
        }
        if session & STORE_SESSION != 0 {
            return Err(Error::StoreSession);
        }
        self.close_segment(Role::Data).await?;
        let i = track as usize;
        let t = &mut self.b.tracks[i];
        t.stream_session = session;
        t.stream_start = self.data.next_seq;
        t.next = 0;
        t.released = 0;
        t.newest = None;
        self.b.sealed_next[i] = 0;
        self.b.durable_next[i] = 0;
        self.b.sealed_newest[i] = None;
        self.b.durable_newest[i] = None;
        self.ensure_flush(Role::Data).await?;
        self.end_flush().await
    }

    /// The medium, for a reader on the same ring (between writes).
    pub(crate) fn medium_mut(&mut self) -> &mut M {
        &mut self.medium
    }

    /// The newest record of track `track` on the medium, found by a reader after the mount.
    pub(crate) fn note_durable_newest(&mut self, track: u32, pos: u64) {
        if let Some(n) = self.b.durable_newest.get_mut(track as usize)
            && n.is_none()
        {
            *n = Some(pos);
            if self.b.tracks[track as usize].newest.is_none() {
                self.b.tracks[track as usize].newest = Some(pos);
            }
            if self.b.sealed_newest[track as usize].is_none() {
                self.b.sealed_newest[track as usize] = Some(pos);
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Pushing

    /// Store `item` on `track` as the next event: position from the track's state, one unit, time `t`.
    pub async fn push(
        &mut self,
        track: u32,
        item: &[u8],
        t: Option<Second>,
    ) -> Result<(), Error<M::Error>> {
        let tr = self.track(track)?;
        let head = StreamHead {
            pos: tr.next,
            units: 1,
            lost: None,
            t,
            ext: None,
        };
        self.push_head(track, &head, item).await
    }

    /// Store an event as it goes on the wire: its head (position and units from the sender) and item bytes.
    pub async fn push_head(
        &mut self,
        track: u32,
        head: &StreamHead,
        item: &[u8],
    ) -> Result<(), Error<M::Error>> {
        self.ready()?;
        self.track(track)?;
        if item.len() > MAX_ITEM || head.pos > StreamHead::MAX_POS {
            return Err(Error::TooBig);
        }
        self.write_record(Role::Data, track, head, &[item]).await?;
        let tr = &mut self.b.tracks[track as usize];
        tr.next = head.end();
        tr.newest = Some(head.pos);
        tr.records += 1;
        tr.bytes += item.len() as u64;
        if head.t.is_some() {
            self.t_last = head.t;
        }
        if tr.immediate || self.st.flush_payload >= self.cfg.flush_bytes {
            self.end_flush().await?;
        }
        Ok(())
    }

    /// A marker in the system track, written at once.
    pub async fn marker(
        &mut self,
        kind: u8,
        label: &str,
        value: Option<i64>,
    ) -> Result<(), Error<M::Error>> {
        self.ready()?;
        self.push_system(SystemEvent::Marker { kind, label, value })
            .await
    }

    /// Start a recording: its id (one more than the last, for the card's life). A recording already running is
    /// stopped first (reason 0).
    pub async fn recording_start(&mut self, trigger: u8) -> Result<u32, Error<M::Error>> {
        self.ready()?;
        if self.sb.recording_id != 0 {
            self.recording_stop(0).await?;
        }
        let id = self.sb.next_recording;
        self.sb.next_recording += 1;
        self.sb.recording_id = id;
        self.sb.recording_start_seq = self.data.seq;
        let ev = MetaEvent::Recording {
            id,
            stopped: false,
            why: trigger,
            session: self.session,
            seq: self.data.seq,
            t: self.t_last,
        };
        self.write_meta(ev, &[]).await?;
        self.write_superblock().await?;
        self.push_system(SystemEvent::RecordingStart { id, trigger })
            .await?;
        Ok(id)
    }

    /// Stop the recording running, if one is.
    pub async fn recording_stop(&mut self, reason: u8) -> Result<(), Error<M::Error>> {
        self.ready()?;
        let id = self.sb.recording_id;
        if id == 0 {
            return Ok(());
        }
        self.push_system(SystemEvent::RecordingStop { id, reason })
            .await?;
        let ev = MetaEvent::Recording {
            id,
            stopped: true,
            why: reason,
            session: self.session,
            seq: self.data.seq,
            t: self.t_last,
        };
        self.write_meta(ev, &[]).await?;
        self.sb.recording_id = 0;
        self.write_superblock().await
    }

    /// Write the flush being filled, if it holds anything: everything pushed so far is on the medium when this
    /// returns.
    pub async fn sync(&mut self) -> Result<(), Error<M::Error>> {
        self.ready()?;
        self.end_flush().await
    }

    /// Write the flush being filled when it has been open for `flush_interval_ms` or more. Call it periodically
    /// (every 100 ms, say) with a monotonic millisecond clock. Returns whether it flushed.
    pub async fn flush_if_due(&mut self, now_ms: u64) -> Result<bool, Error<M::Error>> {
        if !self.st.flush_open || self.st.flush_payload == 0 {
            self.flush_started_ms = None;
            return Ok(false);
        }
        match self.flush_started_ms {
            None => {
                self.flush_started_ms = Some(now_ms);
                Ok(false)
            }
            Some(started) if now_ms.wrapping_sub(started) >= self.cfg.flush_interval_ms as u64 => {
                self.end_flush().await?;
                Ok(true)
            }
            Some(_) => Ok(false),
        }
    }

    fn track(&self, track: u32) -> Result<&Track, Error<M::Error>> {
        if (track as usize) < self.n_tracks {
            Ok(&self.b.tracks[track as usize])
        } else {
            Err(Error::Track)
        }
    }

    async fn push_system(&mut self, ev: SystemEvent<'_>) -> Result<(), Error<M::Error>> {
        let mut tmp = [0u8; 256];
        let mut enc = Enc::new(&mut tmp);
        ev.encode(&mut enc).ok_or(Error::Head)?;
        let n = enc.len();
        let head = StreamHead {
            pos: self.b.tracks[0].next,
            units: 1,
            lost: None,
            t: self.t_last,
            ext: None,
        };
        let item = tmp;
        self.push_head(0, &head, &item[..n]).await
    }

    // -----------------------------------------------------------------------------------------------------------
    // The stage and flushes

    fn ring(&mut self, role: Role) -> &mut Ring {
        match role {
            Role::Data => &mut self.data,
            Role::Meta => &mut self.meta,
        }
    }

    /// Payload bytes left in the open segment of the stage's ring.
    fn room(&self) -> u64 {
        let blocks_left = self
            .geo
            .blocks
            .saturating_sub(self.st.first + self.st.blocks as u32) as u64;
        (blocks_left * PAYLOAD as u64).saturating_sub(self.st.cur as u64)
    }

    /// A flush of `role` is open (a segment of it too), ready for bytes.
    async fn ensure_flush(&mut self, role: Role) -> Result<(), Error<M::Error>> {
        if self.st.flush_open && self.st.role == role {
            return Ok(());
        }
        if self.st.flush_open {
            self.end_flush().await?;
        }
        // a flush needs room for its head and a record; otherwise the segment is done
        let r = *self.ring(role);
        let left = (self.geo.blocks.saturating_sub(r.block) as u64) * PAYLOAD as u64;
        if r.open && left < flush_head_max(self.n_tracks) + MAX_REC_HEADER as u64 {
            self.ring(role).open = false;
        }
        if !self.ring(role).open {
            self.open_segment(role).await
        } else {
            self.begin_flush(role, false).await
        }
    }

    async fn open_segment(&mut self, role: Role) -> Result<(), Error<M::Error>> {
        let seq = self.ring(role).next_seq;
        let n = self.geo.count(role);
        if seq >= n {
            match role {
                Role::Data if self.cfg.full == FullPolicy::DropNewest => return Err(Error::Full),
                Role::Data => self.sb.segments_reused += 1,
                Role::Meta => self.sb.meta_reused += 1,
            }
        }
        let base = self.geo.base(role, seq);
        if M::NEEDS_ERASE {
            self.medium
                .erase(base, base + self.geo.seg)
                .await
                .map_err(Error::Medium)?;
        }
        {
            let r = self.ring(role);
            r.seq = seq;
            r.open = true;
            r.block = 0;
            r.next_seq = seq + 1;
        }
        let first = match role {
            Role::Data => self.sb.data_tail_seq == NONE,
            Role::Meta => self.sb.meta_tail_seq == NONE,
        };
        match role {
            Role::Data => self.sb.data_tail_seq = seq,
            Role::Meta => self.sb.meta_tail_seq = seq,
        }
        if first || role == Role::Meta || seq.is_multiple_of(self.sb_every) {
            self.write_superblock().await?;
        }
        self.begin_flush(role, true).await
    }

    async fn begin_flush(
        &mut self,
        role: Role,
        segment_start: bool,
    ) -> Result<(), Error<M::Error>> {
        let r = *self.ring(role);
        self.st = Stage {
            role,
            seq: r.seq,
            first: r.block,
            blocks: 0,
            cur: 0,
            flush_start: r.block,
            flush_open: true,
            flush_payload: 0,
        };
        self.flush_started_ms = None;
        let n = match role {
            Role::Data => self.n_tracks,
            Role::Meta => 1,
        };
        if segment_start {
            let mut tmp = [0u8; 32];
            let mut enc = Enc::new(&mut tmp);
            SegHead {
                seq: r.seq,
                session: self.session,
                tracks: if role == Role::Data {
                    (n - 1) as u32
                } else {
                    0
                },
            }
            .encode(&mut enc)
            .ok_or(Error::Head)?;
            let len = enc.len();
            self.put(&tmp[..len]).await?;
            if role == Role::Data {
                for i in 1..n {
                    let mut tmp = [0u8; MAX_TRACK_ENTRY + 10];
                    let mut enc = Enc::new(&mut tmp);
                    let t = &self.b.tracks[i];
                    TrackEntry {
                        number: i as u32,
                        source: 0,
                        key: t.key,
                        desc: self.info.tracks[i - 1],
                        released: t.released,
                        stream_session: t.stream_session,
                        stream_start: t.stream_start,
                    }
                    .encode(&mut enc)
                    .ok_or(Error::Session)?;
                    let len = enc.len();
                    self.put(&tmp[..len]).await?;
                }
            }
        }
        // the positions: every track in a full head (a segment's first, and after FULL_EVERY of its size of data
        // or once half the tracks are written), the tracks written since the full head in a sparse one; every
        // codec starts from its track's next position (unchanged since the full head when not listed)
        // full once the sparse heads since the last full one would list as many entries as a full head has: the
        // sparse heads never cost more than the full ones
        let full = match role {
            Role::Data => {
                segment_start
                    || self.since_full_listed + self.since_full_written >= n
                    || self.since_full_bytes >= FULL_EVERY * flush_head_max(n)
            }
            Role::Meta => true,
        };
        let full_back = if full { 0 } else { r.block - self.full_block };
        let entries = match role {
            Role::Data if full => {
                for t in self.b.tracks[..n].iter_mut() {
                    t.written = false;
                }
                self.full_block = r.block;
                self.since_full_written = 0;
                self.since_full_listed = 0;
                self.since_full_bytes = 0;
                n as u32
            }
            Role::Data => {
                self.since_full_listed += self.since_full_written;
                self.since_full_written as u32
            }
            Role::Meta => 1,
        };
        let mut tmp = [0u8; MAX_FLUSH_HEAD];
        let mut enc = Enc::new(&mut tmp);
        FlushHead {
            t: self.t_last,
            full_back,
            entries,
        }
        .encode(&mut enc, !segment_start)
        .ok_or(Error::Head)?;
        let len = enc.len();
        self.put(&tmp[..len]).await?;
        let mut prev = None;
        for i in 0..n {
            let (next, listed) = match role {
                Role::Data => {
                    let t = &mut self.b.tracks[i];
                    t.codec.reset(t.next);
                    (t.next, full || t.written)
                }
                Role::Meta => {
                    self.b.tracks[0].codec.reset(self.meta_next);
                    (self.meta_next, true)
                }
            };
            if !listed {
                continue;
            }
            let mut tmp = [0u8; MAX_FLUSH_ENTRY];
            let mut enc = Enc::new(&mut tmp);
            FlushEntry {
                track: i as u32,
                next,
            }
            .encode(&mut enc, prev)
            .ok_or(Error::Head)?;
            prev = Some(i as u32);
            let len = enc.len();
            self.put(&tmp[..len]).await?;
        }
        Ok(())
    }

    /// Append payload bytes to the open flush; the caller checked the room.
    async fn put(&mut self, mut bytes: &[u8]) -> Result<(), Error<M::Error>> {
        while !bytes.is_empty() {
            let take = (PAYLOAD - self.st.cur).min(bytes.len());
            let at = self.st.blocks * BLOCK + self.st.cur;
            self.b.stage[at..at + take].copy_from_slice(&bytes[..take]);
            self.st.cur += take;
            bytes = &bytes[take..];
            self.st.flush_payload = self.st.flush_payload.saturating_add(take as u32);
            if self.st.cur == PAYLOAD {
                self.seal().await?;
            }
        }
        Ok(())
    }

    /// Seal the block being filled and write the stage when it is full.
    async fn seal(&mut self) -> Result<(), Error<M::Error>> {
        let index = self.st.first + self.st.blocks as u32;
        let at = self.st.blocks * BLOCK;
        let block: &mut [u8; BLOCK] = (&mut self.b.stage[at..at + BLOCK]).try_into().unwrap();
        seal_block(
            block,
            self.geo.log_id,
            self.st.role,
            self.st.seq,
            index,
            (index - self.st.flush_start).min(u16::MAX as u32) as u16,
        );
        self.st.blocks += 1;
        self.st.cur = 0;
        if self.st.role == Role::Data {
            for ((s, n), t) in self
                .b
                .sealed_next
                .iter_mut()
                .zip(self.b.sealed_newest.iter_mut())
                .zip(self.b.tracks.iter())
            {
                *s = t.next;
                *n = t.newest;
            }
        }
        if self.st.blocks * BLOCK == BUF {
            self.write_stage().await?;
        }
        Ok(())
    }

    async fn write_stage(&mut self) -> Result<(), Error<M::Error>> {
        if self.st.blocks == 0 {
            return Ok(());
        }
        let base = self.geo.base(self.st.role, self.st.seq) + self.st.first as u64 * BLOCK as u64;
        let len = self.st.blocks * BLOCK;
        let res = self.medium.write(base, &self.b.stage[..len]).await;
        if let Err(e) = res {
            // the flush is lost; the next record starts a new segment
            self.st.flush_open = false;
            self.st.blocks = 0;
            self.st.cur = 0;
            let role = self.st.role;
            self.ring(role).open = false;
            return Err(Error::Medium(e));
        }
        self.sb.bytes_written += len as u64;
        if self.st.role == Role::Data {
            self.b.durable_next = self.b.sealed_next;
            self.b.durable_newest = self.b.sealed_newest;
        }
        self.st.first += self.st.blocks as u32;
        self.st.blocks = 0;
        let role = self.st.role;
        let first = self.st.first;
        self.ring(role).block = first;
        Ok(())
    }

    async fn end_flush(&mut self) -> Result<(), Error<M::Error>> {
        if !self.st.flush_open {
            return Ok(());
        }
        if self.st.cur > 0 {
            let at = self.st.blocks * BLOCK + self.st.cur;
            let end = self.st.blocks * BLOCK + PAYLOAD;
            self.b.stage[at..end].fill(PAD);
            self.st.cur = PAYLOAD;
            self.seal().await?;
        }
        self.write_stage().await?;
        self.st.flush_open = false;
        self.st.flush_payload = 0;
        self.flush_started_ms = None;
        Ok(())
    }

    /// Close the open segment of `role` (its flush written); the next record opens the next one.
    async fn close_segment(&mut self, role: Role) -> Result<(), Error<M::Error>> {
        if self.st.flush_open && self.st.role == role {
            self.end_flush().await?;
        }
        self.ring(role).open = false;
        Ok(())
    }

    /// Write a record of `track` with `head` and an item made of `parts` to the ring of `role`.
    async fn write_record(
        &mut self,
        role: Role,
        track: u32,
        head: &StreamHead,
        parts: &[&[u8]],
    ) -> Result<(), Error<M::Error>> {
        let item_len: usize = parts.iter().map(|p| p.len()).sum();
        let mut fresh = false;
        loop {
            self.ensure_flush(role).await?;
            let mut hdr = [0u8; MAX_REC_HEADER];
            let mut enc = Enc::new(&mut hdr);
            let mut codec = self.b.tracks[track as usize].codec;
            encode_record(&mut enc, &mut codec, track, head, item_len).ok_or(Error::Head)?;
            let n = enc.len();
            let total = n as u64 + item_len as u64;
            if total <= self.room() {
                self.b.tracks[track as usize].codec = codec;
                if role == Role::Data {
                    // listed in the sparse flush heads after this one, until the next full head
                    let t = &mut self.b.tracks[track as usize];
                    if !t.written {
                        t.written = true;
                        self.since_full_written += 1;
                    }
                    self.since_full_bytes += total;
                }
                self.put(&hdr[..n]).await?;
                for p in parts {
                    self.put(p).await?;
                }
                return Ok(());
            }
            if fresh {
                return Err(Error::TooBig);
            }
            // no room in this segment: pad it out and start the next one
            self.close_segment(role).await?;
            fresh = true;
        }
    }

    /// A meta record: `ev`'s header and `tail` (a bundle's bytes) as the item, written at once.
    async fn write_meta(&mut self, ev: MetaEvent<'_>, tail: &[u8]) -> Result<(), Error<M::Error>> {
        let mut tmp = [0u8; 256];
        let mut enc = Enc::new(&mut tmp);
        ev.encode(&mut enc).ok_or(Error::Head)?;
        let n = enc.len();
        let head = StreamHead {
            pos: self.meta_next,
            units: 1,
            lost: None,
            t: self.t_last,
            ext: None,
        };
        let hdr = tmp;
        self.write_record(Role::Meta, 0, &head, &[&hdr[..n], tail])
            .await?;
        self.meta_written();
        self.end_flush().await
    }

    /// A meta record was written: the next one gets the next position.
    fn meta_written(&mut self) {
        self.meta_next += 1;
        #[cfg(debug_assertions)]
        {
            self.meta_since_sb += 1;
            debug_assert!(
                self.meta_since_sb <= crate::format::META_SLACK,
                "more than META_SLACK meta records between two superblock writes"
            );
        }
    }

    /// The session's meta record: its header, then the track entries as the item's tail.
    async fn write_meta_session(&mut self) -> Result<(), Error<M::Error>> {
        // entries go through the stage after the header: assemble the record in pieces
        let n = self.n_tracks;
        let mut hdr = [0u8; 512];
        let mut enc = Enc::new(&mut hdr);
        MetaEvent::Session {
            session: self.session,
            api_root: api_root_id(&self.info.api),
            api: self.info.api,
            device_id: self.info.device_id,
            fw: self.info.fw,
            hw: self.info.hw,
            bundle_hash: self.sb.bundle_hash,
            tracks: (n - 1) as u32,
        }
        .encode(&mut enc)
        .ok_or(Error::Head)?;
        let hn = enc.len();
        // size the item: encode the entries once to count
        let mut entries_len = 0usize;
        for i in 1..n {
            let mut tmp = [0u8; MAX_TRACK_ENTRY + 10];
            let mut e = Enc::new(&mut tmp);
            self.entry(i).encode(&mut e).ok_or(Error::Session)?;
            entries_len += e.len();
        }
        let head = StreamHead {
            pos: self.meta_next,
            units: 1,
            lost: None,
            t: self.t_last,
            ext: None,
        };
        // the record header, then the pieces, with the room checked for the whole item
        let item_len = hn + entries_len;
        let mut fresh = false;
        loop {
            self.ensure_flush(Role::Meta).await?;
            let mut rh = [0u8; MAX_REC_HEADER];
            let mut e = Enc::new(&mut rh);
            let mut codec = self.b.tracks[0].codec;
            encode_record(&mut e, &mut codec, 0, &head, item_len).ok_or(Error::Head)?;
            let rn = e.len();
            if (rn + item_len) as u64 <= self.room() {
                self.b.tracks[0].codec = codec;
                self.put(&rh[..rn]).await?;
                self.put(&hdr[..hn]).await?;
                for i in 1..n {
                    let mut tmp = [0u8; MAX_TRACK_ENTRY + 10];
                    let mut e = Enc::new(&mut tmp);
                    self.entry(i).encode(&mut e).ok_or(Error::Session)?;
                    let len = e.len();
                    self.put(&tmp[..len]).await?;
                }
                break;
            }
            if fresh {
                return Err(Error::TooBig);
            }
            self.close_segment(Role::Meta).await?;
            fresh = true;
        }
        self.meta_written();
        self.end_flush().await
    }

    fn entry(&self, i: usize) -> TrackEntry<'a> {
        let t = &self.b.tracks[i];
        TrackEntry {
            number: i as u32,
            source: 0,
            key: t.key,
            desc: self.info.tracks[i - 1],
            released: 0,
            stream_session: t.stream_session,
            stream_start: t.stream_start,
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Superblock

    async fn read_superblock(&mut self, slot: u64) -> Result<Option<Superblock>, Error<M::Error>> {
        let off = slot * self.geo.seg / 2;
        self.medium
            .read(off, &mut self.scratch)
            .await
            .map_err(Error::Medium)?;
        self.reads += 1;
        Ok(Superblock::decode(&self.scratch))
    }

    async fn write_superblock(&mut self) -> Result<(), Error<M::Error>> {
        self.sb.log_id = self.geo.log_id;
        self.sb.meta_next = self.meta_next;
        let off = (self.sb.write_seq % 2) * self.geo.seg / 2;
        if M::NEEDS_ERASE {
            self.medium
                .erase(off, off + round_up(SB_LEN as u64, M::ERASE_SIZE as u64))
                .await
                .map_err(Error::Medium)?;
        }
        let b = self.sb.encode();
        self.medium.write(off, &b).await.map_err(Error::Medium)?;
        self.sb.write_seq += 1;
        #[cfg(debug_assertions)]
        {
            self.meta_since_sb = 0;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------------
    // Mount and recovery

    /// A new log: fresh id, counters carried on from `old` so nothing of an earlier log reads as this one.
    async fn start_log(&mut self, old: Option<Superblock>) -> Result<(), Error<M::Error>> {
        let (write_seq, log_id) = match old {
            Some(o) => (o.write_seq + 1, o.log_id.wrapping_add(1)),
            None => (0, 0),
        };
        self.geo.log_id = fnv1a(&[
            &self.cfg.seed.to_le_bytes(),
            &log_id.to_le_bytes(),
            &self.medium.capacity().to_le_bytes(),
        ]) as u32
            | 1;
        self.sb.write_seq = write_seq;
        self.recovery = Recovery {
            formatted: true,
            ..Recovery::default()
        };
        self.start_session().await
    }

    async fn recover(&mut self, sb: Superblock) -> Result<(), Error<M::Error>> {
        self.geo.log_id = sb.log_id;
        self.sb = sb;
        self.sb.write_seq += 1;
        let mut rec = Recovery::default();
        // where meta positions go on, also when the meta ring holds no record yet; the scan below finds it, never
        // behind what the superblock says (a mount that didn't parse the tail may have skipped ahead)
        self.meta_next = sb.meta_next;

        // the meta tail: the last segment with a block 0, and the end of what it holds; appended to
        if sb.meta_tail_seq != NONE {
            let (seq, _walked, torn) = self.find_tail(Role::Meta, sb.meta_tail_seq).await?;
            rec.torn_blocks += torn;
            if let Some(seq) = seq {
                let (end, torn) = self.find_end(Role::Meta, seq).await?;
                rec.torn_blocks += torn;
                // appended to, unless a torn block is in the way or the medium needs erasing (the blocks after
                // the end may be half programmed): then the next meta record opens the next segment
                self.meta = Ring {
                    seq,
                    open: !M::NEEDS_ERASE && torn == 0 && end < self.geo.blocks,
                    next_seq: seq + 1,
                    block: end,
                };
                // positions: the last flush of the meta tail
                let scanned = self.scan_last_flush(Role::Meta, seq, end, &mut rec).await?;
                self.meta_next = scanned.max(sb.meta_next);
            } else {
                self.meta.next_seq = sb.meta_tail_seq;
            }
        }
        // the data tail: positions of every track from its last flush; a new segment is opened for this session
        if sb.data_tail_seq != NONE {
            let (seq, walked, torn) = self.find_tail(Role::Data, sb.data_tail_seq).await?;
            rec.segments_walked = walked;
            rec.torn_blocks += torn;
            if let Some(seq) = seq {
                let (end, torn) = self.find_end(Role::Data, seq).await?;
                rec.torn_blocks += torn;
                self.data.next_seq = seq + 1;
                self.data.seq = seq;
                self.scan_last_flush(Role::Data, seq, end, &mut rec).await?;
            } else {
                // opened, never written (a cut before the mount's sync): its slot is used again
                self.data.next_seq = sb.data_tail_seq;
            }
        }
        rec.reads += self.reads;
        self.recovery = rec;
        for (i, t) in self.b.tracks.iter().enumerate() {
            self.b.sealed_next[i] = t.next;
            self.b.durable_next[i] = t.next;
            self.b.sealed_newest[i] = t.newest;
            self.b.durable_newest[i] = t.newest;
        }
        self.start_session().await
    }

    /// The last segment of `role` with a valid block 0, from the superblock's hint: `hint` itself, else the one
    /// before, then forward while the next one has a block 0. `None` when none has. Also how many segments were
    /// walked past the hint and how many block 0s were torn.
    async fn find_tail(
        &mut self,
        role: Role,
        hint: u64,
    ) -> Result<(Option<u64>, u32, u32), Error<M::Error>> {
        let mut tail = None;
        let mut torn = 0;
        for cand in [Some(hint), hint.checked_sub(1)].into_iter().flatten() {
            match self.block_ok(role, cand, 0).await? {
                BlockCheck::Ok { .. } => {
                    tail = Some(cand);
                    break;
                }
                BlockCheck::Torn => torn += 1,
                BlockCheck::Other => {}
            }
        }
        let Some(mut seq) = tail else {
            return Ok((None, 0, torn));
        };
        if seq != hint {
            // the hint's block 0 isn't there: nothing after it can be
            return Ok((Some(seq), 0, torn));
        }
        let mut walked = 0u32;
        let n = self.geo.count(role);
        while (walked as u64) < n {
            match self.block_ok(role, seq + 1, 0).await? {
                BlockCheck::Ok { .. } => {
                    seq += 1;
                    walked += 1;
                }
                BlockCheck::Torn => {
                    torn += 1;
                    break;
                }
                BlockCheck::Other => break,
            }
        }
        Ok((Some(seq), walked, torn))
    }

    async fn block_ok(
        &mut self,
        role: Role,
        seq: u64,
        index: u32,
    ) -> Result<BlockCheck, Error<M::Error>> {
        let at = self.geo.base(role, seq) + index as u64 * BLOCK as u64;
        self.medium
            .read(at, &mut self.scratch)
            .await
            .map_err(Error::Medium)?;
        self.reads += 1;
        Ok(check_block(
            &self.scratch,
            self.geo.log_id,
            role,
            seq,
            index,
        ))
    }

    /// The end of segment `seq`: the index after its last valid block (a binary search, then a few probes past
    /// it), and how many blocks right after the end carry its tag with a bad CRC.
    async fn find_end(&mut self, role: Role, seq: u64) -> Result<(u32, u32), Error<M::Error>> {
        // block 0 is valid; find the last valid one assuming the written blocks form a prefix
        let (mut lo, mut hi) = (0u32, self.geo.blocks);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            match self.block_ok(role, seq, mid).await? {
                BlockCheck::Ok { .. } => lo = mid,
                _ => hi = mid,
            }
        }
        let mut end = lo + 1;
        let mut torn = 0;
        // a damaged block inside the prefix can mislead the search: probe a few past the end
        let mut i = end;
        while i < self.geo.blocks && i < end + 4 {
            match self.block_ok(role, seq, i).await? {
                BlockCheck::Ok { .. } => end = i + 1,
                BlockCheck::Torn => torn += 1,
                BlockCheck::Other => {}
            }
            i += 1;
        }
        Ok((end, torn))
    }

    /// Parse the last flush of segment `seq` (`end` blocks long) to learn where every track's positions got to,
    /// and the segment's track table for the tracks' released positions. Returns track 0's next position and
    /// sets the data tracks' `next`/`released` by key. Malformed content never fails the mount: what was parsed
    /// before it counts, the rest is reported as a torn record.
    async fn scan_last_flush(
        &mut self,
        role: Role,
        seq: u64,
        end: u32,
        rec: &mut Recovery,
    ) -> Result<u64, Error<M::Error>> {
        let last = end - 1;
        let flush_off = match self.block_ok(role, seq, last).await? {
            BlockCheck::Ok { flush_off } => flush_off as u32,
            _ => 0,
        };
        let start = last.saturating_sub(flush_off);
        let base = self.geo.base(role, seq);
        let blocks = self.geo.blocks;
        let log_id = self.geo.log_id;
        let n_tracks = self.n_tracks;
        // the meta ring's one track goes through `meta`, the data ring's into this session's tracks: the tail
        // segment's track `old` is this session's `self.b.tracks[i]` by key (`tail_map`, see [Track])
        let mut meta = Track::default();
        for t in self.b.tracks.iter_mut() {
            t.tail_map = NO_TRACK;
        }
        self.b.tracks[0].tail_map = 0;
        let mut tail_n = 1usize;
        let mut torn = false;
        // payload bytes before the segment's first flush head
        let mut first_off = 0usize;

        // block 0: the segment head and the track table
        let mut stream = BlockStream::new(
            &mut self.medium,
            &mut self.scratch,
            base,
            log_id,
            role,
            seq,
            blocks,
            0,
        );
        let n = stream
            .peek(&mut self.peek[..MAX_SEG_HEAD])
            .await
            .map_err(Error::Medium)?;
        let mut d = Dec::new(&self.peek[..n]);
        match SegHead::decode(&mut d) {
            Some(sh) => {
                tail_n = sh.tracks as usize + 1;
                stream.skip(d.pos()).await.map_err(Error::Medium)?;
                first_off += d.pos();
                for _ in 0..sh.tracks {
                    let n = stream
                        .peek(&mut self.peek[..MAX_TRACK_ENTRY + 10])
                        .await
                        .map_err(Error::Medium)?;
                    let mut d = Dec::new(&self.peek[..n]);
                    let Some(e) = TrackEntry::decode(&mut d) else {
                        torn = true;
                        break;
                    };
                    if role == Role::Data
                        && let Some(i) = self.b.tracks[1..n_tracks]
                            .iter()
                            .position(|t| t.key == e.key)
                    {
                        let t = &mut self.b.tracks[i + 1];
                        t.released = e.released;
                        t.stream_session = e.stream_session;
                        t.stream_start = e.stream_start;
                        t.known = true;
                        t.tail_number = e.number;
                        if let Some(o) = self.b.tracks.get_mut(e.number as usize) {
                            o.tail_map = (i + 1) as u32;
                        }
                    }
                    stream.skip(d.pos()).await.map_err(Error::Medium)?;
                    first_off += d.pos();
                }
            }
            None => torn = true,
        }
        let tracks = &mut self.b.tracks[..n_tracks];
        // this session's track for the tail segment's track `old`
        let slot = |tracks: &[Track], old: u32| -> Option<usize> {
            if old as usize >= tail_n {
                return None;
            }
            match tracks.get(old as usize) {
                Some(t) if t.tail_map != NO_TRACK => Some(t.tail_map as usize),
                // numbers past this session's tracks: look the track up
                None => tracks.iter().position(|t| t.known && t.tail_number == old),
                Some(_) => None,
            }
        };
        // the last flush's head says where its full head is; positions come from that one, then from the last
        // flush's own head when it is sparse
        let mut full_at = 0u32;
        if !torn && start != 0 {
            rec.reads += stream.reads();
            stream = BlockStream::new(
                &mut self.medium,
                &mut self.scratch,
                base,
                log_id,
                role,
                seq,
                blocks,
                start,
            );
            let n = stream
                .peek(&mut self.peek[..MAX_FLUSH_HEAD])
                .await
                .map_err(Error::Medium)?;
            let mut d = Dec::new(&self.peek[..n]);
            match (d.u8(), FlushHead::decode(&mut d)) {
                (Some(KIND_FLUSH), Some(fh)) if fh.full_back <= start => {
                    full_at = start - fh.full_back
                }
                _ => torn = true,
            }
        }
        if !torn && start != 0 && full_at != start {
            // the full head: the segment's first (right after the track table) or a later flush's
            let (from, skip) = if full_at == 0 {
                ((first_off / PAYLOAD) as u32, first_off % PAYLOAD)
            } else {
                (full_at, 1)
            };
            rec.reads += stream.reads();
            stream = BlockStream::new(
                &mut self.medium,
                &mut self.scratch,
                base,
                log_id,
                role,
                seq,
                blocks,
                from,
            );
            torn = !stream.skip(skip).await.map_err(Error::Medium)?;
        } else if !torn && start != 0 {
            stream.skip(1).await.map_err(Error::Medium)?;
        }
        if !torn {
            let fh = stream
                .flush_head(|old, next| {
                    if old as usize >= tail_n {
                        return false;
                    }
                    let t = match role {
                        Role::Meta => (old == 0).then_some(&mut meta),
                        Role::Data => slot(tracks, old).map(|i| &mut tracks[i]),
                    };
                    if let Some(t) = t {
                        t.next = next;
                        t.newest = None;
                    }
                    true
                })
                .await
                .map_err(Error::Medium)?;
            torn = !fh.is_some_and(|fh| fh.full_back == 0);
        }
        if !torn && start != full_at {
            rec.reads += stream.reads();
            stream = BlockStream::new(
                &mut self.medium,
                &mut self.scratch,
                base,
                log_id,
                role,
                seq,
                blocks,
                start,
            );
            stream.skip(1).await.map_err(Error::Medium)?;
            // the last flush's sparse head: the tracks written since the full one
            let fh = stream
                .flush_head(|old, next| {
                    if old as usize >= tail_n {
                        return false;
                    }
                    let t = match role {
                        Role::Meta => (old == 0).then_some(&mut meta),
                        Role::Data => slot(tracks, old).map(|i| &mut tracks[i]),
                    };
                    if let Some(t) = t {
                        t.next = next;
                    }
                    true
                })
                .await
                .map_err(Error::Medium)?;
            torn = fh.is_none();
        }
        // the records of the last flush
        if !torn {
            meta.codec.reset(meta.next);
            for t in tracks.iter_mut() {
                t.codec.reset(t.next);
            }
            loop {
                let n = stream
                    .peek(&mut self.peek[..MAX_REC_HEADER])
                    .await
                    .map_err(Error::Medium)?;
                if n == 0 {
                    break; // the end of what was written
                }
                let mut d = Dec::new(&self.peek[..n]);
                let (track, header, item_len) = match skip_record(&mut d.clone()) {
                    Some(Skipped::Padding) => {
                        stream.skip_block();
                        continue;
                    }
                    Some(Skipped::Record {
                        track,
                        header,
                        item_len,
                    }) if (track as usize) < tail_n => (track, header, item_len),
                    _ => {
                        torn = true;
                        break;
                    }
                };
                let t = match role {
                    Role::Meta => (track == 0).then_some(&mut meta),
                    Role::Data => slot(tracks, track).map(|i| &mut tracks[i]),
                };
                let head = match t {
                    Some(t) => match decode_record_with(&mut d, |_| Some(&mut t.codec)) {
                        Some(Decoded::Record { head, .. }) => Some((t, head)),
                        _ => {
                            torn = true;
                            break;
                        }
                    },
                    None => None,
                };
                stream.skip(header).await.map_err(Error::Medium)?;
                // a record counts once its item is all there
                if !stream.skip(item_len).await.map_err(Error::Medium)? {
                    torn = true;
                    break;
                }
                if let Some((t, head)) = head {
                    t.next = head.end();
                    t.newest = Some(head.pos);
                }
            }
        }
        rec.reads += stream.reads();
        if torn {
            rec.torn_records += 1;
        }
        Ok(match role {
            Role::Meta => meta.next,
            Role::Data => self.b.tracks[0].next,
        })
    }

    /// Open this boot's session: a new session number, the meta records, the data segment with the session
    /// start record, everything synced.
    async fn start_session(&mut self) -> Result<(), Error<M::Error>> {
        self.session = self.sb.next_session;
        self.sb.next_session += 1;
        self.recovery.session = self.session;
        // a track the tail segment didn't name starts its positions at 0 in this session's segment
        let first_seq = self.data.next_seq;
        for t in self.b.tracks.iter_mut() {
            t.records = 0;
            t.bytes = 0;
            if !t.known {
                t.stream_start = first_seq;
            }
        }
        // the bundle, once per hash while its meta record is still in the meta ring
        let hash = self.info.bundle.map(|b| fnv1a(&[b])).unwrap_or(0);
        let stored = self.sb.bundle_hash == hash
            && self.sb.bundle_meta_seq != NONE
            && self.meta.next_seq.saturating_sub(self.geo.meta_n) <= self.sb.bundle_meta_seq;
        if let Some(b) = self.info.bundle
            && !stored
        {
            if b.len() as u64 + 64 > self.geo.seg / BLOCK as u64 * PAYLOAD as u64 {
                return Err(Error::TooBig);
            }
            self.write_meta(
                MetaEvent::Bundle {
                    hash,
                    len: b.len() as u32,
                },
                b,
            )
            .await?;
            self.sb.bundle_meta_seq = self.meta.seq;
        }
        self.sb.bundle_hash = hash;
        self.write_meta_session().await?;
        self.write_superblock().await?;
        // the data segment of this session
        self.close_segment(Role::Data).await?;
        let rec = self.recovery;
        self.push_system(SystemEvent::SessionStart {
            session: self.session,
            torn_blocks: rec.torn_blocks,
            torn_records: rec.torn_records,
            formatted: rec.formatted,
            positions_restarted: rec.positions_restarted,
            device_id: self.info.device_id,
            fw: self.info.fw,
            hw: self.info.hw,
            bundle_hash: hash,
        })
        .await?;
        self.sync().await
    }
}
