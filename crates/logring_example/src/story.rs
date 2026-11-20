//! The runs behind every number of the book's guide (feature `std`): the drive's first minute, flush sizes, a power
//! cut, two boots with their recordings, an SD card, and a ring that goes round. `examples/story.rs` prints them,
//! `logring_figures` draws from them, and a test checks that the pages quote what they print.

extern crate std;

use std::collections::BTreeMap;
use std::vec;
use std::vec::Vec;

use exfat_lite::{BlockDevice, SECTOR_SIZE};
use logring::exfat::{CardGeometry, CardInfo, CheckOptions, FormatOptions, Opened, SdStatus, open};
use logring::format::{BLOCK, Dec, PAYLOAD};
use logring::scan::{Scan, SegmentScan};
use logring::system::SystemEvent;
use logring::tests_support::block_on;
use logring::{Buffers, Config, FullPolicy, LogMedium, Recovery};
use ww_api::shrink_wrap::DeserializeShrinkWrap;

use crate::api::CanFrame;
use crate::bench::{self, Bench, Totals};
use crate::card::{CardError, SIM_CARD_BYTES};
use crate::drive::{self, Error, track};
use crate::sd::{self, Mounted, SdError, mount_sd};
use crate::{Drive, SimCard};

/// Records per track in a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub system: u32,
    pub log: u32,
    pub mode: u32,
    pub can0: u32,
    pub vbat: u32,
}

impl Counts {
    /// The records of `scan`, per track.
    pub fn of(scan: &Scan) -> Counts {
        let mut c = Counts::default();
        for (_, r) in scan.records() {
            match r.track {
                0 => c.system += 1,
                track::LOG => c.log += 1,
                track::MODE => c.mode += 1,
                track::CAN0 => c.can0 += 1,
                track::VBAT => c.vbat += 1,
                _ => {}
            }
        }
        c
    }
}

/// Run `bench` on `card` in one session with `cfg`; the error, if the run stopped on one, and what it pushed.
pub fn boot<M: LogMedium>(card: M, cfg: Config, bench: Bench) -> (Totals, Option<Error<M::Error>>) {
    let mut bufs = Buffers::new();
    let mut n = Totals::default();
    let r = block_on(async {
        let mut drive = Drive::mount(card, &mut bufs, cfg).await?;
        bench::run_counting(&mut drive, bench, &mut n).await
    });
    (n, r.err())
}

/// The scan of the ring on a raw simulated card.
pub fn scan(bytes: &mut [u8], cfg: Config) -> Scan {
    block_on(Scan::open(&mut SimCard::new(bytes), Some(cfg.segment_size)))
        .expect("the ring the example wrote scans")
}

/// A flush as the trailers of a segment's blocks show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flush {
    pub first_block: u32,
    pub blocks: u32,
    pub records: usize,
    /// Payload bytes of its last block that hold data; the rest is padding.
    pub last_used: usize,
}

/// The block offset on the card of data segment `seg`.
pub fn segment_base(scan: &Scan, seq: u64) -> usize {
    let sb = &scan.superblock;
    let slot = 1 + u64::from(sb.meta_segments) + seq % u64::from(sb.data_segments);
    usize::try_from(slot * u64::from(sb.segment_size)).expect("a simulated card fits memory")
}

/// The flushes of `seg`, from the trailers of its blocks in `card` and the records the scan found in them.
pub fn flushes(card: &[u8], scan: &Scan, seg: &SegmentScan) -> Vec<Flush> {
    let base = segment_base(scan, seg.seq);
    let mut out: Vec<Flush> = Vec::new();
    for b in 0..seg.blocks {
        let at = base + b as usize * BLOCK + PAYLOAD + 2;
        let flush_off = u16::from_le_bytes([card[at], card[at + 1]]);
        if flush_off == 0 {
            out.push(Flush {
                first_block: b,
                blocks: 0,
                records: 0,
                last_used: 0,
            });
        }
        let f = out.last_mut().expect("block 0 starts a flush");
        assert_eq!(
            u32::from(flush_off),
            b - f.first_block,
            "block {b}'s flush_off"
        );
        f.blocks += 1;
    }
    assert_eq!(
        out.len(),
        seg.flushes as usize,
        "flushes by trailers and by the scan"
    );
    for r in &seg.records {
        let f = &mut out[r.flush as usize];
        f.records += 1;
        let end = r.at.0 as usize * PAYLOAD + r.at.1 + r.item.len();
        let last = (f.first_block + f.blocks - 1) as usize;
        f.last_used = f.last_used.max(end.saturating_sub(last * PAYLOAD));
    }
    out
}

