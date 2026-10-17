//! [BddDevice]: an SD driver that implements `block-device-driver`'s `BlockDevice<512>` (embassy's SDMMC, other
//! embedded drivers) as `exfat_lite`'s [BlockDevice](exfat_lite::BlockDevice), so [ExfatRing](crate::exfat::ExfatRing)
//! and the format run on it. Every write of several sectors goes to the driver as one call, a multi-block write
//! (CMD25 on an SD card): straight from the caller's buffer when it has the driver's alignment (the writer's
//! stage buffer does), through a bounce buffer of `BOUNCE` blocks otherwise.

use aligned::Aligned;
use block_device_driver::{BlockDevice as Bdd, slice_to_blocks, slice_to_blocks_mut};
use exfat_lite::{BlockDevice, SECTOR_SIZE};

/// Errors of a [BddDevice].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BddError<E> {
    /// The driver failed.
    #[error("block device error")]
    Device(E),
    /// An access past the device's end or past block 2³² (the driver's addresses are 32 bit).
    #[error("access out of range")]
    OutOfRange,
}

/// A `block-device-driver` device of 512-byte blocks as an `exfat_lite` block device; see the
/// [module docs](self).
pub struct BddDevice<T: Bdd<SECTOR_SIZE>, const BOUNCE: usize = 8> {
    dev: T,
    sectors: u64,
    bounce: [Aligned<T::Align, [u8; SECTOR_SIZE]>; BOUNCE],
}

impl<T: Bdd<SECTOR_SIZE>, const BOUNCE: usize> BddDevice<T, BOUNCE> {
    const CHECK: () = assert!(BOUNCE >= 1, "BOUNCE: at least one block");

    /// Wrap `dev`, asking it for its size once.
    pub async fn new(mut dev: T) -> Result<Self, T::Error> {
        #[allow(clippy::let_unit_value)]
        let () = Self::CHECK;
        let sectors = (dev.size().await? / SECTOR_SIZE as u64).min(1 << 32);
        Ok(BddDevice {
            dev,
            sectors,
            bounce: core::array::from_fn(|_| Aligned([0; SECTOR_SIZE])),
        })
    }

    /// The driver.
    pub fn inner(&mut self) -> &mut T {
        &mut self.dev
    }

    /// Give the driver back.
    pub fn into_inner(self) -> T {
        self.dev
    }

    fn block(&self, lba: u64, len: usize) -> Result<u32, BddError<T::Error>> {
        let n = (len / SECTOR_SIZE) as u64;
        if !len.is_multiple_of(SECTOR_SIZE) || lba.checked_add(n).is_none_or(|e| e > self.sectors) {
            return Err(BddError::OutOfRange);
        }
        u32::try_from(lba).map_err(|_| BddError::OutOfRange)
    }

    /// `buf` can be handed to the driver as blocks: its alignment and a block size that keeps it.
    fn direct(buf: &[u8]) -> bool {
        let align = core::mem::align_of::<Aligned<T::Align, [u8; SECTOR_SIZE]>>();
        core::mem::size_of::<Aligned<T::Align, [u8; SECTOR_SIZE]>>() == SECTOR_SIZE
            && (buf.as_ptr() as usize).is_multiple_of(align)
    }
}

impl<T: Bdd<SECTOR_SIZE>, const BOUNCE: usize> BlockDevice for BddDevice<T, BOUNCE> {
    type Error = BddError<T::Error>;

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let block = self.block(lba, buf.len())?;
        if buf.is_empty() {
            return Ok(());
        }
        if Self::direct(buf) {
            let blocks = slice_to_blocks_mut::<T::Align, SECTOR_SIZE>(buf);
            return self.dev.read(block, blocks).await.map_err(BddError::Device);
        }
        let mut at = block;
        for chunk in buf.chunks_mut(BOUNCE * SECTOR_SIZE) {
            let n = chunk.len() / SECTOR_SIZE;
            self.dev
                .read(at, &mut self.bounce[..n])
                .await
                .map_err(BddError::Device)?;
            for (dst, src) in chunk.chunks_mut(SECTOR_SIZE).zip(self.bounce.iter()) {
                dst.copy_from_slice(&src[..]);
            }
            at += n as u32;
        }
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        let block = self.block(lba, buf.len())?;
        if buf.is_empty() {
            return Ok(());
        }
        if Self::direct(buf) {
            let blocks = slice_to_blocks::<T::Align, SECTOR_SIZE>(buf);
            return self
                .dev
                .write(block, blocks)
                .await
                .map_err(BddError::Device);
        }
        let mut at = block;
        for chunk in buf.chunks(BOUNCE * SECTOR_SIZE) {
            let n = chunk.len() / SECTOR_SIZE;
            for (dst, src) in self.bounce.iter_mut().zip(chunk.chunks(SECTOR_SIZE)) {
                dst.copy_from_slice(src);
            }
            self.dev
                .write(at, &self.bounce[..n])
                .await
                .map_err(BddError::Device)?;
            at += n as u32;
        }
        Ok(())
    }
}
