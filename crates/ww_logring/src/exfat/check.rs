//! The card check before a format: an aliasing test that catches fake cards (they report more than they hold and
//! wrap writes around), and a sweep of write sizes that measures speed and stalls and picks the card's best
//! `flush_bytes`. Both write to the card: run them only once the user said yes to formatting it.

use exfat_lite::{BlockDevice, SECTOR_SIZE};

/// Microseconds from some fixed point, for the sweep. A closure `FnMut() -> u64` is one.
pub trait Clock {
    fn now_us(&mut self) -> u64;
}

impl<F: FnMut() -> u64> Clock for F {
    fn now_us(&mut self) -> u64 {
        self()
    }
}

/// Write sizes the sweep measures.
pub const SWEEP_SIZES: [u32; 7] = [
    16 << 10,
    32 << 10,
    64 << 10,
    128 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
];

/// The sweep picks the smallest size whose speed is within this share (per mille) of the fastest: a bigger
/// flush costs RAM and loses more on a power cut, so it has to earn its place.
pub const SWEEP_PICK_PERMILLE: u64 = 900;

/// What the card check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckOptions {
    /// Run the aliasing test.
    pub aliasing: bool,
    /// Run the write-size sweep (needs a [Clock] that ticks).
    pub sweep: bool,
    /// Bytes written per size in the sweep (default 4 MB: a few hundred ms per size on a good card).
    pub sweep_bytes: u32,
    /// Points spread over the card for the aliasing test, besides every power of two and the last sector.
    pub alias_points: u32,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions {
            aliasing: true,
            sweep: true,
            sweep_bytes: 4 << 20,
            alias_points: 64,
        }
    }
}

/// The aliasing test failed: writing at `lba` didn't read back, or changed sector 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aliased {
    /// The sector whose write failed the test.
    pub lba: u64,
    /// The sectors the card seems to hold for real: `lba` when it is a power of two (fake cards are real cards
    /// of a power-of-two size, and every power of two is a point), else the last point that passed + 1; 0 when
    /// unknown (the final read-back failed).
    pub usable_sectors: u64,
    /// The card reports this many.
    pub reported_sectors: u64,
}

/// One size of the sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SizeResult {
    /// Bytes per write.
    pub write_bytes: u32,
    /// Bytes written in all at this size (0: not measured: the scratch buffer is smaller, or no room).
    pub bytes: u64,
    /// Microseconds the writes took.
    pub us: u64,
    /// Longest single write, microseconds.
    pub worst_us: u64,
}

impl SizeResult {
    /// KB/s, 0 when not measured.
    pub fn kb_per_s(&self) -> u64 {
        if self.bytes == 0 || self.us == 0 {
            0
        } else {
            self.bytes * 1_000_000 / 1024 / self.us
        }
    }
}

/// What the sweep measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sweep {
    pub sizes: [SizeResult; SWEEP_SIZES.len()],
    /// The pick ([SWEEP_PICK_PERMILLE]), 0 when nothing was measured.
    pub best_flush_bytes: u32,
    /// Longest single write seen, microseconds.
    pub worst_stall_us: u64,
}

/// A tag sector of the aliasing test.
fn tag(buf: &mut [u8; SECTOR_SIZE], lba: u64, nonce: u32) {
    buf.fill(0);
    buf[0..8].copy_from_slice(b"WLRALIAS");
    buf[8..16].copy_from_slice(&lba.to_le_bytes());
    buf[16..20].copy_from_slice(&nonce.to_le_bytes());
    for (i, b) in buf[20..].iter_mut().enumerate() {
        *b = (lba as u8) ^ (nonce as u8) ^ (i as u8);
    }
}

/// The points of the aliasing test in ascending order, each once: every power of two below `n`, `points` spread
/// points (off the powers of two by a few sectors) and the last sector. Sector 0 is not one (it is the witness).
struct Points {
    n: u64,
    points: u32,
    p2: u64,
    spread: u32,
    last_done: bool,
    prev: u64,
}

impl Points {
    fn new(n: u64, points: u32) -> Self {
        Points {
            n,
            points,
            p2: 1,
            spread: 1,
            last_done: false,
            prev: 0,
        }
    }
}

