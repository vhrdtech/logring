//! exfat_writer plus a reliable stream on track 1 (TrackStore): a device logging to a card formatted elsewhere
//! and serving one track as a reliable stream. An unbound card stops logging.
#![no_std]
#![no_main]

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use logring_size::{BUF, Card, Slot, TRACKS, block_on, input, log_loop, output, session};
use ww_logring::exfat::{ExfatRing, Opened, open};
use ww_logring::{Config, Logring, TrackStore};
use ww_stream::store::StreamStore;

type Lr = Logring<'static, ExfatRing<Card>, BUF, TRACKS>;
static LR: Slot<Mutex<NoopRawMutex, Lr>> = Slot::new();

fn halt() -> ! {
    loop {
        output(0xdead);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let (ring, cfg) = match open(Card).await {
            Ok(Opened::Bound(b)) => b.into_parts(Config::default()),
            Ok(Opened::Unbound(why, _)) => {
                core::hint::black_box(why);
                halt()
            }
            Err(_) => halt(),
        };
        let Ok(lr) = Logring::mount(ring, cfg, session()).await else {
            halt()
        };
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
