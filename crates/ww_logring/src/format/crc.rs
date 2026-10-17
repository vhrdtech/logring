//! CRC-32C (Castagnoli, the `crc` crate's `CRC_32_ISCSI`), the checksum of every block and superblock. Three ways
//! to compute it, the same value each, chosen by features:
//!
//! - `crc-table` (default): a 1 KB lookup table, a byte per step;
//! - without it: bit by bit, no table (several times slower per byte; not measured on an MCU yet);
//! - `crc-hw`: a CRC unit of the MCU (an STM32's programmable CRC peripheral, say), through a [Crc32cHw] the
//!   firmware names once with [set_crc32c](crate::set_crc32c), as `critical-section` names its implementation.
//!   A build with `crc-hw` and no `set_crc32c!` fails to link.

#[cfg(not(feature = "crc-hw"))]
use crc::{CRC_32_ISCSI, Crc};

#[cfg(all(not(feature = "crc-hw"), feature = "crc-table"))]
type Imp = crc::Table<1>;
#[cfg(all(not(feature = "crc-hw"), not(feature = "crc-table")))]
type Imp = crc::NoTable;

#[cfg(not(feature = "crc-hw"))]
static SOFT: Crc<u32, Imp> = Crc::<u32, Imp>::new(&CRC_32_ISCSI);

/// A CRC-32C unit (feature `crc-hw`): `update` continues the CRC from `state` over `data` and returns the new
/// state. `state` is the reflected CRC register as software CRC-32C keeps it: `0xffff_ffff` at the start, the
/// checksum is the last state XOR `0xffff_ffff`. Called from the writer's and the reader's tasks, never from an
/// interrupt; an implementation that shares the unit with other code guards it itself.
#[cfg(feature = "crc-hw")]
pub trait Crc32cHw {
    fn update(state: u32, data: &[u8]) -> u32;
}

#[cfg(feature = "crc-hw")]
unsafe extern "Rust" {
    fn _ww_logring_crc32c_update(state: u32, data: &[u8]) -> u32;
}

/// Name the firmware's [Crc32cHw] (feature `crc-hw`), once in the binary: `ww_logring::set_crc32c!(MyCrc);`.
#[cfg(feature = "crc-hw")]
#[macro_export]
macro_rules! set_crc32c {
    ($t:ty) => {
        #[unsafe(no_mangle)]
        fn _ww_logring_crc32c_update(state: u32, data: &[u8]) -> u32 {
            <$t as $crate::format::crc::Crc32cHw>::update(state, data)
        }
    };
}

/// A CRC-32C over several pieces.
pub struct Digest {
    #[cfg(not(feature = "crc-hw"))]
    d: crc::Digest<'static, u32, Imp>,
    #[cfg(feature = "crc-hw")]
    state: u32,
}

impl Digest {
    pub fn new() -> Self {
        Digest {
            #[cfg(not(feature = "crc-hw"))]
            d: SOFT.digest(),
            #[cfg(feature = "crc-hw")]
            state: 0xffff_ffff,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        #[cfg(not(feature = "crc-hw"))]
        self.d.update(data);
        #[cfg(feature = "crc-hw")]
        {
            // SAFETY: the symbol is the firmware's `set_crc32c!` function, a safe Rust fn of this signature
            self.state = unsafe { _ww_logring_crc32c_update(self.state, data) };
        }
    }

    pub fn finalize(self) -> u32 {
        #[cfg(not(feature = "crc-hw"))]
        return self.d.finalize();
        #[cfg(feature = "crc-hw")]
        return self.state ^ 0xffff_ffff;
    }
}

impl Default for Digest {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32C of `data`.
pub fn checksum(data: &[u8]) -> u32 {
    let mut d = Digest::new();
    d.update(data);
    d.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A software CRC-32C as the "unit" (feature `crc-hw` in tests): bitwise, reflected polynomial 0x82f63b78.
    #[cfg(feature = "crc-hw")]
    struct Bitwise;

    #[cfg(feature = "crc-hw")]
    impl Crc32cHw for Bitwise {
        fn update(mut state: u32, data: &[u8]) -> u32 {
            for &b in data {
                state ^= b as u32;
                for _ in 0..8 {
                    state = if state & 1 != 0 {
                        (state >> 1) ^ 0x82f6_3b78
                    } else {
                        state >> 1
                    };
                }
            }
            state
        }
    }

    #[cfg(feature = "crc-hw")]
    crate::set_crc32c!(Bitwise);

    #[test]
    fn check_value_in_every_build() {
        assert_eq!(checksum(b"123456789"), 0xe306_9283);
        let mut d = Digest::new();
        d.update(b"1234");
        d.update(b"");
        d.update(b"56789");
        assert_eq!(d.finalize(), 0xe306_9283);
    }
}