/// The drive's first minute on the simulated card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstMinute {
    pub totals: Totals,
    /// Data segments the minute filled or began.
    pub segments: usize,
    /// Segment size, bytes.
    pub segment_bytes: u32,
    pub flushes: u32,
    /// Bytes of the data ring the minute wrote: whole blocks.
    pub data_bytes: u64,
    /// The first frame's encoded size: what `can_frame` hands the ring.
    pub frame_item_bytes: usize,
}

/// [FirstMinute] with the drive's own config.
pub fn first_minute() -> FirstMinute {
    let cfg = drive::config();
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let (totals, err) = boot(SimCard::new(&mut bytes), cfg, Bench::FIRST_MINUTE);
    assert_eq!(err, None, "the first minute runs");
    let scan = scan(&mut bytes, cfg);
    let blocks: u64 = scan.data.iter().map(|s| u64::from(s.blocks)).sum();
    let frame = scan
        .records()
        .find(|(_, r)| r.track == track::CAN0)
        .map(|(_, r)| r.item.len())
        .expect("a frame");
    FirstMinute {
        totals,
        segments: scan.data.len(),
        segment_bytes: cfg.segment_size,
        flushes: scan.data.iter().map(|s| s.flushes).sum(),
        data_bytes: blocks * BLOCK as u64,
        frame_item_bytes: frame,
    }
}

/// The first minute with one `flush_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushRow {
    pub flush_bytes: u32,
    pub flushes: u32,
    /// Of them, flushes the tick wrote: the flush interval was up before the flush was full.
    pub ticked: u32,
    /// Bytes of the data ring the minute wrote.
    pub data_bytes: u64,
    /// Bytes of padding in them: the rest of a flush's last block.
    pub padding_bytes: u64,
    /// The most frames that waited in RAM at once: what a power cut at the worst moment costs `can0`.
    pub frames_in_ram_max: u32,
}

/// The flush sizes the table of "Flushes and power cuts" compares.
pub const FLUSH_SIZES: [u32; 4] = [2 << 10, 8 << 10, 16 << 10, 32 << 10];

/// The first minute once per size of [FLUSH_SIZES].
pub fn flush_sizes() -> Vec<FlushRow> {
    FLUSH_SIZES
        .iter()
        .map(|&flush_bytes| {
            let cfg = Config {
                flush_bytes,
                ..drive::config()
            };
            let mut bytes = vec![0u8; SIM_CARD_BYTES];
            let (totals, err) = boot(SimCard::new(&mut bytes), cfg, Bench::FIRST_MINUTE);
            assert_eq!(err, None, "the first minute runs");
            let scan = scan(&mut bytes, cfg);
            let mut padding = 0;
            for seg in &scan.data {
                for f in flushes(&bytes, &scan, seg) {
                    padding += (PAYLOAD - f.last_used) as u64;
                }
            }
            let blocks: u64 = scan.data.iter().map(|s| u64::from(s.blocks)).sum();
            FlushRow {
                flush_bytes,
                flushes: scan.data.iter().map(|s| s.flushes).sum(),
                ticked: totals.ticked,
                data_bytes: blocks * BLOCK as u64,
                padding_bytes: padding,
                frames_in_ram_max: totals.frames_in_ram_max,
            }
        })
        .collect()
}

/// The power cut of "Flushes and power cuts": the power fails in the middle of a block, during the first minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCut {
    /// Bytes the card wrote before its power failed.
    pub budget: u64,
    /// What the drive had pushed when the power failed.
    pub pushed: Totals,
    /// What the next mount found.
    pub recovery: Recovery,
    /// Records on the card after the next mount.
    pub on_card: Counts,
    /// Bench time of the newest frame on the card, ms.
    pub last_frame_ms: u64,
    /// The seq of the segment the cut tore, and of the segment the next mount opened.
    pub torn_seq: u64,
    pub next_seq: u64,
    /// The card's bytes as the power cut left them, and after the next mount.
    pub cut: Vec<u8>,
    pub remounted: Vec<u8>,
}

