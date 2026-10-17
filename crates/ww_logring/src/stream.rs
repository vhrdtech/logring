//! [BlockStream]: the payload bytes of one segment's blocks read in order from a [LogMedium], each block checked
//! by its trailer, with a bounded number of reads: every block is read once.

use crate::format::{BLOCK, BlockCheck, PAYLOAD, Role, check_block};
use ww_stream::store::LogMedium;

/// A sequential reader of one segment: block by block, payload bytes only, stopping at the first block that isn't
/// this segment's (the end of what was written) or a damaged one.
pub struct BlockStream<'m, M: LogMedium> {
    medium: &'m mut M,
    buf: &'m mut [u8; BLOCK],
    base: u64,
    log_id: u32,
    role: Role,
    seq: u64,
    blocks: u32,
    /// Index of the block in `buf`, `None` before the first read
    loaded: Option<u32>,
    /// Flush offset of the loaded block
    flush_off: u16,
    /// Next block to load
    next: u32,
    /// Position in the loaded block's payload
    off: usize,
    /// How the stream ended, once it did
    end: Option<BlockCheck>,
    reads: u32,
}

impl<'m, M: LogMedium> BlockStream<'m, M> {
    /// Over segment `seq` of `role` at byte `base` of the medium, `blocks` blocks long, starting at block `from`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        medium: &'m mut M,
        buf: &'m mut [u8; BLOCK],
        base: u64,
        log_id: u32,
        role: Role,
        seq: u64,
        blocks: u32,
        from: u32,
    ) -> Self {
        BlockStream {
            medium,
            buf,
            base,
            log_id,
            role,
            seq,
            blocks,
            loaded: None,
            flush_off: 0,
            next: from,
            off: PAYLOAD,
            end: None,
            reads: 0,
        }
    }

    /// Blocks read from the medium so far.
    pub fn reads(&self) -> u32 {
        self.reads
    }

    /// Why the stream ended: `Torn` for a block of this segment with a bad CRC, `Other` for anything else (the
    /// segment's end included). `None` while it hasn't.
    pub fn ended(&self) -> Option<BlockCheck> {
        self.end
    }

    /// Index of the block the next byte comes from (the next block to load when at a block boundary).
    pub fn block(&self) -> u32 {
        if self.off < PAYLOAD {
            self.loaded.unwrap_or(self.next)
        } else {
            self.next
        }
    }

    /// Offset in the payload of the block the next byte comes from.
    pub fn offset(&self) -> usize {
        if self.off >= PAYLOAD { 0 } else { self.off }
    }

    /// At the start of a block (no byte of it consumed yet).
    pub fn at_block_start(&self) -> bool {
        self.off == PAYLOAD || self.off == 0
    }

    /// Flush offset of the block the next byte comes from (loads it); `None` at the end.
    pub async fn flush_off(&mut self) -> Result<Option<u16>, M::Error> {
        if self.off >= PAYLOAD && !self.load().await? {
            return Ok(None);
        }
        Ok(Some(self.flush_off))
    }

    /// Load the next block; `false` (and `ended()` set) when it isn't this segment's.
    async fn load(&mut self) -> Result<bool, M::Error> {
        if self.end.is_some() {
            return Ok(false);
        }
        if self.next >= self.blocks {
            self.end = Some(BlockCheck::Other);
            return Ok(false);
        }
        self.medium
            .read(self.base + self.next as u64 * BLOCK as u64, self.buf)
            .await?;
        self.reads += 1;
        match check_block(self.buf, self.log_id, self.role, self.seq, self.next) {
            BlockCheck::Ok { flush_off } => {
                self.flush_off = flush_off;
                self.loaded = Some(self.next);
                self.next += 1;
                self.off = 0;
                Ok(true)
            }
            other => {
                self.end = Some(other);
                Ok(false)
            }
        }
    }

    /// Copy up to `out.len()` bytes from the current position into `out` without consuming them (across blocks:
    /// the look-ahead loads following blocks and then reloads the current one, so `out` should be small).
    /// Returns how many were copied: fewer at the end.
    pub async fn peek(&mut self, out: &mut [u8]) -> Result<usize, M::Error> {
        let mut n = 0;
        if self.off < PAYLOAD {
            let take = (PAYLOAD - self.off).min(out.len());
            out[..take].copy_from_slice(&self.buf[self.off..self.off + take]);
            n = take;
        }
        if n == out.len() || self.end.is_some() {
            return Ok(n);
        }
        let saved = (self.loaded, self.off, self.next, self.flush_off);
        let mut attempted = false;
        while n < out.len() {
            attempted = true;
            if !self.load().await? {
                break;
            }
            let take = PAYLOAD.min(out.len() - n);
            out[n..n + take].copy_from_slice(&self.buf[..take]);
            n += take;
        }
        if attempted {
            // a load, failed or not, replaced the buffer: bring the block the position is in back
            self.end = None;
            if saved.1 < PAYLOAD
                && let Some(idx) = saved.0
            {
                self.next = idx;
                self.load().await?;
            }
            (self.loaded, self.off, self.next, self.flush_off) = saved;
        }
        Ok(n)
    }

    /// Consume `n` bytes; `false` when the stream ends first.
    pub async fn skip(&mut self, mut n: usize) -> Result<bool, M::Error> {
        while n > 0 {
            if self.off >= PAYLOAD && !self.load().await? {
                return Ok(false);
            }
            let take = (PAYLOAD - self.off).min(n);
            self.off += take;
            n -= take;
        }
        Ok(true)
    }

    /// Copy `out.len()` bytes into `out`; `false` when the stream ends first.
    pub async fn read(&mut self, out: &mut [u8]) -> Result<bool, M::Error> {
        let mut done = 0;
        while done < out.len() {
            if self.off >= PAYLOAD && !self.load().await? {
                return Ok(false);
            }
            let take = (PAYLOAD - self.off).min(out.len() - done);
            out[done..done + take].copy_from_slice(&self.buf[self.off..self.off + take]);
            self.off += take;
            done += take;
        }
        Ok(true)
    }

    /// Skip the rest of the loaded block (after a padding byte).
    pub fn skip_block(&mut self) {
        self.off = PAYLOAD;
    }
}
