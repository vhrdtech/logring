//! Formatting a card for the logring ([format_card]) and re-creating the ring file on a card that keeps its rw
//! files ([recreate_ring]).

use super::card::{CardGeometry, CardInfo};
use super::check::{Aliased, Clock, Sweep};
#[cfg(feature = "card-check")]
use super::check::{CheckOptions, alias_test, sweep};
use super::ring::{Binding, ExfatRing};
use super::{RING_ATTRIBUTES, RING_NAME};
use crate::Config;
use exfat_lite::{
    BlockDevice, Error as FsError, FormatParams, Geometry, NewFile, SECTOR_SIZE, Timestamp, Volume,
    attr, format,
};

/// What `README.txt` says by default.
pub const README: &[u8] = b"This card belongs to a logring device.\r\n\
\r\n\
logring.wlr is its log: a ring the device writes all the time. Leave it where it is: if it is deleted, moved,\r\n\
truncated or defragmented, the device stops logging until it re-creates it (and the log in it is gone).\r\n\
The rest of the card is free for your own files.\r\n\
\r\n\
Read the log with the logring CLI or the logring Python package: https://github.com/vhrdtech/logring\r\n";

/// How to format a card.
#[derive(Debug, Clone, Copy)]
pub struct FormatOptions<'a> {
    /// Bytes left free for rw files (default 128 MB); the ring gets the rest, in whole segments.
    pub rw_bytes: u64,
    /// Volume label (up to 11 characters): the device name.
    pub label: &'a str,
    /// Volume serial: from the device's RNG or clock, so the binding of another card never matches.
    pub serial: u32,
    /// Segments of the ring's meta area (default 4).
    pub meta_segments: u32,
    /// `README.txt`'s contents (default [README]); empty: none.
    pub readme: &'a [u8],
    /// The files' timestamps.
    pub time: Timestamp,
    /// The card check before the format (feature `card-check`).
    #[cfg(feature = "card-check")]
    pub check: CheckOptions,
}

impl<'a> FormatOptions<'a> {
    pub fn new(label: &'a str, serial: u32) -> Self {
        FormatOptions {
            rw_bytes: 128 << 20,
            label,
            serial,
            meta_segments: 4,
            readme: README,
            time: Timestamp::DEFAULT,
            #[cfg(feature = "card-check")]
            check: CheckOptions::default(),
        }
    }
}

/// What the format found out about the card, for the device's status and track 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardReport {
    /// Sectors the card reports.
    pub sectors: u64,
    pub geometry: CardGeometry,
    /// The card's own facts, as given.
    pub card: Option<CardInfo>,
    /// The sweep (all zero when it didn't run).
    pub sweep: Sweep,
    /// What goes to the ring header: the sweep's pick, or 0 when nothing was measured.
    pub card_flush_bytes: u32,
    /// The aliasing test ran and passed.
    pub alias_checked: bool,
}

/// A card formatted (or a ring file re-created): the ring's medium and what the [Logring](crate::Logring) gets.
pub struct Formatted<D: BlockDevice> {
    pub ring: ExfatRing<D>,
    /// Segment size of the ring.
    pub segment_size: u32,
    pub meta_segments: u32,
    pub report: CardReport,
}

impl<D: BlockDevice> Formatted<D> {
    /// `base` with the ring's layout, binding and measured flush size: hand it with [Formatted::ring] to
    /// [Logring::format](crate::Logring::format) to write the ring header.
    pub fn config(&self, base: Config) -> Config {
        let mut cfg = Config {
            segment_size: self.segment_size,
            meta_segments: self.meta_segments,
            binding: self.ring.binding().encode(),
            card_flush_bytes: self.report.card_flush_bytes,
            ..base
        };
        if self.report.card_flush_bytes != 0 {
            cfg.flush_bytes = self.report.card_flush_bytes;
        }
        cfg
    }

    /// The ring and its [config](Formatted::config), for [Logring::format](crate::Logring::format).
    pub fn into_parts(self, base: Config) -> (ExfatRing<D>, Config) {
        let cfg = self.config(base);
        (self.ring, cfg)
    }
}

