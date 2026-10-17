//! The writer alone on a raw medium (a card partition or flash, no exFAT): mount with recovery, records,
//! markers, recordings, flushes.
#![no_std]
#![no_main]

use logring_size::{BUF, ConstSlot, RawCard, Slot, TRACKS, block_on, log_loop, output, session};
use ww_logring::{Buffers, Config, Logring};

static BUFS: ConstSlot<Buffers<BUF, TRACKS>> = ConstSlot::new(Buffers::new());
static LR: Slot<Logring<'static, RawCard, BUF, TRACKS>> = Slot::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let cfg = Config {
            segment_size: 1 << 20,
            flush_bytes: 16 << 10,
            ..Config::default()
        };
        let lr = match Logring::new(RawCard, BUFS.take(), cfg, session()) {
            Ok(lr) => LR.put(lr),
            Err(_) => loop {
                output(0xdead);
            },
        };
        if lr.mount().await.is_err() {
            loop {
                output(0xdead);
            }
        }
        output(lr.recovery().torn_blocks);
        log_loop(lr).await
    })
}
