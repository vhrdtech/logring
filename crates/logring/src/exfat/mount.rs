//! The mount check: is the ring file still where the device put it? [open] binds an [ExfatRing] to it or says
//! why not ([Unbound]).

use super::ring::{Binding, ExfatRing};
use super::{RING_ATTRIBUTES, RING_NAME};
use crate::Config;
use crate::format::Superblock;
use exfat_lite::{BlockDevice, Error as FsError, FileInfo, SECTOR_SIZE, Volume};

/// Segment sizes a superblock's slot B is looked for at (half a segment in) when slot A doesn't decode: every
/// SD allocation unit size (16 KB to 64 MB) and the 4 MB default. Bounds the search to this many reads.
const SEGMENT_CANDIDATES: [u32; 15] = [
    16 << 10,
    32 << 10,
    64 << 10,
    128 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
    2 << 20,
    4 << 20,
    8 << 20,
    12 << 20,
    16 << 20,
    24 << 20,
    32 << 20,
    64 << 20,
];

/// Why the ring file can't be written: logging stays off until the device re-creates the ring file
/// ([recreate_ring](super::recreate_ring), keeps the rw files) or formats the card ([format_card](super::format_card)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unbound {
    /// No exFAT volume, or one this crate refuses or finds damaged (the card from a camera, a corrupt card).
    Volume(FsError<()>),
    /// No `logring.wlr` in the root directory: deleted, renamed or moved into a folder.
    Missing,
    /// `logring.wlr` is a directory.
    NotAFile,
    /// The file has a FAT chain: a PC rewrote or defragmented it, its clusters may be anywhere.
    NotContiguous,
    /// The file is empty, not whole sectors, or its valid data length differs from its length (truncated or
    /// extended by a PC).
    Size,
    /// Not all of the file's clusters are marked in the allocation bitmap: a PC could give them to other files.
    NotAllocated,
    /// The ring header (superblock A and B) doesn't decode: another file was put there, or the header was
    /// overwritten.
    NoHeader,
    /// The header is bound to another volume (its serial differs): the file was copied from another card.
    OtherCard,
    /// The file isn't where the header says: it was moved (copied elsewhere and the original deleted, a
    /// defragmenter) or replaced by a copy.
    Moved,
    /// The file has another length than the header says: truncated or extended.
    Resized,
    /// The header's layout doesn't fit in the file.
    Geometry,
}

/// What the mount check noticed that doesn't stop logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Warnings {
    /// The ring file lost read-only, system or hidden (a PC changed them): its place is still checked on every
    /// mount, so logging goes on; [Bound::restore_attributes] puts them back.
    pub attributes_changed: bool,
    /// The volume's dirty flag is set: a PC didn't unmount it cleanly (the rw files may want a check there).
    pub volume_dirty: bool,
}

/// A ring file that checked out: its medium, the header found in it and what was noticed.
pub struct Bound<D: BlockDevice> {
    /// The ring file's sectors, to hand to [Logring::mount](crate::Logring::mount) with [Bound::config].
    pub ring: ExfatRing<D>,
    /// The newest valid superblock of the ring file.
    pub superblock: Superblock,
    /// The ring file's attributes as found.
    pub attributes: u16,
    pub warnings: Warnings,
}

impl<D: BlockDevice> Bound<D> {
    /// `base` with what the ring header fixes: segment size, meta segments, the binding and the card's measured
    /// `flush_bytes` (which also becomes `flush_bytes` when the card check measured one; a setting can override
    /// it afterwards).
    pub fn config(&self, base: Config) -> Config {
        let mut cfg = Config {
            segment_size: self.superblock.segment_size,
            meta_segments: self.superblock.meta_segments,
            binding: self.ring.binding().encode(),
            card_flush_bytes: self.superblock.flush_bytes,
            ..base
        };
        if self.superblock.flush_bytes != 0 {
            cfg.flush_bytes = self.superblock.flush_bytes;
        }
        cfg
    }

    /// The ring and its [config](Bound::config), for [Logring::mount](crate::Logring::mount).
    pub fn into_parts(self, base: Config) -> (ExfatRing<D>, Config) {
        let cfg = self.config(base);
        (self.ring, cfg)
    }

    /// Put read-only, system and hidden back on the ring file (after [Warnings::attributes_changed]); other bits
    /// are kept. Writes one directory entry.
    pub async fn restore_attributes(&mut self) -> Result<(), FsError<D::Error>> {
        let binding = self.ring.binding();
        let mut vol = Volume::mount(self.ring.device()).await?;
        let file = vol.find(RING_NAME).await?.ok_or(FsError::NotFound)?;
        if file.first_cluster != binding.first_cluster {
            return Err(FsError::NotFound);
        }
        vol.set_attributes(&file, file.attributes | RING_ATTRIBUTES)
            .await?;
        self.attributes = file.attributes | RING_ATTRIBUTES;
        self.warnings.attributes_changed = false;
        Ok(())
    }
}

/// What [open] found: a ring to log to, or why not and the device back.
#[allow(clippy::large_enum_variant)] // the bound ring is the common case, and there is no heap to box it
pub enum Opened<D: BlockDevice> {
    Bound(Bound<D>),
    Unbound(Unbound, D),
}

impl<D: BlockDevice> Opened<D> {
    /// The reason when unbound.
    pub fn unbound(&self) -> Option<Unbound> {
        match self {
            Opened::Bound(_) => None,
            Opened::Unbound(u, _) => Some(*u),
        }
    }
}