/// Errors of [format_card] and [recreate_ring].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FormatError<E> {
    /// The block device failed.
    #[error("block device error")]
    Device(E),
    /// The aliasing test failed: a fake card (or a broken one). Not formatted; the card holds about
    /// `usable_sectors`.
    #[error("the card is not as big as it says (fake card?)")]
    FakeCard(Aliased),
    /// exFAT refused something (a damaged volume at re-create, a bad label).
    #[error("exFAT: {0:?}")]
    Fs(FsError<()>),
    /// No room for a ring of at least the meta area and two data segments next to the rw region.
    #[error("no room for the ring")]
    NoSpace,
    /// The scratch buffer is smaller than one sector.
    #[error("scratch buffer too small")]
    Scratch,
}

fn fs<E>(e: FsError<E>) -> FormatError<E> {
    match e {
        FsError::Device(d) => FormatError::Device(d),
        FsError::NoSpace => FormatError::NoSpace,
        other => FormatError::Fs(other.map_device(|_| ())),
    }
}

/// Format a card for the logring: the card check (feature `card-check`: aliasing test, write-size sweep, as
/// `CheckOptions` says; without the feature nothing is checked and `clock` is unused), then MBR +
/// exFAT with the card's allocation unit as boundary unit where it is a power of two, `README.txt`, and the ring
/// file `logring.wlr` (read-only + system + hidden, contiguous, starting on an AU boundary, whole segments) at the
/// end of the free space, `rw_bytes` left free before it. Erases the card. The ring header isn't written yet:
/// [Logring::format](crate::Logring::format) with [Formatted::config] does that.
///
/// `scratch` (at least 512 bytes) is the format's write buffer and the sweep's: the sweep measures the sizes
/// that fit it. `clock` times the sweep.
pub async fn format_card<D: BlockDevice, C: Clock>(
    mut dev: D,
    card: Option<&CardInfo>,
    opts: &FormatOptions<'_>,
    clock: &mut C,
    scratch: &mut [u8],
) -> Result<Formatted<D>, FormatError<D::Error>> {
    if scratch.len() < SECTOR_SIZE {
        return Err(FormatError::Scratch);
    }
    #[cfg(not(feature = "card-check"))]
    let _ = clock;
    let sectors = dev.sector_count();
    let geo = CardGeometry::from_card(card);
    #[cfg_attr(not(feature = "card-check"), allow(unused_mut))]
    let mut alias_checked = false;
    #[cfg(feature = "card-check")]
    if opts.check.aliasing {
        if let Some(a) = alias_test(&mut dev, opts.check.alias_points, opts.serial)
            .await
            .map_err(FormatError::Device)?
        {
            return Err(FormatError::FakeCard(a));
        }
        alias_checked = true;
    }
    #[cfg_attr(not(feature = "card-check"), allow(unused_mut))]
    let mut sw = Sweep::default();
    #[cfg(feature = "card-check")]
    if opts.check.sweep {
        // in the second half of the card: the ring goes there, the rw region before it stays as written
        sw = sweep(
            &mut dev,
            clock,
            scratch,
            sectors / 2,
            sectors,
            geo.au_sectors(),
            opts.check.sweep_bytes,
        )
        .await
        .map_err(FormatError::Device)?;
    }
    let card_flush_bytes = if sw.best_flush_bytes == 0 {
        0
    } else {
        geo.round_flush(sw.best_flush_bytes)
    };

    let mut fs_geo = Geometry::for_capacity(sectors);
    let au = geo.au_sectors();
    if au.is_power_of_two() && au > fs_geo.boundary_unit_sectors as u64 && au <= u32::MAX as u64 {
        fs_geo.boundary_unit_sectors = au as u32;
    }
    let params = FormatParams::new(fs_geo, opts.label, opts.serial);
    let n = scratch.len() / SECTOR_SIZE * SECTOR_SIZE;
    format(&mut dev, &params, &mut scratch[..n])
        .await
        .map_err(fs)?;
    let report = CardReport {
        sectors,
        geometry: geo,
        card: card.copied(),
        sweep: sw,
        card_flush_bytes,
        alias_checked,
    };
    {
        let mut vol = Volume::mount(&mut dev).await.map_err(fs)?;
        if !opts.readme.is_empty() {
            vol.create(
                &NewFile::new("README.txt")
                    .with_data(opts.readme)
                    .time(opts.time),
            )
            .await
            .map_err(fs)?;
        }
    }
    create_ring(dev, &geo, opts, report).await
}

