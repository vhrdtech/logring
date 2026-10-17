//! What the card says about itself ([CardInfo]: SD Status from ACMD13, the CID) and the ring geometry that
//! follows from it ([CardGeometry]), with fallbacks where it says nothing.

/// The SD Status register (ACMD13, 64 bytes, most significant bit first), the fields the logring uses.
/// SD Physical Layer Simplified Specification, "SD Status".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SdStatus {
    /// `SPEED_CLASS` as a class number: 0, 2, 4, 6 or 10 (0 also for a reserved code).
    pub speed_class: u8,
    /// `AU_SIZE` in bytes, 0 when not defined.
    pub au_bytes: u32,
    /// `ERASE_SIZE`: allocation units erased in one operation, 0 when erase timeouts are not supported.
    pub erase_size_aus: u16,
    /// `ERASE_TIMEOUT`, seconds per `erase_size_aus` units.
    pub erase_timeout_s: u8,
    /// `ERASE_OFFSET`, seconds added to an erase.
    pub erase_offset_s: u8,
    /// `UHS_SPEED_GRADE`: 0, 1 (U1) or 3 (U3).
    pub uhs_speed_grade: u8,
    /// `UHS_AU_SIZE` in bytes, 0 when not defined.
    pub uhs_au_bytes: u32,
    /// `VIDEO_SPEED_CLASS`: 0, 6, 10, 30, 60 or 90.
    pub video_speed_class: u8,
    /// `VSC_AU_SIZE` in bytes, 0 when not defined.
    pub vsc_au_bytes: u32,
    /// `APP_PERF_CLASS`: 0, 1 (A1) or 2 (A2).
    pub app_perf_class: u8,
}

/// `AU_SIZE` / `UHS_AU_SIZE` codes in KB.
const AU_KB: [u32; 16] = [
    0,
    16,
    32,
    64,
    128,
    256,
    512,
    1024,
    2048,
    4096,
    8192,
    12 * 1024,
    16 * 1024,
    24 * 1024,
    32 * 1024,
    64 * 1024,
];

impl SdStatus {
    /// Parse the 64 bytes as the card sends them (bit 511 first). Every field is a few bits: any input parses.
    pub fn parse(raw: &[u8; 64]) -> SdStatus {
        // bit `hi` down to bit `lo` of the 512-bit register
        let bits = |hi: u32, lo: u32| -> u32 {
            let mut v = 0u32;
            for b in (lo..=hi).rev() {
                let byte = raw[((511 - b) / 8) as usize];
                v = (v << 1) | ((byte >> (b % 8)) & 1) as u32;
            }
            v
        };
        SdStatus {
            speed_class: match bits(447, 440) {
                1 => 2,
                2 => 4,
                3 => 6,
                4 => 10,
                _ => 0,
            },
            au_bytes: AU_KB[bits(431, 428) as usize] * 1024,
            erase_size_aus: bits(423, 408) as u16,
            erase_timeout_s: bits(407, 402) as u8,
            erase_offset_s: bits(401, 400) as u8,
            uhs_speed_grade: bits(399, 396) as u8,
            uhs_au_bytes: AU_KB[bits(395, 392) as usize] * 1024,
            video_speed_class: bits(391, 384) as u8,
            vsc_au_bytes: bits(377, 368) << 20,
            app_perf_class: bits(339, 336) as u8,
        }
    }
}

/// What the SD driver knows about the card, next to its block device: the SD Status where the driver reads it
/// (ACMD13), the CID. Both optional: a card or driver that gives neither gets the fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CardInfo {
    pub sd_status: Option<SdStatus>,
    /// The CID register (16 bytes, as sent): maker, product name, serial, for the card facts.
    pub cid: Option<[u8; 16]>,
}

impl CardInfo {
    /// `MID`, the manufacturer id.
    pub fn manufacturer(&self) -> Option<u8> {
        self.cid.map(|c| c[0])
    }

    /// `PNM`, the product name (5 ASCII characters).
    pub fn product_name(&self) -> Option<[u8; 5]> {
        self.cid.map(|c| [c[3], c[4], c[5], c[6], c[7]])
    }

    /// `PSN`, the product serial number.
    pub fn product_serial(&self) -> Option<u32> {
        self.cid
            .map(|c| u32::from_be_bytes([c[9], c[10], c[11], c[12]]))
    }
}

/// Segment size when the card reports no allocation unit, and the smallest segment.
pub const FALLBACK_SEGMENT: u32 = 4 << 20;
/// `flush_bytes` when the card check measured none.
pub const FALLBACK_FLUSH: u32 = 64 << 10;
/// Largest allocation unit taken from a card (`AU_SIZE` goes up to 64 MB); a larger video-class AU is left out
/// so a segment stays a size the ring and the rw region can afford.
pub const MAX_AU: u32 = 64 << 20;
/// The recording unit `flush_bytes` is a multiple of: 16 KB, the speed class contract's smallest RU.
pub const RU_BYTES: u32 = 16 << 10;

/// The ring's geometry on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardGeometry {
    /// The card's allocation unit: the largest of `AU_SIZE`, `UHS_AU_SIZE` and `VSC_AU_SIZE`, or 4 MB when it
    /// reports none; a `VSC_AU_SIZE` over [MAX_AU] is left out. The ring file starts on a multiple of it.
    pub au_bytes: u32,
    /// The card reported an allocation unit (`false`: the 4 MB fallback).
    pub au_reported: bool,
    /// Bytes per segment: the AU, at least 4 MB (a multiple of the AU when that is smaller).
    pub segment_bytes: u32,
    /// The recording unit `flush_bytes` is rounded to.
    pub ru_bytes: u32,
}

impl CardGeometry {
    /// From what the card reports, fallbacks where it reports nothing.
    pub fn from_card(card: Option<&CardInfo>) -> CardGeometry {
        let au = card
            .and_then(|c| c.sd_status)
            .map(|s| {
                [s.au_bytes, s.uhs_au_bytes, s.vsc_au_bytes]
                    .into_iter()
                    .filter(|&a| a <= MAX_AU && a.is_multiple_of(512))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let au_reported = au != 0;
        let au_bytes = if au_reported { au } else { FALLBACK_SEGMENT };
        CardGeometry {
            au_bytes,
            au_reported,
            segment_bytes: FALLBACK_SEGMENT.div_ceil(au_bytes) * au_bytes,
            ru_bytes: RU_BYTES,
        }
    }

    /// AU in sectors.
    pub fn au_sectors(&self) -> u64 {
        self.au_bytes as u64 / 512
    }

    /// `bytes` rounded up to whole recording units.
    pub fn round_flush(&self, bytes: u32) -> u32 {
        bytes.max(1).div_ceil(self.ru_bytes) * self.ru_bytes
    }
}
