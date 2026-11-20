//! [Drive]: the drive's logger, a [Logring] with one call per track of the drive's API.

use crate::api::{CanFrame, DRIVE_FULL_GID, Level, LogLine, Mode, Vbat};
use logring::{Buffers, Config, Durability, LogMedium, Logring, SessionInfo, TrackDesc, UNib32};
use ww_api::shrink_wrap::{RefVec, SerializeShrinkWrap};
use ww_si::ww_numeric::NumericValue;

/// Bytes of the writer's stage buffer: the RAM a flush is built in before it goes to the card.
pub const BUF: usize = 4096;
/// Tracks of the ring: the system track and the drive's four.
pub const TRACKS: usize = 5;
/// Bytes of an encoded item at most: a log line of up to 120 bytes of text, a frame of up to 8 data bytes.
pub const ITEM_MAX: usize = 128;

/// The track numbers of the drive's streams; track 0 is the system track.
pub mod track {
    pub const LOG: u32 = 1;
    pub const MODE: u32 = 2;
    pub const CAN0: u32 = 3;
    pub const VBAT: u32 = 4;
}

// --8<-- [start:tracks]
/// The drive's tracks, in the order of [track]: each names its stream by its path in the API.
pub static TRACK_DESCS: [TrackDesc<'static>; TRACKS - 1] = [
    TrackDesc {
        rate: Some(2),
        ..TrackDesc::new(&[UNib32(0)])
    },
    // a change of mode is on the card before `mode` returns
    TrackDesc {
        durability: Durability::Immediate,
        ..TrackDesc::new(&[UNib32(1)])
    },
    TrackDesc {
        rate: Some(1000),
        ..TrackDesc::new(&[UNib32(2)])
    },
    TrackDesc {
        rate: Some(1),
        ..TrackDesc::new(&[UNib32(3)])
    },
];
// --8<-- [end:tracks]

/// What the drive tells the ring about itself at every mount.
pub fn session() -> SessionInfo<'static> {
    SessionInfo {
        api: DRIVE_FULL_GID,
        device_id: "drive-0042",
        fw: "drive fw 1.4.0",
        hw: "drive rev C",
        tracks: &TRACK_DESCS,
        bundle: None,
    }
}

/// The ring's layout on the simulated card: segments of 64 KiB, so that a minute of the bench fills several. A real
/// card takes its allocation unit, 4 MiB on most SD cards.
pub fn config() -> Config {
    Config {
        segment_size: 64 << 10,
        flush_bytes: 16 << 10,
        seed: 42,
        ..Config::default()
    }
}

/// Why a recording starts: the drive's own codes, stored with the recording for a reader to show.
pub mod trigger {
    /// The motor started.
    pub const MOTOR_START: u8 = 1;
}

/// Why a recording stops.
pub mod reason {
    /// The motor stopped as asked.
    pub const MOTOR_STOP: u8 = 1;
    /// A fault stopped the motor.
    pub const FAULT: u8 = 2;
}

/// The kinds of the drive's markers.
pub mod marker {
    /// The phase current went over its limit; the value is the current in mA.
    pub const OVERCURRENT: u8 = 1;
}

/// Errors of a [Drive].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// An item does not encode into [ITEM_MAX] bytes.
    Encode(ww_api::shrink_wrap::Error),
    /// The ring refused the item or the card failed.
    Ring(logring::Error<E>),
}

impl<E> From<logring::Error<E>> for Error<E> {
    fn from(e: logring::Error<E>) -> Self {
        Error::Ring(e)
    }
}

/// The drive's logger: a mounted ring and a buffer to encode items in.
pub struct Drive<'a, M: LogMedium> {
    lr: Logring<'a, M, BUF, TRACKS>,
    item: [u8; ITEM_MAX],
}

/// A device time: microseconds since boot.
fn micros(t_us: u64) -> logring::Second {
    logring::Second::from_parts(ww_si::Prefix::Micro, NumericValue::U64(t_us))
}

impl<'a, M: LogMedium> Drive<'a, M> {
    // --8<-- [start:mount]
    /// Mount the ring on `card` in a new session, with the buffers `bufs` lent for the logger's life.
    pub async fn mount(
        card: M,
        bufs: &'a mut Buffers<BUF, TRACKS>,
        cfg: Config,
    ) -> Result<Self, Error<M::Error>> {
        let mut lr = Logring::new(card, bufs, cfg, session())?;
        lr.mount().await?;
        Ok(Drive {
            lr,
            item: [0; ITEM_MAX],
        })
    }
    // --8<-- [end:mount]

