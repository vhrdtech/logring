//! The logring on an SD card's exFAT volume (LOG-3): the ring is the preallocated contiguous file `logring.wlr`
//! that the device writes by raw sector address, past the filesystem. So on every mount the device checks that
//! the file is still where it put it ([open]: a [Bound] ring, or [Unbound] and why), formats a card after a card
//! check ([format_card]: fake-card aliasing test, write-size sweep, geometry from the card's SD Status) and
//! re-creates the ring file keeping the rw files ([recreate_ring]). Built on `exfat_lite`.
//!
//! ```text
//! mount:  open(dev) -> Opened::Bound(b)          -> let (ring, cfg) = b.into_parts(cfg); Logring::mount(ring, cfg, info)
//!                   -> Opened::Unbound(why, dev) -> logging off; recreate_ring(dev, ..) or format_card(dev, ..)
//! format: format_card(dev, card, opts, clock, scratch) -> f -> let (ring, cfg) = f.into_parts(cfg); Logring::format(ring, cfg, info)
//! ```

mod card;
mod check;
mod format;
mod mount;
mod ring;

pub use card::{
    CardGeometry, CardInfo, FALLBACK_FLUSH, FALLBACK_SEGMENT, MAX_AU, RU_BYTES, SdStatus,
};
pub use check::{
    Aliased, CheckOptions, Clock, SWEEP_PICK_PERMILLE, SWEEP_SIZES, SizeResult, Sweep, alias_test,
    sweep,
};
pub use format::{
    CardReport, FormatError, FormatOptions, Formatted, README, format_card, recreate_ring,
};
pub use mount::{Bound, Opened, Unbound, Warnings, open};
pub use ring::{Binding, ExfatRing, RingError};

/// The ring file's name, in the root directory.
pub const RING_NAME: &str = "logring.wlr";

/// The attributes the ring file is created with (archive aside): read-only, system, hidden.
pub const RING_ATTRIBUTES: u16 =
    exfat_lite::attr::READ_ONLY | exfat_lite::attr::SYSTEM | exfat_lite::attr::HIDDEN;