/// Bytes the card of [power_cut] writes before the power fails: the first mount and part of the bench, and
/// 200 bytes of the next block.
pub const POWER_CUT_BUDGET: u64 = 1280 * 512 + 200;

/// [PowerCut]: the first minute cut at [POWER_CUT_BUDGET], then the next boot's mount.
pub fn power_cut() -> PowerCut {
    let cfg = drive::config();
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let (pushed, err) = boot(
        SimCard::power_fails_after(&mut bytes, POWER_CUT_BUDGET),
        cfg,
        Bench::FIRST_MINUTE,
    );
    assert_eq!(
        err,
        Some(Error::Ring(logring::Error::Medium(CardError::PowerCut))),
        "the power fails during the run"
    );
    let cut = bytes.clone();
    let torn_seq = scan(&mut bytes, cfg).data.last().expect("a segment").seq;
    let mut bufs = Buffers::new();
    let recovery = block_on(async {
        let drive = Drive::mount(SimCard::new(&mut bytes), &mut bufs, cfg).await?;
        Ok::<_, Error<CardError>>(drive.logring().recovery())
    })
    .expect("the next boot mounts");
    let scan = scan(&mut bytes, cfg);
    let last = scan
        .records()
        .filter(|(_, r)| r.track == track::CAN0)
        .last()
        .map(|(_, r)| CanFrame::from_ww_bytes(&r.item).expect("a frame"))
        .expect("frames on the card");
    let n = u32::from_le_bytes([last.data[2], last.data[3], last.data[4], last.data[5]]);
    PowerCut {
        budget: POWER_CUT_BUDGET,
        pushed,
        recovery,
        on_card: Counts::of(&scan),
        last_frame_ms: u64::from(n) * 1000 / u64::from(Bench::FIRST_MINUTE.can_hz),
        torn_seq,
        next_seq: scan.data.last().expect("a segment").seq,
        cut,
        remounted: bytes,
    }
}

/// What the system track of a ring says, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemRecord {
    SessionStart { session: u32 },
    RecordingStart { id: u32, trigger: u8 },
    RecordingStop { id: u32, reason: u8 },
    Marker { kind: u8, value: Option<i64> },
}

/// Two boots of the drive: a run that faults, a reset, and a run that ends as planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoBoots {
    pub first: Totals,
    pub second: Totals,
    /// The sessions of the data segments, in seq order.
    pub segment_sessions: Vec<u32>,
    /// The position of the second boot's first frame.
    pub second_first_frame_pos: u64,
    pub system: Vec<SystemRecord>,
}

/// The first boot of [two_boots]: 20 seconds, a fault at second 12.
pub const BOOT_1: Bench = Bench {
    seconds: 20,
    can_hz: 1000,
    fault_at_s: Some(12),
};
/// The second boot: 30 seconds without a fault.
pub const BOOT_2: Bench = Bench {
    seconds: 30,
    can_hz: 1000,
    fault_at_s: None,
};

/// [TwoBoots] on one card.
pub fn two_boots() -> TwoBoots {
    let cfg = drive::config();
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let (first, err) = boot(SimCard::new(&mut bytes), cfg, BOOT_1);
    assert_eq!(err, None, "boot 1 runs");
    let (second, err) = boot(SimCard::new(&mut bytes), cfg, BOOT_2);
    assert_eq!(err, None, "boot 2 runs");
    let scan = scan(&mut bytes, cfg);
    let mut system = Vec::new();
    let mut second_first_frame_pos = None;
    for (seg, r) in scan.records() {
        if r.track == track::CAN0 && seg.session == 2 && second_first_frame_pos.is_none() {
            second_first_frame_pos = Some(r.head.pos);
        }
        if r.track != 0 {
            continue;
        }
        let ev = SystemEvent::decode(&mut Dec::new(&r.item)).expect("a system record");
        system.push(match ev {
            SystemEvent::SessionStart { session, .. } => SystemRecord::SessionStart { session },
            SystemEvent::RecordingStart { id, trigger } => {
                SystemRecord::RecordingStart { id, trigger }
            }
            SystemEvent::RecordingStop { id, reason } => SystemRecord::RecordingStop { id, reason },
            SystemEvent::Marker { kind, value, .. } => SystemRecord::Marker { kind, value },
            other => panic!("the drive writes no {other:?}"),
        });
    }
    TwoBoots {
        first,
        second,
        segment_sessions: scan.data.iter().map(|s| s.session).collect(),
        second_first_frame_pos: second_first_frame_pos.expect("a frame of boot 2"),
        system,
    }
}

