//! What the size bins share: a dummy SD card (a block device and a raw `LogMedium` over a fake peripheral, read
//! and written with volatile accesses so nothing is optimised away), a busy-polling `block_on`, statics to put
//! the writer in, and a session like a firmware's. `baseline` is these alone; every other bin's size minus
//! baseline's is what logring costs.
#![no_std]

use core::cell::UnsafeCell;
use core::future::Future;
use core::mem::MaybeUninit;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use logring::{Durability, FullVersion, SessionInfo, TrackDesc, UNib32};
use wire_weaver::ww_version::Version;
use ww_si::ww_numeric::NumericValue;
use ww_stream::Second;

/// Stage buffer of the writer: 4 KB, a firmware with little RAM.
pub const BUF: usize = 4096;
/// Tracks of a session, the system track included.
pub const TRACKS: usize = 8;

const BASE: usize = 0x4000_0000;
const FIFO: *mut u32 = BASE as *mut u32;
const LBA: *mut u32 = (BASE + 4) as *mut u32;
const STATUS: *mut u32 = (BASE + 8) as *mut u32;
const INPUT: *mut u32 = (BASE + 12) as *mut u32;
const TICKS: *mut u32 = (BASE + 16) as *mut u32;

fn rd(r: *mut u32) -> u32 {
    // SAFETY: a register of the fake peripheral; the bins are linked, never run
    unsafe { r.read_volatile() }
}

fn wr(r: *mut u32, v: u32) {
    // SAFETY: as in rd
    unsafe { r.write_volatile(v) }
}

/// A value the compiler can't know: what a firmware gets from a peripheral or a host.
pub fn input() -> u32 {
    rd(INPUT)
}

/// Hand a value to the outside, so its computation stays.
pub fn output(v: u32) {
    wr(INPUT, v)
}

/// A millisecond clock.
pub fn now_ms() -> u64 {
    rd(TICKS) as u64
}

/// A device time, µs.
pub fn now() -> Option<Second> {
    Some(Second::from_parts(
        ww_si::Prefix::Micro,
        NumericValue::U32(rd(TICKS)),
    ))
}

/// A CRC unit (feature `crc-hw`): the state goes to a register, the data through its FIFO, the result back.
#[cfg(feature = "crc-hw")]
pub struct CrcUnit;

#[cfg(feature = "crc-hw")]
impl logring::Crc32cHw for CrcUnit {
    fn update(state: u32, data: &[u8]) -> u32 {
        wr(TICKS, state);
        for c in data.chunks(4) {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            wr(FIFO, u32::from_le_bytes(w));
        }
        rd(TICKS)
    }
}

#[cfg(feature = "crc-hw")]
logring::set_crc32c!(CrcUnit);

/// The card driver's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardError(pub u32);

fn transfer_in(lba: u64, buf: &mut [u8]) -> Result<(), CardError> {
    wr(LBA, lba as u32);
    for c in buf.chunks_mut(4) {
        let w = rd(FIFO).to_le_bytes();
        c.copy_from_slice(&w[..c.len()]);
    }
    status()
}

fn transfer_out(lba: u64, buf: &[u8]) -> Result<(), CardError> {
    wr(LBA, lba as u32);
    for c in buf.chunks(4) {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        wr(FIFO, u32::from_le_bytes(w));
    }
    status()
}

fn status() -> Result<(), CardError> {
    match rd(STATUS) {
        0 => Ok(()),
        e => Err(CardError(e)),
    }
}

/// The SD card as an exFAT block device.
pub struct Card;

impl exfat_lite::BlockDevice for Card {
    type Error = CardError;

    fn sector_count(&self) -> u64 {
        rd(STATUS) as u64 * 1024
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), CardError> {
        transfer_in(lba, buf)
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), CardError> {
        transfer_out(lba, buf)
    }
}

/// The SD card (or a partition of it) as a raw ring medium, no filesystem.
pub struct RawCard;

impl ww_stream::store::LogMedium for RawCard {
    type Error = CardError;
    const WRITE_SIZE: usize = 512;
    const ERASE_SIZE: usize = 512;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        rd(STATUS) as u64 * 512 * 1024
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), CardError> {
        transfer_in(offset / 512, buf)
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), CardError> {
        transfer_out(offset / 512, data)
    }

    async fn erase(&mut self, _from: u64, _to: u64) -> Result<(), CardError> {
        Ok(())
    }
}

