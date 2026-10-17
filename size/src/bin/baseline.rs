//! The dummy card, the executor and the inputs without logring: what every other bin's size is compared to.
#![no_std]
#![no_main]

use exfat_lite::BlockDevice;
use logring_size::{Card, block_on, input, output};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let mut card = Card;
    let mut buf = [0u8; 512];
    loop {
        block_on(async {
            let lba = input() as u64;
            if card.read(lba, &mut buf).await.is_ok() {
                let _ = card.write(lba + 1, &buf).await;
            }
            output(card.sector_count() as u32 ^ buf[3] as u32);
        });
    }
}