/// A card that keeps only the sectors written to it: an SD card of real size in little RAM.
pub struct SparseCard {
    sectors: u64,
    written: BTreeMap<u64, [u8; SECTOR_SIZE]>,
}

impl SparseCard {
    /// A new card of `bytes`, all zeroes.
    pub fn new(bytes: u64) -> Self {
        SparseCard {
            sectors: bytes / SECTOR_SIZE as u64,
            written: BTreeMap::new(),
        }
    }
}

/// An access past the end of a [SparseCard].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PastEnd;

impl BlockDevice for SparseCard {
    type Error = PastEnd;

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), PastEnd> {
        for (i, s) in buf.chunks_mut(SECTOR_SIZE).enumerate() {
            let at = lba + i as u64;
            if at >= self.sectors {
                return Err(PastEnd);
            }
            match self.written.get(&at) {
                Some(d) => s.copy_from_slice(d),
                None => s.fill(0),
            }
        }
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), PastEnd> {
        for (i, s) in buf.chunks(SECTOR_SIZE).enumerate() {
            let at = lba + i as u64;
            if at >= self.sectors {
                return Err(PastEnd);
            }
            let mut d = [0u8; SECTOR_SIZE];
            d.copy_from_slice(s);
            self.written.insert(at, d);
        }
        Ok(())
    }
}

/// The drive's SD card: 32 GB, an allocation unit of 4 MiB.
pub const CARD_BYTES: u64 = 32_000_000_000;
/// The allocation unit the card reports.
pub const CARD_AU_BYTES: u32 = 4 << 20;

/// What the card reports about itself: its allocation unit.
pub fn card_info() -> CardInfo {
    CardInfo {
        sd_status: Some(SdStatus {
            au_bytes: CARD_AU_BYTES,
            ..SdStatus::default()
        }),
        cid: None,
    }
}

/// The drive's card after a format and a minute of the bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdCard {
    pub card_bytes: u64,
    pub geometry: CardGeometry,
    /// How the first and the second boot mounted.
    pub first: Mounted,
    pub second: Mounted,
    /// Where the ring file starts on the card, bytes, and its length.
    pub ring_start: u64,
    pub ring_bytes: u64,
    pub rw_bytes: u64,
    pub segment_bytes: u32,
    pub meta_segments: u32,
    pub data_segments: u32,
    /// The data ring's bytes the first minute wrote.
    pub minute_bytes: u64,
    /// Days of the bench the data ring holds, in tenths, rounded.
    pub days_tenths: u64,
}

