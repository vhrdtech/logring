//! [SimCard]: an SD card simulated in RAM.

use logring::LogMedium;

/// Bytes of the simulated card the book's runs use: 4 MiB.
pub const SIM_CARD_BYTES: usize = 4 << 20;

/// An SD card simulated by a byte slice: it reads and writes like a card's 512-byte blocks and never waits.
///
/// A firmware would use its card driver instead, through `ExfatRing` (feature `exfat`) or a raw partition.
pub struct SimCard<'a> {
    bytes: &'a mut [u8],
}

/// An access outside the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange {
    pub offset: u64,
    pub len: usize,
}

impl<'a> SimCard<'a> {
    /// A card that holds `bytes`: zeroes for a new card, or an image written earlier.
    pub fn new(bytes: &'a mut [u8]) -> Self {
        SimCard { bytes }
    }

    /// The card's contents.
    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    fn range(&self, offset: u64, len: usize) -> Result<core::ops::Range<usize>, OutOfRange> {
        let end = offset.checked_add(len as u64);
        if end.is_none_or(|end| end > self.bytes.len() as u64) {
            return Err(OutOfRange { offset, len });
        }
        // within the slice, so within usize
        let start = offset as usize;
        Ok(start..start + len)
    }
}

impl LogMedium for SimCard<'_> {
    type Error = OutOfRange;
    const WRITE_SIZE: usize = 512;
    const ERASE_SIZE: usize = 512;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        self.bytes.len() as u64
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), OutOfRange> {
        let r = self.range(offset, buf.len())?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OutOfRange> {
        let r = self.range(offset, data.len())?;
        self.bytes[r].copy_from_slice(data);
        Ok(())
    }

    async fn erase(&mut self, _from: u64, _to: u64) -> Result<(), OutOfRange> {
        Ok(())
    }
}
