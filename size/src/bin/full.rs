//! Everything a device does with a card: exfat_recreate plus the card check and format (format_card) on an
//! unbound card.
#![no_std]
#![no_main]

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use logring_size::{BUF, Card, Slot, TRACKS, block_on, input, log_loop, now_ms, output, session};
use ww_logring::exfat::{ExfatRing, FormatOptions, Opened, format_card, open, recreate_ring};
use ww_logring::{Config, Logring, TrackStore};
use ww_stream::store::StreamStore;

type Lr = Logring<'static, ExfatRing<Card>, BUF, TRACKS>;
static LR: Slot<Mutex<NoopRawMutex, Lr>> = Slot::new();
static SCRATCH: Slot<[u8; BUF]> = Slot::new();

fn halt() -> ! {
    loop {
        output(0xdead);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let base = Config::default();
        let lr = match open(Card).await {
            Ok(Opened::Bound(mut b)) => {
                if b.warnings.attributes_changed {
                    let _ = b.restore_attributes().await;
                }
                let (ring, cfg) = b.into_parts(base);
                Logring::mount(ring, cfg, session()).await
            }
            Ok(Opened::Unbound(why, card)) => {
                core::hint::black_box(why);
                let opts = FormatOptions::new("LOGGER", input());
                let f = if input() == 1 {
                    let scratch = SCRATCH.put([0; BUF]);
                    let mut clock = now_ms;
                    format_card(card, None, &opts, &mut clock, scratch).await
                } else {
                    recreate_ring(card, None, &opts, 0).await
                };
                let Ok(f) = f else { halt() };
                output(f.report.card_flush_bytes);
                let (ring, cfg) = f.into_parts(base);
                Logring::format(ring, cfg, session()).await
            }
            Err(_) => halt(),
        };
        let Ok(lr) = lr else { halt() };
        let lr: &'static Mutex<NoopRawMutex, Lr> = LR.put(Mutex::new(lr));
        let Ok(mut store) = TrackStore::new(lr, 1).await else {
            halt()
        };
        let mut buf = [0u8; 64];
        if let Ok(Some(ev)) = store.read(input() as u64, &mut buf).await {
            output(ev.head.pos as u32);
        }
        let _ = store.release(input() as u64).await;
        output(store.state().next as u32);
        log_loop(&mut *lr.lock().await).await
    })
}
