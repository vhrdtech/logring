//! The writer on a card formatted elsewhere: the exFAT mount check binds logring.wlr, then raw_writer's loop.
//! An unbound card stops logging (no format, no re-create).
#![no_std]
#![no_main]

use logring_size::{BUF, Card, Slot, TRACKS, block_on, log_loop, output, session};
use ww_logring::exfat::{ExfatRing, Opened, open};
use ww_logring::{Config, Logring};

static LR: Slot<Logring<'static, ExfatRing<Card>, BUF, TRACKS>> = Slot::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let (ring, cfg) = match open(Card).await {
            Ok(Opened::Bound(b)) => {
                output(b.warnings.volume_dirty as u32);
                b.into_parts(Config::default())
            }
            Ok(Opened::Unbound(why, _)) => loop {
                core::hint::black_box(why);
            },
            Err(_) => loop {
                output(0xdead);
            },
        };
        let lr = match Logring::mount(ring, cfg, session()).await {
            Ok(lr) => LR.put(lr),
            Err(_) => loop {
                output(0xdead);
            },
        };
        output(lr.recovery().torn_blocks);
        log_loop(lr).await
    })
}
