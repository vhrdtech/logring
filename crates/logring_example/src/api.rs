//! The drive's API: four streams, each kept in a track of the ring, and the types of their items.

use ww_api::prelude::*;

// --8<-- [start:api]
#[ww_api_root]
pub trait Drive {
    /// The firmware's log lines, a few per second.
    stream!(log: LogLine<'i>);
    /// The controller's mode, sent on every change.
    stream!(mode: Mode);
    /// Frames seen on the vehicle bus, about a thousand per second.
    stream!(can0: CanFrame<'i>);
    /// The battery voltage, once a second.
    stream!(vbat: Vbat);
}
// --8<-- [end:api]

/// One line of the firmware's log.
#[derive_shrink_wrap(derive(Debug, PartialEq, Eq, Clone, Copy))]
pub struct LogLine<'i> {
    pub level: Level,
    pub text: &'i str,
}

/// How much a log line matters.
#[derive_shrink_wrap(borrowed, derive(Debug, PartialEq, Eq, Clone, Copy), ww_repr = u4)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// What the controller does.
#[derive_shrink_wrap(borrowed, derive(Debug, PartialEq, Eq, Clone, Copy), ww_repr = u4)]
pub enum Mode {
    /// Powered, the bridge off.
    Idle,
    /// The motor runs.
    Run,
    /// Stopped by a fault, the bridge off until a reset.
    Fault,
}

/// A frame of the vehicle bus.
#[derive_shrink_wrap(derive(Debug, PartialEq, Eq, Clone, Copy))]
pub struct CanFrame<'i> {
    /// The 11-bit or 29-bit identifier.
    pub id: u32,
    /// Up to 8 data bytes.
    pub data: RefVec<'i, u8>,
}

/// The battery voltage.
#[derive_shrink_wrap(borrowed, derive(Debug, PartialEq, Eq, Clone, Copy))]
pub struct Vbat {
    pub mv: u16,
}