/// Run a future to its end, polling it in a loop (the dummy card never waits).
pub fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// A static to put a value in once, as `static_cell` does.
pub struct Slot<T>(UnsafeCell<MaybeUninit<T>>);

// SAFETY: one thread, `put` once
unsafe impl<T> Sync for Slot<T> {}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Slot(UnsafeCell::new(MaybeUninit::uninit()))
    }

    /// Put `v` in; call once.
    #[allow(clippy::mut_from_ref)]
    pub fn put(&'static self, v: T) -> &'static mut T {
        // SAFETY: called once, so no other reference exists
        unsafe { (*self.0.get()).write(v) }
    }
}

/// A static holding a value from a const initialiser, handed out once, as `static_cell::ConstStaticCell` does: the
/// value is never on the stack.
pub struct ConstSlot<T>(UnsafeCell<T>, core::cell::Cell<bool>);

// SAFETY: one thread (the bins have no interrupts), `take` hands the value out once
unsafe impl<T> Sync for ConstSlot<T> {}

impl<T> ConstSlot<T> {
    pub const fn new(v: T) -> Self {
        ConstSlot(UnsafeCell::new(v), core::cell::Cell::new(false))
    }

    /// The value; call once.
    #[allow(clippy::mut_from_ref)]
    pub fn take(&'static self) -> &'static mut T {
        assert!(!self.1.replace(true), "taken twice");
        // SAFETY: handed out once (checked above)
        unsafe { &mut *self.0.get() }
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Data tracks of the session, like a sensor logger's.
pub static TRACK_DESCS: [TrackDesc<'static>; TRACKS - 1] = {
    const fn t(path: &'static [UNib32], rate: u32) -> TrackDesc<'static> {
        TrackDesc {
            signature: Some([1, 2, 3, 4, 5, 6, 7, 8]),
            rate: Some(rate),
            ..TrackDesc::new(path)
        }
    }
    [
        t(&[UNib32(3), UNib32(0), UNib32(1)], 1000),
        t(&[UNib32(3), UNib32(1), UNib32(1)], 1000),
        t(&[UNib32(4), UNib32(1)], 400),
        t(&[UNib32(4), UNib32(2)], 400),
        t(&[UNib32(5)], 1),
        TrackDesc {
            durability: Durability::Immediate,
            ..t(&[UNib32(6)], 0)
        },
        t(&[UNib32(7)], 0),
    ]
};

/// The session the bins mount with.
pub fn session() -> SessionInfo<'static> {
    SessionInfo {
        api: FullVersion::new("board_api", Version::new(0, 3, 1)),
        device_id: "dev-0001",
        fw: "fw 1.2.3",
        hw: "rev B",
        tracks: &TRACK_DESCS,
        bundle: Some(b"\x01\x02bundle"),
    }
}

/// A firmware's panic: stop (a real one would reset or log).
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// What a logging firmware does with a mounted writer, every call in reach: records on any track, markers,
/// recordings, flushes on time and on demand.
pub async fn log_loop<M: ww_stream::store::LogMedium>(
    lr: &mut logring::Logring<'static, M, BUF, TRACKS>,
) -> ! {
    let mut item = [0u8; 64];
    loop {
        let track = input() % TRACKS as u32;
        let len = input() as usize % item.len();
        item[..4].copy_from_slice(&input().to_le_bytes());
        let r = match input() % 8 {
            0..=3 => lr.push(track, &item[..len], now()).await,
            4 => {
                lr.marker(input() as u8, "button", Some(input() as i64))
                    .await
            }
            5 => lr.recording_start(input() as u8).await.map(output),
            6 => lr.recording_stop(input() as u8).await,
            _ => lr.sync().await,
        };
        if r.is_err() {
            output(0xdead);
        }
        if let Ok(true) = lr.flush_if_due(now_ms()).await {
            output(1);
        }
        if let Some(s) = lr.track_state(track) {
            output(s.records as u32 ^ s.bytes as u32);
        }
    }
}
