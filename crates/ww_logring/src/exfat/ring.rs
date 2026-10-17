//! [ExfatRing]: the ring file's sectors as a [LogMedium], a window the writer can't leave.

use exfat_lite::{BlockDevice, SECTOR_SIZE};
use ww_stream::store::LogMedium;

/// Where the ring file sits on its card, as the superblock keeps it (`binding`, 16 bytes): the volume serial, the
/// file's first cluster and cluster count, and the absolute LBA of its first sector (low 32 bits). A mount
/// compares what it finds on the card against this, see [open](super::open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Binding {
    /// `VolumeSerialNumber` of the exFAT volume.
    pub serial: u32,
    /// First cluster of the ring file.
    pub first_cluster: u32,
    /// Clusters of the ring file.
    pub clusters: u32,
    /// Absolute LBA of the file's first sector, low 32 bits (cards up to 2 TB: all of it).
    pub start_lba: u32,
}

impl Binding {
    /// The 16 bytes of the superblock's `binding` field.
    pub fn encode(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.serial.to_le_bytes());
        b[4..8].copy_from_slice(&self.first_cluster.to_le_bytes());
        b[8..12].copy_from_slice(&self.clusters.to_le_bytes());
        b[12..16].copy_from_slice(&self.start_lba.to_le_bytes());
        b
    }

    /// Read a superblock's `binding` field.
    pub fn decode(b: &[u8; 16]) -> Self {
        let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Binding {
            serial: w(0),
            first_cluster: w(4),
            clusters: w(8),
            start_lba: w(12),
        }
    }
}

/// Errors of an [ExfatRing].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RingError<E> {
    /// The block device failed.
    #[error("block device error")]
    Device(E),
    /// An access past the end of the ring file: refused, nothing outside the file is ever touched.
    #[error("access outside the ring file")]
    OutOfRange,
    /// A write not on whole sectors.
    #[error("write not on whole sectors")]
    Unaligned,
}

/// The ring file's sectors on a block device, as a [LogMedium] for [Logring](crate::Logring).
///
/// Made by [open](super::open) once the mount check has bound it to the file's place on the card, or by
/// [format_card](super::format_card) / [recreate_ring](super::recreate_ring). Every access is checked against the
/// window, so a writer bug can't reach the rw files around it. Reads may be of any length at any offset (through
/// a one-sector bounce buffer where they are not whole sectors); writes are whole sectors and go to the device as
/// one call (a multi-block write) each.
pub struct ExfatRing<D: BlockDevice> {
    dev: D,
    start_lba: u64,
    sectors: u64,
    binding: Binding,
    bounce: [u8; SECTOR_SIZE],
}

impl<D: BlockDevice> ExfatRing<D> {
    pub(crate) fn new(dev: D, start_lba: u64, sectors: u64, binding: Binding) -> Self {
        ExfatRing {
            dev,
            start_lba,
            sectors,
            binding,
            bounce: [0; SECTOR_SIZE],
        }
    }

    /// The binding the ring was opened or created with: [Config::binding](crate::Config::binding) is its
    /// [encode](Binding::encode).
    pub fn binding(&self) -> Binding {
        self.binding
    }

    /// Absolute LBA of the ring file's first sector.
    pub fn start_lba(&self) -> u64 {
        self.start_lba
    }

    /// Sectors of the ring file.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// The device under the ring, for the rest of the card (the rw files, a USB passthrough).
    pub fn device(&mut self) -> &mut D {
        &mut self.dev
    }

    /// Give the device back.
    pub fn into_device(self) -> D {
        self.dev
    }

    fn lba(&self, offset: u64, len: u64) -> Result<u64, RingError<D::Error>> {
        let end = offset.checked_add(len).ok_or(RingError::OutOfRange)?;
        if end > self.sectors * SECTOR_SIZE as u64 {
            return Err(RingError::OutOfRange);
        }
        Ok(self.start_lba + offset / SECTOR_SIZE as u64)
    }
}

impl<D: BlockDevice> LogMedium for ExfatRing<D> {
    type Error = RingError<D::Error>;
    const WRITE_SIZE: usize = SECTOR_SIZE;
    const ERASE_SIZE: usize = SECTOR_SIZE;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        self.sectors * SECTOR_SIZE as u64
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut lba = self.lba(offset, buf.len() as u64)?;
        let s = SECTOR_SIZE as u64;
        if offset.is_multiple_of(s) && buf.len().is_multiple_of(SECTOR_SIZE) {
            if !buf.is_empty() {
                self.dev.read(lba, buf).await.map_err(RingError::Device)?;
            }
            return Ok(());
        }
        let mut skip = (offset % s) as usize;
        let mut done = 0;
        while done < buf.len() {
            self.dev
                .read(lba, &mut self.bounce)
                .await
                .map_err(RingError::Device)?;
            let n = (SECTOR_SIZE - skip).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&self.bounce[skip..skip + n]);
            done += n;
            skip = 0;
            lba += 1;
        }
        Ok(())
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), Self::Error> {
        if !offset.is_multiple_of(SECTOR_SIZE as u64) || !data.len().is_multiple_of(SECTOR_SIZE) {
            return Err(RingError::Unaligned);
        }
        let lba = self.lba(offset, data.len() as u64)?;
        if data.is_empty() {
            return Ok(());
        }
        self.dev.write(lba, data).await.map_err(RingError::Device)
    }

    async fn erase(&mut self, _from: u64, _to: u64) -> Result<(), Self::Error> {
        Ok(())
    }
}