/// [SdCard]: a new card of [CARD_BYTES] in the drive, a minute of the bench, and the next boot.
pub fn sd_card() -> SdCard {
    let mut card = SparseCard::new(CARD_BYTES);
    // a card simulated in RAM takes no time to write: nothing for the write-size sweep to measure
    let opts = FormatOptions {
        check: CheckOptions {
            sweep: false,
            ..CheckOptions::default()
        },
        ..sd::format_options(0x0042_0001)
    };
    let info = card_info();
    let mut bufs = Buffers::new();
    let (first, ring_start, ring_bytes, sb) = block_on(async {
        let (mut drive, how) =
            mount_sd(&mut card, Some(&info), &opts, &mut || 0, &mut bufs).await?;
        let totals = bench::run(&mut drive, Bench::FIRST_MINUTE)
            .await
            .map_err(SdError::Ring)?;
        assert_eq!(totals.can0, 60_000);
        let lr = drive.logring();
        let ring = lr.medium();
        Ok::<_, SdError<PastEnd>>((
            how,
            ring.start_lba() * SECTOR_SIZE as u64,
            ring.sectors() * SECTOR_SIZE as u64,
            lr.superblock(),
        ))
    })
    .expect("a minute on a new card");
    let second = block_on(async {
        let (_, how) = mount_sd(&mut card, Some(&info), &opts, &mut || 0, &mut bufs).await?;
        Ok::<_, SdError<PastEnd>>(how)
    })
    .expect("the next boot mounts");
    let Opened::Bound(bound) = block_on(open(&mut card)).expect("the card reads") else {
        panic!("the ring file binds");
    };
    let (mut ring, cfg) = bound.into_parts(drive::config());
    let scan = block_on(Scan::open(&mut ring, Some(cfg.segment_size))).expect("the ring scans");
    let minute_bytes: u64 = scan
        .data
        .iter()
        .filter(|s| s.session == 1)
        .map(|s| u64::from(s.blocks) * BLOCK as u64)
        .sum();
    let data_bytes = u64::from(sb.data_segments) * u64::from(sb.segment_size);
    SdCard {
        card_bytes: CARD_BYTES,
        geometry: CardGeometry::from_card(Some(&info)),
        first,
        second,
        ring_start,
        ring_bytes,
        rw_bytes: opts.rw_bytes,
        segment_bytes: sb.segment_size,
        meta_segments: sb.meta_segments,
        data_segments: sb.data_segments,
        minute_bytes,
        days_tenths: (data_bytes * 10 + minute_bytes * 60 * 12) / (minute_bytes * 60 * 24),
    }
}

/// A ring that goes round: the bench for [FULL_RUN] on the simulated card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingFull {
    pub data_segments: u64,
    /// The newest data seq, the oldest one the card still holds, segments overwritten.
    pub newest_seq: u64,
    pub oldest_seq: u64,
    pub segments_reused: u64,
    /// The superblock's writes.
    pub superblock_writes: u64,
    /// Bench time, ms, at which the same run with drop newest stopped, and the frames it kept.
    pub refused_at_ms: u64,
    pub kept_frames: u32,
    /// What a mount with drop newest says on the card the ten minutes with drop oldest left.
    pub switched_mount: Error<CardError>,
}

/// The bench run of [ring_full]: ten minutes.
pub const FULL_RUN: Bench = Bench {
    seconds: 600,
    can_hz: 1000,
    fault_at_s: None,
};

/// [RingFull]: [FULL_RUN] with each [FullPolicy].
pub fn ring_full() -> RingFull {
    let cfg = drive::config();
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let mut bufs = Buffers::new();
    let (data_segments, newest_seq, oldest_seq, sb) = block_on(async {
        let mut drive = Drive::mount(SimCard::new(&mut bytes), &mut bufs, cfg).await?;
        bench::run(&mut drive, FULL_RUN).await?;
        let lr = drive.logring();
        Ok::<_, Error<CardError>>((
            lr.data_segments(),
            lr.data_seq(),
            lr.oldest_data_seq(),
            lr.superblock(),
        ))
    })
    .expect("ten minutes go round the ring");

    let newest = Config {
        full: FullPolicy::DropNewest,
        ..cfg
    };
    let mut bufs = Buffers::new();
    let switched = block_on(Drive::mount(SimCard::new(&mut bytes), &mut bufs, newest))
        .err()
        .expect("drop newest does not mount a ring that went round");
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let (refused, err) = boot(SimCard::new(&mut bytes), newest, FULL_RUN);
    assert_eq!(
        err,
        Some(Error::Ring(logring::Error::Full)),
        "drop newest refuses"
    );
    let kept = Counts::of(&scan(&mut bytes, newest));
    RingFull {
        data_segments,
        newest_seq,
        oldest_seq,
        segments_reused: sb.segments_reused,
        superblock_writes: sb.write_seq + 1,
        refused_at_ms: refused.t_ms,
        kept_frames: kept.can0,
        switched_mount: switched,
    }
}
