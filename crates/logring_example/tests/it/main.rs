//! The drive's firmware on a simulated card: it logs a minute of the bench, and the ring holds every item.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod drive;
mod live_reading;
