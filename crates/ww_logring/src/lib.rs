//! `ww_logring`: a power-cut-safe ring log store for embedded devices.
//!
//! Many tracks of any [shrink_wrap](https://github.com/vhrdtech/wire_weaver) type are written at once into one
//! preallocated ring file, with sessions, recordings and markers, read back over `wire_weaver`, as a USB disk or
//! with the `logring` PC tool. The on-medium format is in `docs/format.md`; `AGENTS.md` and `FEATURES.md` say what
//! is implemented.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "block-device-driver")]
pub mod bdd;
#[cfg(feature = "exfat")]
pub mod exfat;
pub mod format;
#[cfg(feature = "std")]
pub mod harness;
#[cfg(feature = "std")]
pub mod image;
pub mod reader;
#[cfg(feature = "std")]
pub mod scan;
pub mod stream;
pub mod system;
#[cfg(feature = "std")]
pub mod tests_support;
pub mod track_store;
pub mod writer;

pub use format::header::{Durability, MAX_PATH, TrackDesc, api_root_id};
pub use track_store::TrackStore;
pub use wire_weaver::shrink_wrap::{RefVec, UNib32};
pub use wire_weaver::ww_version::FullVersion;
pub use writer::{Config, Error, Geo, Logring, Recovery, SessionInfo, TrackState};
pub use ww_stream::store::FullPolicy;
pub use ww_stream::store::LogMedium;
pub use ww_stream::{Second, StreamHead};

#[cfg(test)]
mod tests;
