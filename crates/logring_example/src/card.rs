//! [SimCard]: an SD card simulated in RAM, whose power can fail in the middle of a write.

use exfat_lite::{BlockDevice, SECTOR_SIZE};
use logring::LogMedium;

/// Bytes of the simulated card the book's runs use: 4 MiB.
pub const SIM_CARD_BYTES: usize = 4 << 20;

/// An SD card simulated by a byte slice: it reads and writes like a card's 512-byte blocks and never waits.
///
/// It serves the ring two ways: as a raw [LogMedium], the whole card one ring, or as an exFAT [BlockDevice] for
/// `logring::exfat`, which keeps the ring in a file. A firmware would use its card driver instead.
pub struct SimCard<'a> {
    bytes: &'a mut [u8],
    /// Bytes the card still writes before its power fails; `None`: it never fails.
    power_left: Option<u64>,
}

/// Why the card did not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardError {
    /// An access outside the card.
    OutOfRange { offset: u64, len: usize },
    /// The power failed: the write that hit it went only partly to the card, and nothing after it did.
    PowerCut,
}

impl<'a> SimCard<'a> {
    /// A card that holds `bytes`: zeroes for a new card, or an image written earlier.
    pub fn new(bytes: &'a mut [u8]) -> Self {
        SimCard {
            bytes,
            power_left: None,
        }
    }

    /// A card whose power fails once it has written `budget` more bytes. The write the cut hits stops after the
    /// 512-byte sector that holds that byte, and the byte is damaged, as a sector the power left half-programmed.
    pub fn power_fails_after(bytes: &'a mut [u8], budget: u64) -> Self {
        SimCard {
            bytes,
            power_left: Some(budget),
        }
    }

    /// The card's contents.
    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    fn range(&self, offset: u64, len: usize) -> Result<core::ops::Range<usize>, CardError> {
        let end = offset.checked_add(len as u64);
        if end.is_none_or(|end| end > self.bytes.len() as u64) {
            return Err(CardError::OutOfRange { offset, len });
        }
        // within the slice, so within usize
        let start = offset as usize;
        Ok(start..start + len)
    }

    fn put(&mut self, offset: u64, data: &[u8]) -> Result<(), CardError> {
        let r = self.range(offset, data.len())?;
        let Some(left) = self.power_left else {
            self.bytes[r].copy_from_slice(data);
            return Ok(());
        };
        let n = usize::try_from(left).unwrap_or(usize::MAX);
        if n >= data.len() {
            self.bytes[r].copy_from_slice(data);
            self.power_left = Some(left - data.len() as u64);
            return Ok(());
        }
        // the sector the power cut hits lands whole but for one damaged byte, those before it whole, none after it
        let end = (n / SECTOR_SIZE + 1) * SECTOR_SIZE;
        let end = end.min(data.len());
        self.bytes[r.start..r.start + end].copy_from_slice(&data[..end]);
        self.bytes[r.start + n] ^= 0x5a;
        self.power_left = Some(0);
        Err(CardError::PowerCut)
    }
}

impl LogMedium for SimCard<'_> {
    type Error = CardError;
    const WRITE_SIZE: usize = 512;
    const ERASE_SIZE: usize = 512;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        self.bytes.len() as u64
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), CardError> {
        let r = self.range(offset, buf.len())?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), CardError> {
        self.put(offset, data)
    }

    async fn erase(&mut self, _from: u64, _to: u64) -> Result<(), CardError> {
        Ok(())
    }
}

impl BlockDevice for SimCard<'_> {
    type Error = CardError;

    fn sector_count(&self) -> u64 {
        (self.bytes.len() / SECTOR_SIZE) as u64
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), CardError> {
        let offset = lba.saturating_mul(SECTOR_SIZE as u64);
        let r = self.range(offset, buf.len())?;
        buf.copy_from_slice(&self.bytes[r]);
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), CardError> {
        self.put(lba.saturating_mul(SECTOR_SIZE as u64), buf)
    }
}
