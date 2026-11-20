//! [mount_sd]: the drive's ring on its SD card, as the file `logring.wlr` of the card's exFAT volume.

use crate::drive::{self, BUF, Drive, TRACKS};
use exfat_lite::BlockDevice;
use logring::Buffers;
use logring::exfat::{
    CardInfo, Clock, ExfatRing, FormatError, FormatOptions, Opened, RingError, Unbound,
    format_card, open, recreate_ring,
};

/// What the drive formats its card with: the volume label `DRIVE`, `serial` from its random number generator.
pub fn format_options(serial: u32) -> FormatOptions<'static> {
    FormatOptions::new("DRIVE", serial)
}

/// Errors of [mount_sd].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdError<E> {
    /// The card failed.
    Card(E),
    /// The format or the re-created ring file failed, or the card is a fake.
    Format(FormatError<E>),
    /// The ring in the file refused to mount.
    Ring(drive::Error<RingError<E>>),
}

/// How [mount_sd] got its ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mounted {
    /// The ring file was where the drive put it: logging goes on.
    Bound,
    /// The ring file was gone, moved or resized: the drive made it anew, the log in it is lost.
    Recreated(Unbound),
    /// The card held no exFAT volume the drive accepts: the drive formatted it.
    Formatted,
}

// --8<-- [start:mount_sd]
/// Mount the drive's ring on the SD card `dev`. A card whose ring file checks out goes on logging. A ring file
/// that a PC moved, deleted or resized is made anew, and the other files stay. A card without a volume the drive
/// accepts is formatted, after the card check that `opts` asks for.
pub async fn mount_sd<'a, D: BlockDevice, C: Clock>(
    dev: D,
    card: Option<&CardInfo>,
    opts: &FormatOptions<'_>,
    clock: &mut C,
    bufs: &'a mut Buffers<BUF, TRACKS>,
) -> Result<(Drive<'a, ExfatRing<D>>, Mounted), SdError<D::Error>> {
    let (formatted, how) = match open(dev).await.map_err(SdError::Card)? {
        Opened::Bound(bound) => {
            let (ring, cfg) = bound.into_parts(drive::config());
            let drive = Drive::mount(ring, bufs, cfg).await.map_err(SdError::Ring)?;
            return Ok((drive, Mounted::Bound));
        }
        Opened::Unbound(Unbound::Volume(_), dev) => {
            let f = format_card(dev, card, opts, clock, bufs.stage()).await;
            (f.map_err(SdError::Format)?, Mounted::Formatted)
        }
        Opened::Unbound(why, dev) => {
            let f = recreate_ring(dev, card, opts, 0).await;
            (f.map_err(SdError::Format)?, Mounted::Recreated(why))
        }
    };
    let (ring, cfg) = formatted.into_parts(drive::config());
    let drive = Drive::format(ring, bufs, cfg)
        .await
        .map_err(SdError::Ring)?;
    Ok((drive, how))
}
// --8<-- [end:mount_sd]