/// Check the card and bind the ring file: the exFAT volume mounts (boot checksum and fields), `logring.wlr` is a
/// contiguous (`NoFatChain`) file of whole sectors whose valid length is its length, all its clusters are marked
/// in the bitmap, and its header (superblock A/B, with CRC) carries the same binding: this volume's serial, the
/// file's first cluster, cluster count and start LBA. Anything else is [Unbound], and nothing has been written.
/// Only a device error is an `Err`. Reads a few dozen sectors plus the file's bitmap range.
pub async fn open<D: BlockDevice>(mut dev: D) -> Result<Opened<D>, D::Error> {
    let found = match check_file(&mut dev).await {
        Ok(Ok(f)) => f,
        Ok(Err(u)) => return Ok(Opened::Unbound(u, dev)),
        Err(FsError::Device(e)) => return Err(e),
        Err(e) => return Ok(Opened::Unbound(Unbound::Volume(e.map_device(|_| ())), dev)),
    };
    let Found {
        binding,
        file,
        start_lba,
        dirty,
    } = found;
    let sectors = file.data_length / SECTOR_SIZE as u64;
    let Some(sb) = find_superblock(&mut dev, start_lba, sectors).await? else {
        return Ok(Opened::Unbound(Unbound::NoHeader, dev));
    };
    let stored = Binding::decode(&sb.binding);
    if stored.serial != binding.serial {
        return Ok(Opened::Unbound(Unbound::OtherCard, dev));
    }
    if stored.first_cluster != binding.first_cluster || stored.start_lba != binding.start_lba {
        return Ok(Opened::Unbound(Unbound::Moved, dev));
    }
    if stored.clusters != binding.clusters {
        return Ok(Opened::Unbound(Unbound::Resized, dev));
    }
    let need = (1 + sb.meta_segments as u64 + sb.data_segments as u64) * sb.segment_size as u64;
    if sb.segment_size == 0 || need > file.data_length {
        return Ok(Opened::Unbound(Unbound::Geometry, dev));
    }
    let attributes = file.attributes;
    Ok(Opened::Bound(Bound {
        ring: ExfatRing::new(dev, start_lba, sectors, binding),
        superblock: sb,
        attributes,
        warnings: Warnings {
            attributes_changed: attributes & RING_ATTRIBUTES != RING_ATTRIBUTES,
            volume_dirty: dirty,
        },
    }))
}

struct Found {
    binding: Binding,
    file: FileInfo,
    start_lba: u64,
    dirty: bool,
}

/// The volume and file part of [open]: everything up to the header.
async fn check_file<D: BlockDevice>(
    dev: &mut D,
) -> Result<Result<Found, Unbound>, FsError<D::Error>> {
    let mut vol = Volume::mount(dev).await?;
    let Some(file) = vol.find(RING_NAME).await? else {
        return Ok(Err(Unbound::Missing));
    };
    if file.is_dir() {
        return Ok(Err(Unbound::NotAFile));
    }
    if !file.no_fat_chain {
        return Ok(Err(Unbound::NotContiguous));
    }
    let info = *vol.info();
    if file.first_cluster == 0
        || file.data_length == 0
        || file.valid_data_length != file.data_length
        || !file.data_length.is_multiple_of(SECTOR_SIZE as u64)
    {
        return Ok(Err(Unbound::Size));
    }
    let clusters = file.cluster_count(info.cluster_bytes());
    let Some(start_lba) = info.cluster_lba(file.first_cluster) else {
        return Ok(Err(Unbound::NotAllocated));
    };
    if clusters > u32::MAX as u64 || !vol.is_allocated(file.first_cluster, clusters).await? {
        return Ok(Err(Unbound::NotAllocated));
    }
    Ok(Ok(Found {
        binding: Binding {
            serial: info.serial,
            first_cluster: file.first_cluster,
            clusters: clusters as u32,
            start_lba: start_lba as u32,
        },
        file,
        start_lba,
        dirty: info.is_dirty(),
    }))
}

/// The newest valid superblock of the ring file at `start_lba` (`sectors` long): slot A at its start, slot B half
/// a segment in, for the segment size slot A gives or, when A doesn't decode, for each of
/// [SEGMENT_CANDIDATES]. A slot B counts only where its own segment size puts it.
pub(crate) async fn find_superblock<D: BlockDevice>(
    dev: &mut D,
    start_lba: u64,
    sectors: u64,
) -> Result<Option<Superblock>, D::Error> {
    let mut buf = [0u8; SECTOR_SIZE];
    dev.read(start_lba, &mut buf).await?;
    let a = Superblock::decode(&buf);
    let mut best = a;
    let mut try_b = async |seg: u32, best: &mut Option<Superblock>| -> Result<(), D::Error> {
        let off = seg as u64 / 2 / SECTOR_SIZE as u64;
        if seg == 0 || !(seg as u64 / 2).is_multiple_of(SECTOR_SIZE as u64) || off >= sectors {
            return Ok(());
        }
        let mut buf = [0u8; SECTOR_SIZE];
        dev.read(start_lba + off, &mut buf).await?;
        if let Some(b) = Superblock::decode(&buf)
            && b.segment_size == seg
            && best.is_none_or(|x| b.write_seq > x.write_seq)
        {
            *best = Some(b);
        }
        Ok(())
    };
    match a {
        Some(a) => try_b(a.segment_size, &mut best).await?,
        None => {
            for seg in SEGMENT_CANDIDATES {
                try_b(seg, &mut best).await?;
            }
        }
    }
    Ok(best)
}