/// Re-create the ring file on a card whose volume is fine (after [Unbound](super::Unbound)): delete
/// `logring.wlr` if there is one (whatever it is now; the log in it is lost), then create it again in the longest
/// free run as [format_card] would, `rw_bytes` of the free space left free. The rw files stay. No card check;
/// the card's measured `flush_bytes`, if any, is given back as `card_flush_bytes`.
pub async fn recreate_ring<D: BlockDevice>(
    mut dev: D,
    card: Option<&CardInfo>,
    opts: &FormatOptions<'_>,
    card_flush_bytes: u32,
) -> Result<Formatted<D>, FormatError<D::Error>> {
    let geo = CardGeometry::from_card(card);
    {
        let mut vol = Volume::mount(&mut dev).await.map_err(fs)?;
        if let Some(old) = vol.find(RING_NAME).await.map_err(fs)? {
            if old.is_dir() {
                return Err(FormatError::Fs(FsError::NotADirectory));
            }
            vol.delete(&old).await.map_err(fs)?;
        }
    }
    let report = CardReport {
        sectors: dev.sector_count(),
        geometry: geo,
        card: card.copied(),
        sweep: Sweep::default(),
        card_flush_bytes,
        alias_checked: false,
    };
    create_ring(dev, &geo, opts, report).await
}

/// Place and create the ring file: at the end of the longest free run, starting on an AU boundary (and a
/// cluster), as many whole segments as fit while `rw_bytes` of all free space stays free.
async fn create_ring<D: BlockDevice>(
    mut dev: D,
    geo: &CardGeometry,
    opts: &FormatOptions<'_>,
    report: CardReport,
) -> Result<Formatted<D>, FormatError<D::Error>> {
    let mut vol = Volume::mount(&mut dev).await.map_err(fs)?;
    let info = *vol.info();
    let free = vol.free_space().await.map_err(fs)?;
    let spc = info.sectors_per_cluster() as u64;
    let seg = geo.segment_bytes as u64 / SECTOR_SIZE as u64;
    let run_lba = info
        .cluster_lba(free.largest_run_first)
        .ok_or(FormatError::NoSpace)?;
    let run_end = run_lba + free.largest_run_len * spc;
    let keep = opts.rw_bytes.div_ceil(SECTOR_SIZE as u64);
    let ring_max =
        (free.largest_run_len * spc).min((free.free_clusters * spc).saturating_sub(keep));
    let align = lcm(geo.au_sectors().max(1), spc);
    let start = (run_end - ring_max).div_ceil(align) * align;
    let segments = run_end.saturating_sub(start) / seg;
    if segments < 1 + opts.meta_segments as u64 + 2 || start < run_lba {
        return Err(FormatError::NoSpace);
    }
    let bytes = segments * seg * SECTOR_SIZE as u64;
    let first_cluster = 2 + ((start - info.heap_lba()) / spc) as u32;
    let file = vol
        .create(
            &NewFile::new(RING_NAME)
                .size(bytes)
                .at_cluster(first_cluster)
                .attributes(RING_ATTRIBUTES | attr::ARCHIVE)
                .time(opts.time),
        )
        .await
        .map_err(fs)?;
    let clusters = file.cluster_count(info.cluster_bytes());
    let binding = Binding {
        serial: info.serial,
        first_cluster: file.first_cluster,
        clusters: clusters as u32,
        start_lba: start as u32,
    };
    Ok(Formatted {
        ring: ExfatRing::new(dev, start, bytes / SECTOR_SIZE as u64, binding),
        segment_size: geo.segment_bytes,
        meta_segments: opts.meta_segments,
        report,
    })
}

fn lcm(a: u64, b: u64) -> u64 {
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    a / x * b
}