impl Iterator for Points {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        loop {
            let p2 = (self.p2 < self.n).then_some(self.p2);
            let spread = (self.spread <= self.points)
                .then(|| {
                    (self.n as u128 * self.spread as u128 / (self.points as u128 + 1)) as u64
                        + self.spread as u64 % 7
                })
                .filter(|&l| l < self.n);
            let last = (!self.last_done).then_some(self.n - 1);
            let lba = [p2, spread, last].into_iter().flatten().min()?;
            if p2 == Some(lba) {
                self.p2 <<= 1;
            }
            if spread == Some(lba) {
                self.spread += 1;
            }
            if last == Some(lba) {
                self.last_done = true;
            }
            if lba > self.prev {
                self.prev = lba;
                return Some(lba);
            }
        }
    }
}

/// The aliasing test: tag sector 0, then at each of the [Points] (ascending) write a tag, read it back, and read
/// sector 0 back (a fake card wraps the write onto it). At the end every point is read once more, for cards that
/// map points onto each other some other way. `Ok(None)`: passed. About four sector accesses per point;
/// overwrites those sectors.
pub async fn alias_test<D: BlockDevice>(
    dev: &mut D,
    points: u32,
    nonce: u32,
) -> Result<Option<Aliased>, D::Error> {
    let n = dev.sector_count();
    if n < 2 {
        return Ok(None);
    }
    let mut want = [0u8; SECTOR_SIZE];
    let mut got = [0u8; SECTOR_SIZE];
    let mut zero = [0u8; SECTOR_SIZE];
    tag(&mut zero, 0, nonce);
    dev.write(0, &zero).await?;
    let mut last_ok = 0u64;
    for lba in Points::new(n, points) {
        tag(&mut want, lba, nonce);
        dev.write(lba, &want).await?;
        dev.read(lba, &mut got).await?;
        let ok = got == want;
        dev.read(0, &mut got).await?;
        if !ok || got != zero {
            return Ok(Some(Aliased {
                lba,
                // every power of two is a point, so a card of a power-of-two size fails right at it
                usable_sectors: if lba.is_power_of_two() {
                    lba
                } else {
                    last_ok + 1
                },
                reported_sectors: n,
            }));
        }
        last_ok = lba;
    }
    for lba in Points::new(n, points) {
        tag(&mut want, lba, nonce);
        dev.read(lba, &mut got).await?;
        if got != want {
            return Ok(Some(Aliased {
                lba,
                usable_sectors: 0,
                reported_sectors: n,
            }));
        }
    }
    Ok(None)
}

/// The write-size sweep: for each of [SWEEP_SIZES] that fits `scratch`, `bytes_per_size` written sequentially in
/// writes of that size, each size from its own allocation unit boundary, from `start_lba` on (an AU-aligned
/// place that is free to overwrite, inside `end_lba`). Timed with `clock`; picks `best_flush_bytes` (the
/// smallest size within [SWEEP_PICK_PERMILLE] of the fastest).
pub async fn sweep<D: BlockDevice, C: Clock>(
    dev: &mut D,
    clock: &mut C,
    scratch: &mut [u8],
    start_lba: u64,
    end_lba: u64,
    au_sectors: u64,
    bytes_per_size: u32,
) -> Result<Sweep, D::Error> {
    let mut out = Sweep::default();
    let au = au_sectors.max(1);
    let mut lba = start_lba.div_ceil(au) * au;
    for (k, &size) in SWEEP_SIZES.iter().enumerate() {
        let r = &mut out.sizes[k];
        r.write_bytes = size;
        let writes = (bytes_per_size / size).max(1) as u64;
        let sectors = size as u64 / SECTOR_SIZE as u64;
        if (size as usize) > scratch.len() || lba + writes * sectors > end_lba {
            continue;
        }
        let buf = &mut scratch[..size as usize];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8) ^ (k as u8).wrapping_mul(37);
        }
        let t0 = clock.now_us();
        let mut t = t0;
        for _ in 0..writes {
            dev.write(lba, buf).await?;
            lba += sectors;
            let now = clock.now_us();
            r.worst_us = r.worst_us.max(now.saturating_sub(t));
            t = now;
        }
        r.us = t.saturating_sub(t0);
        r.bytes = writes * size as u64;
        out.worst_stall_us = out.worst_stall_us.max(r.worst_us);
        lba = lba.div_ceil(au) * au;
    }
    let rate = |r: &SizeResult| {
        if r.bytes == 0 || r.us == 0 {
            0
        } else {
            r.bytes * 1_000_000 / r.us
        }
    };
    let fastest = out.sizes.iter().map(rate).max().unwrap_or(0);
    if fastest > 0 {
        out.best_flush_bytes = out
            .sizes
            .iter()
            .find(|r| rate(r) * 1000 >= fastest * SWEEP_PICK_PERMILLE)
            .map_or(0, |r| r.write_bytes);
    }
    Ok(out)
}
