//! [BddDevice](crate::bdd::BddDevice): multi-block writes reach the driver as one call, unaligned buffers go
//! through the bounce buffer, and a card behind it formats, logs and mounts.

use super::block_on;
use crate::bdd::{BddDevice, BddError};
use crate::exfat::{CheckOptions, FormatOptions, Opened, format_card, open};
use crate::harness::info;
use crate::{Config, Logring};
use aligned::{A4, Aligned};
use exfat_lite::BlockDevice;

/// A driver in RAM with 4-byte alignment that records the block count of every call.
struct Driver {
    data: Vec<u8>,
    writes: Vec<usize>,
    reads: Vec<usize>,
}

impl block_device_driver::BlockDevice<512> for Driver {
    type Error = ();
    type Align = A4;

    async fn read(&mut self, block: u32, data: &mut [Aligned<A4, [u8; 512]>]) -> Result<(), ()> {
        self.reads.push(data.len());
        for (i, b) in data.iter_mut().enumerate() {
            let at = (block as usize + i) * 512;
            b.copy_from_slice(self.data.get(at..at + 512).ok_or(())?);
        }
        Ok(())
    }

    async fn write(&mut self, block: u32, data: &[Aligned<A4, [u8; 512]>]) -> Result<(), ()> {
        self.writes.push(data.len());
        for (i, b) in data.iter().enumerate() {
            let at = (block as usize + i) * 512;
            self.data
                .get_mut(at..at + 512)
                .ok_or(())?
                .copy_from_slice(&b[..]);
        }
        Ok(())
    }

    async fn size(&mut self) -> Result<u64, ()> {
        Ok(self.data.len() as u64)
    }
}

fn driver(bytes: usize) -> Driver {
    Driver {
        data: vec![0; bytes],
        writes: Vec::new(),
        reads: Vec::new(),
    }
}

#[repr(C, align(4))]
struct Buf([u8; 8192]);

#[test]
fn aligned_writes_are_one_call() {
    let mut dev = block_on(BddDevice::<_, 2>::new(driver(1 << 20))).unwrap();
    assert_eq!(dev.sector_count(), 2048);
    let mut buf = Buf([0; 8192]);
    for (i, b) in buf.0.iter_mut().enumerate() {
        *b = i as u8;
    }
    block_on(dev.write(10, &buf.0)).unwrap();
    assert_eq!(dev.inner().writes, [16]);
    // off by one byte: the bounce buffer, 2 blocks a call
    block_on(dev.write(100, &buf.0[1..1 + 2560])).unwrap();
    assert_eq!(dev.inner().writes, [16, 2, 2, 1]);
    let mut back = Buf([0; 8192]);
    block_on(dev.read(10, &mut back.0)).unwrap();
    assert_eq!(back.0, buf.0);
    let mut odd = [0u8; 2561];
    block_on(dev.read(100, &mut odd[1..])).unwrap();
    assert_eq!(odd[1..], buf.0[1..1 + 2560]);
    assert_eq!(
        block_on(dev.write(2047, &buf.0[..1024])),
        Err(BddError::OutOfRange)
    );
    assert_eq!(
        block_on(dev.write(0, &buf.0[..100])),
        Err(BddError::OutOfRange)
    );
}

#[test]
fn logring_on_a_block_device_driver() {
    let dev = block_on(BddDevice::<_, 8>::new(driver(64 << 20))).unwrap();
    let opts = FormatOptions {
        rw_bytes: 8 << 20,
        check: CheckOptions {
            sweep: false,
            ..CheckOptions::default()
        },
        ..FormatOptions::new("BDD", 42)
    };
    let mut scratch = vec![0u8; 16 << 10];
    let f = block_on(format_card(dev, None, &opts, &mut || 0u64, &mut scratch)).unwrap();
    let base = Config {
        flush_bytes: 8192,
        ..Config::default()
    };
    let (ring, cfg) = f.into_parts(base);
    let mut lr = block_on(Logring::<_, 8192, 6>::format(ring, cfg, info())).unwrap();
    let before = lr.medium().sectors();
    assert!(before > 0);
    for i in 0..2000u32 {
        block_on(lr.push(1, &i.to_le_bytes(), None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let mut ring = lr.into_medium();
    // the stage buffer is aligned: its flushes went to the driver as multi-block writes
    assert!(ring.device().inner().writes.contains(&16));
    let dev = ring.into_device();
    let b = match block_on(open(dev)).unwrap() {
        Opened::Bound(b) => b,
        Opened::Unbound(u, _) => panic!("{u:?}"),
    };
    let (ring, cfg) = b.into_parts(base);
    let lr = block_on(Logring::<_, 8192, 6>::mount(ring, cfg, info())).unwrap();
    assert_eq!(lr.track_state(1).unwrap().next, 2000);
}
