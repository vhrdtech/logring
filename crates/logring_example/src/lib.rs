//! The running example of logring's book: the firmware side of the drive, a motor controller board that keeps the
//! last days of its log lines, its mode, its CAN frames and its battery voltage on an SD card.
//!
//! - [api]: the drive's WireWeaver API, whose streams are the ring's tracks, and their item types.
//! - [card]: [SimCard], a simulated SD card in RAM, so the example runs on a PC and builds for any MCU.
//! - [drive]: [Drive], the firmware's logger: the session it mounts with, one call per track, the flush tick.
//! - [bench]: [bench::run], a deterministic stretch of the endurance bench at any length, which the book's tests and
//!   figures replay.
//!
//! The crate is `no_std` and allocates nothing: the same code builds for the host and for an MCU target such as
//! `thumbv8m.main-none-eabihf`.
#![no_std]

pub mod api;
pub mod bench;
pub mod card;
pub mod drive;

pub use card::SimCard;
pub use drive::Drive;
