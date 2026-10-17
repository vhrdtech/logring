//! raw_writer plus a reliable stream backed by track 1 (TrackStore, which reads the ring back).
#![no_std]
#![no_main]

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use logring_size::{BUF, ConstSlot, RawCard, Slot, TRACKS, block_on, input, now, output, session};
use ww_logring::{Buffers, Config, Logring, TrackStore};
use ww_stream::StreamHead;
use ww_stream::store::StreamStore;

type Lr = Logring<'static, RawCard, BUF, TRACKS>;
static BUFS: ConstSlot<Buffers<BUF, TRACKS>> = ConstSlot::new(Buffers::new());
static LR: Slot<Mutex<NoopRawMutex, Lr>> = Slot::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let cfg = Config {
            segment_size: 1 << 20,
            flush_bytes: 16 << 10,
            ..Config::default()
        };
        let lr: &'static Mutex<NoopRawMutex, Lr> =
            match Logring::new(RawCard, BUFS.take(), cfg, session()) {
                Ok(lr) => LR.put(Mutex::new(lr)),
                Err(_) => loop {
                    output(0xdead);
                },
            };
        if lr.lock().await.mount().await.is_err() {
            loop {
                output(0xdead);
            }
        }
        let mut store = match TrackStore::new(lr, 1).await {
            Ok(s) => s,
            Err(_) => loop {
                output(0xdead);
            },
        };
        let mut buf = [0u8; 64];
        loop {
            let st = store.state();
            match input() % 4 {
                0 => {
                    let head = StreamHead {
                        pos: st.next,
                        units: 1,
                        lost: None,
                        t: now(),
                        ext: None,
                    };
                    let len = input() as usize % buf.len();
                    let _ = store.push(&head, &buf[..len]).await;
                }
                1 => {
                    if let Ok(Some(ev)) = store.read(input() as u64, &mut buf).await {
                        output(ev.head.pos as u32 ^ ev.item.len() as u32);
                    }
                }
                2 => {
                    let _ = store.release(input() as u64).await;
                }
                _ => {
                    let mut g = lr.lock().await;
                    let _ = g
                        .push(2 + input() % (TRACKS as u32 - 2), &buf[..8], now())
                        .await;
                    let _ = g.flush_if_due(logring_size::now_ms()).await;
                }
            }
            output(st.used_bytes as u32 ^ st.dropped_units as u32);
        }
    })
}