    /// Start a new, empty log on `card`, whatever it holds, in a new session: after a card was formatted or its
    /// ring file re-created.
    pub async fn format(
        card: M,
        bufs: &'a mut Buffers<BUF, TRACKS>,
        cfg: Config,
    ) -> Result<Self, Error<M::Error>> {
        let mut lr = Logring::new(card, bufs, cfg, session())?;
        lr.format().await?;
        Ok(Drive {
            lr,
            item: [0; ITEM_MAX],
        })
    }

    /// Log a line of text at `t_us`.
    pub async fn log(
        &mut self,
        t_us: u64,
        level: Level,
        text: &str,
    ) -> Result<(), Error<M::Error>> {
        self.push(track::LOG, t_us, &LogLine { level, text }).await
    }

    /// Record a change of mode at `t_us`; it is on the card when this returns.
    pub async fn mode(&mut self, t_us: u64, mode: Mode) -> Result<(), Error<M::Error>> {
        self.push(track::MODE, t_us, &mode).await
    }

    // --8<-- [start:push]
    /// Record a frame of the vehicle bus at `t_us`.
    pub async fn can_frame(
        &mut self,
        t_us: u64,
        id: u32,
        data: &[u8],
    ) -> Result<(), Error<M::Error>> {
        let frame = CanFrame {
            id,
            data: RefVec::Slice { slice: data },
        };
        self.push(track::CAN0, t_us, &frame).await
    }
    // --8<-- [end:push]

    /// Record the battery voltage at `t_us`.
    pub async fn vbat(&mut self, t_us: u64, mv: u16) -> Result<(), Error<M::Error>> {
        self.push(track::VBAT, t_us, &Vbat { mv }).await
    }

    // --8<-- [start:tick]
    /// Write the flush being filled once it has been open for the flush interval. The firmware calls it every
    /// 100 ms or so with its millisecond clock; returns whether it flushed.
    pub async fn tick(&mut self, now_ms: u64) -> Result<bool, Error<M::Error>> {
        Ok(self.lr.flush_if_due(now_ms).await?)
    }
    // --8<-- [end:tick]

    // --8<-- [start:recording]
    /// Start a recording for `trigger`; returns its id, one more than the card's last.
    pub async fn recording_start(&mut self, trigger: u8) -> Result<u32, Error<M::Error>> {
        Ok(self.lr.recording_start(trigger).await?)
    }

    /// Stop the recording running, for `reason`.
    pub async fn recording_stop(&mut self, reason: u8) -> Result<(), Error<M::Error>> {
        Ok(self.lr.recording_stop(reason).await?)
    }

    /// Mark a moment of `kind`, with a label and a value; it is on the card when this returns.
    pub async fn marker(
        &mut self,
        kind: u8,
        label: &str,
        value: Option<i64>,
    ) -> Result<(), Error<M::Error>> {
        Ok(self.lr.marker(kind, label, value).await?)
    }
    // --8<-- [end:recording]

    /// Write everything recorded so far to the card.
    pub async fn sync(&mut self) -> Result<(), Error<M::Error>> {
        Ok(self.lr.sync().await?)
    }

    /// Frames pushed to `can0` that are not on the card yet: they wait in the stage buffer.
    pub fn frames_in_ram(&self) -> u32 {
        let next = self.lr.track_state(track::CAN0).map_or(0, |t| t.next);
        let durable = self.lr.durable(track::CAN0).unwrap_or(0);
        u32::try_from(next.saturating_sub(durable)).unwrap_or(u32::MAX)
    }

    /// The ring under the logger.
    pub fn logring(&self) -> &Logring<'a, M, BUF, TRACKS> {
        &self.lr
    }

    /// The card, once the logger is done with it.
    pub fn into_card(self) -> M {
        self.lr.into_medium()
    }

    async fn push<T: SerializeShrinkWrap>(
        &mut self,
        track: u32,
        t_us: u64,
        item: &T,
    ) -> Result<(), Error<M::Error>> {
        let bytes = item.to_ww_bytes(&mut self.item).map_err(Error::Encode)?;
        Ok(self.lr.push(track, bytes, Some(micros(t_us))).await?)
    }
}
