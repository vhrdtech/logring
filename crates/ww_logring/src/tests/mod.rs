//! Tests on simulated media: round trips of random multi-track workloads, a power cut at every write, bounded mount
//! reads, geometry checks.

#[cfg(feature = "block-device-driver")]
mod bdd;
mod corrupt;
#[cfg(feature = "exfat")]
mod exfat;
mod writer;

pub(crate) use crate::tests_support::block_on;
use ww_stream::store::LogMedium;

/// Small seeded PRNG (xorshift64*), so every run can be repeated.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub(crate) fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

/// A medium that counts the block reads made through it.
pub(crate) struct Counting<M> {
    pub inner: M,
    pub reads: u64,
    pub read_bytes: u64,
    pub writes: u64,
}

impl<M> Counting<M> {
    pub(crate) fn new(inner: M) -> Self {
        Counting {
            inner,
            reads: 0,
            read_bytes: 0,
            writes: 0,
        }
    }
}

impl<M: LogMedium> LogMedium for Counting<M> {
    type Error = M::Error;
    const WRITE_SIZE: usize = M::WRITE_SIZE;
    const ERASE_SIZE: usize = M::ERASE_SIZE;
    const NEEDS_ERASE: bool = M::NEEDS_ERASE;

    fn capacity(&self) -> u64 {
        self.inner.capacity()
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), M::Error> {
        assert!(
            offset + buf.len() as u64 <= self.inner.capacity(),
            "read of {} bytes at {offset} past the capacity {}",
            buf.len(),
            self.inner.capacity()
        );
        self.reads += 1;
        self.read_bytes += buf.len() as u64;
        self.inner.read(offset, buf).await
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), M::Error> {
        self.writes += 1;
        self.inner.write(offset, data).await
    }

    async fn erase(&mut self, from: u64, to: u64) -> Result<(), M::Error> {
        self.inner.erase(from, to).await
    }
}
