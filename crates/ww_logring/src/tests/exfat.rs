//! The logring on exFAT (LOG-3): format with the card check, the mount binding check against every way a PC can
//! move the ring file, re-create, fake cards, the write-size sweep, the SD Status parser. Cards are in-memory
//! images ([SimCard]); one test runs `fsck.exfat -n` on the result where exfatprogs is installed.

use super::block_on;
use crate::exfat::{
    Bound, CardGeometry, CardInfo, CheckOptions, ExfatRing, FormatError, FormatOptions, Opened,
    RING_ATTRIBUTES, RING_NAME, RingError, SdStatus, Unbound, format_card, open, recreate_ring,
};
use crate::harness::info;
use crate::scan::Scan;
use crate::{Config, Error, Logring};
use exfat_lite::dir::{ENTRY_SIZE, set_checksum};
use exfat_lite::{
    BlockDevice, FileInfo, NewFile, OutOfRange, SECTOR_SIZE, Volume, WalkLimits, attr,
};
use std::cell::Cell;
use std::rc::Rc;
use ww_stream::store::LogMedium;

const MB: u64 = 1 << 20;
const BUF: usize = 4096;
const TRACKS: usize = 6;

type Lr = Logring<'static, ExfatRing<SimCard>, BUF, TRACKS>;

/// How a simulated card lies about its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fake {
    Honest,
    /// Addresses wrap around at the real size (the usual fake card).
    Wrap,
    /// Writes past the real size vanish, reads there give zeros.
    Drop,
}

/// An SD card in RAM: `claimed` sectors reported, `data` the real ones; writes advance a shared clock by a
/// per-size cost (µs), for the sweep.
struct SimCard {
    data: Vec<u8>,
    claimed: u64,
    fake: Fake,
    clock: Rc<Cell<u64>>,
    cost: fn(usize) -> u64,
}

impl SimCard {
    fn new(bytes: u64) -> Self {
        SimCard {
            data: vec![0; bytes as usize],
            claimed: bytes / SECTOR_SIZE as u64,
            fake: Fake::Honest,
            clock: Rc::new(Cell::new(0)),
            cost: |_| 0,
        }
    }

    fn fake(real: u64, claimed: u64, fake: Fake) -> Self {
        SimCard {
            claimed: claimed / SECTOR_SIZE as u64,
            fake,
            ..SimCard::new(real)
        }
    }

    fn real(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }

    /// Where sector `lba` really is, `None` when it isn't anywhere.
    fn map(&self, lba: u64) -> Option<usize> {
        let real = self.real();
        let s = match self.fake {
            Fake::Honest => lba,
            Fake::Wrap => lba % real,
            Fake::Drop if lba >= real => return None,
            Fake::Drop => lba,
        };
        Some(s as usize * SECTOR_SIZE)
    }
}

impl BlockDevice for SimCard {
    type Error = OutOfRange;

    fn sector_count(&self) -> u64 {
        self.claimed
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), OutOfRange> {
        let n = (buf.len() / SECTOR_SIZE) as u64;
        if lba + n > self.claimed {
            return Err(OutOfRange);
        }
        for (i, chunk) in buf.chunks_mut(SECTOR_SIZE).enumerate() {
            match self.map(lba + i as u64) {
                Some(at) => chunk.copy_from_slice(&self.data[at..at + SECTOR_SIZE]),
                None => chunk.fill(0),
            }
        }
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), OutOfRange> {
        let n = (buf.len() / SECTOR_SIZE) as u64;
        if lba + n > self.claimed {
            return Err(OutOfRange);
        }
        for (i, chunk) in buf.chunks(SECTOR_SIZE).enumerate() {
            if let Some(at) = self.map(lba + i as u64) {
                self.data[at..at + SECTOR_SIZE].copy_from_slice(chunk);
            }
        }
        self.clock.set(self.clock.get() + (self.cost)(buf.len()));
        Ok(())
    }
}

fn opts() -> FormatOptions<'static> {
    FormatOptions {
        rw_bytes: 8 * MB,
        check: CheckOptions {
            sweep_bytes: MB as u32,
            ..CheckOptions::default()
        },
        ..FormatOptions::new("LOGGER", 0x5EED_0001)
    }
}

fn base() -> Config {
    Config {
        flush_bytes: 4096,
        seed: 11,
        ..Config::default()
    }
}

fn no_clock() -> impl FnMut() -> u64 {
    || 0
}

/// A 64 MB card formatted, with a few records logged and synced.
fn logged_card(serial: u32) -> SimCard {
    let mut scratch = vec![0u8; 64 << 10];
    let o = FormatOptions { serial, ..opts() };
    let f = block_on(format_card(
        SimCard::new(64 * MB),
        None,
        &o,
        &mut no_clock(),
        &mut scratch,
    ))
    .unwrap();
    let cfg = f.config(base());
    let mut lr = block_on(Lr::format(f.ring, cfg, info())).unwrap();
    for i in 0..50u8 {
        block_on(lr.push(1, &[i; 13], None)).unwrap();
    }
    block_on(lr.marker(1, "start", None)).unwrap();
    block_on(lr.sync()).unwrap();
    lr.into_medium().into_device()
}

fn bound(card: SimCard) -> Bound<SimCard> {
    match block_on(open(card)).unwrap() {
        Opened::Bound(b) => b,
        Opened::Unbound(u, _) => panic!("unbound: {u:?}"),
    }
}

fn unbound(card: SimCard) -> Unbound {
    match block_on(open(card)).unwrap() {
        Opened::Bound(_) => panic!("bound, expected unbound"),
        Opened::Unbound(u, _) => u,
    }
}

fn ring_file(card: &mut SimCard) -> FileInfo {
    let mut vol = block_on(Volume::mount(card)).unwrap();
    block_on(vol.find(RING_NAME)).unwrap().unwrap()
}

/// Edit the raw entry set of `file` (primary entry first) and fix its checksum, as a PC's filesystem driver
/// would leave it.
fn patch_entry_set(card: &mut SimCard, file: &FileInfo, edit: impl FnOnce(&mut [u8])) {
    let info = *block_on(Volume::mount(&mut *card)).unwrap().info();
    let n = (file.secondary_count as usize + 1) * ENTRY_SIZE;
    // the set lies inside one cluster for the root directory files of these tests
    let at = info.cluster_lba(file.location.cluster).unwrap() as usize * SECTOR_SIZE
        + file.location.offset as usize;
    assert!(file.location.offset as usize + n <= info.cluster_bytes() as usize);
    let set = &mut card.data[at..at + n];
    edit(set);
    let sum = set_checksum(set);
    set[2..4].copy_from_slice(&sum.to_le_bytes());
}

#[test]
fn format_log_and_mount_again() {
    let mut card = logged_card(1);
    {
        let mut vol = block_on(Volume::mount(&mut card)).unwrap();
        let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
        assert!(report.clean, "{report:?}");
        assert!(block_on(vol.find("README.txt")).unwrap().is_some());
        let free = block_on(vol.free_space()).unwrap();
        let rw = free.free_clusters * vol.info().cluster_bytes() as u64;
        assert!(rw >= 8 * MB, "rw region {rw} bytes");
    }
    let file = ring_file(&mut card);
    assert_eq!(file.attributes & RING_ATTRIBUTES, RING_ATTRIBUTES);
    let mut b = bound(card);
    assert_eq!(b.warnings, Default::default());
    // starts on an AU (the 4 MB fallback here) and is whole segments
    assert_eq!(b.ring.start_lba() % (4 * MB / 512), 0);
    assert_eq!(b.superblock.segment_size, 4 << 20);
    assert_eq!(b.ring.capacity() % (4 * MB), 0);
    let scan = block_on(Scan::open(&mut b.ring, None)).unwrap();
    assert_eq!(scan.records().filter(|(_, r)| r.track == 1).count(), 50);
    let cfg = b.config(base());
    assert_eq!(cfg.binding, b.ring.binding().encode());
    let mut lr = block_on(Lr::mount(b.ring, cfg, info())).unwrap();
    assert!(!lr.recovery().formatted);
    block_on(lr.push(1, &[7; 5], None)).unwrap();
    block_on(lr.sync()).unwrap();
    let card = lr.into_medium().into_device();
    bound(card);
}

#[test]
fn ring_writes_stay_inside_the_file() {
    let b = bound(logged_card(1));
    let mut ring = b.ring;
    let cap = ring.capacity();
    let block = [0u8; 512];
    assert_eq!(
        block_on(ring.write(cap, &block)),
        Err(RingError::OutOfRange)
    );
    assert_eq!(
        block_on(ring.write(cap - 256, &block)),
        Err(RingError::Unaligned)
    );
    assert_eq!(
        block_on(ring.write(u64::MAX - 511, &block)),
        Err(RingError::OutOfRange)
    );
    let mut big = [0u8; 1024];
    assert_eq!(
        block_on(ring.read(cap - 512, &mut big)),
        Err(RingError::OutOfRange)
    );
    // an unaligned read goes through the bounce buffer and matches the aligned one
    let mut a = [0u8; 1536];
    block_on(ring.read(0, &mut a)).unwrap();
    let mut u = [0u8; 700];
    block_on(ring.read(300, &mut u)).unwrap();
    assert_eq!(u[..], a[300..1000]);
}

#[test]
fn deleted_ring_file_is_missing() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    let mut vol = block_on(Volume::mount(&mut card)).unwrap();
    block_on(vol.delete(&file)).unwrap();
    assert_eq!(unbound(card), Unbound::Missing);
}

#[test]
fn renamed_ring_file_is_missing() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    // "logring.wlr" -> "logring.wlx": a rename keeps the clusters
    patch_entry_set(&mut card, &file, |set| {
        let name = 2 * ENTRY_SIZE + 2 + 10 * 2;
        set[name] = b'x';
        let hash_of = |units: &[u16]| exfat_lite::Name::from_units(units).hash();
        let units: Vec<u16> = "logring.wlx".encode_utf16().collect();
        let h = hash_of(&units);
        set[ENTRY_SIZE + 4..ENTRY_SIZE + 6].copy_from_slice(&h.to_le_bytes());
    });
    assert_eq!(unbound(card), Unbound::Missing);
}

#[test]
fn truncated_ring_file_is_resized() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    // a PC truncating it: same first cluster, fewer clusters, the rest freed
    let mut vol = block_on(Volume::mount(&mut card)).unwrap();
    block_on(vol.delete(&file)).unwrap();
    let spec = NewFile::new(RING_NAME)
        .size(file.data_length / 2)
        .at_cluster(file.first_cluster)
        .attributes(file.attributes);
    block_on(vol.create(&spec)).unwrap();
    assert_eq!(unbound(card), Unbound::Resized);
}

#[test]
fn shortened_valid_length_is_size() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    patch_entry_set(&mut card, &file, |set| {
        let s = ENTRY_SIZE;
        set[s + 8..s + 16].copy_from_slice(&(file.data_length / 3).to_le_bytes());
    });
    assert_eq!(unbound(card), Unbound::Size);
}

#[test]
fn moved_ring_file_is_moved() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    let info = *block_on(Volume::mount(&mut card)).unwrap().info();
    let spc = info.sectors_per_cluster() as u64;
    // copied elsewhere byte for byte (header and all), the original deleted: a move by a PC or a defragmenter
    let mut vol = block_on(Volume::mount(&mut card)).unwrap();
    block_on(vol.delete(&file)).unwrap();
    let to = file.first_cluster - (MB / 512 / spc) as u32 * 4;
    let moved = block_on(
        vol.create(
            &NewFile::new(RING_NAME)
                .size(file.data_length - 4 * MB)
                .at_cluster(to)
                .attributes(file.attributes),
        ),
    )
    .unwrap();
    let from = info.cluster_lba(file.first_cluster).unwrap() as usize * 512;
    let dst = info.cluster_lba(moved.first_cluster).unwrap() as usize * 512;
    let n = 4 * MB as usize;
    card.data.copy_within(from..from + n, dst);
    assert_eq!(unbound(card), Unbound::Moved);
}

#[test]
fn defragmented_ring_file_is_not_contiguous() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    // what a defragmenter or a PC rewrite leaves: the same clusters but in a FAT chain
    patch_entry_set(&mut card, &file, |set| {
        set[ENTRY_SIZE + 1] &= !exfat_lite::dir::STREAM_NO_FAT_CHAIN;
    });
    assert_eq!(unbound(card), Unbound::NotContiguous);
}

#[test]
fn freed_clusters_are_not_allocated() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    let info = *block_on(Volume::mount(&mut card)).unwrap().info();
    // a chkdsk that frees the clusters but keeps the entry
    let last = file.first_cluster as u64 + file.cluster_count(info.cluster_bytes()) - 1;
    let bit = last - 2;
    let at = info.cluster_lba(info.bitmap_cluster).unwrap() as usize * 512 + (bit / 8) as usize;
    card.data[at] &= !(1 << (bit % 8));
    assert_eq!(unbound(card), Unbound::NotAllocated);
}

#[test]
fn replaced_ring_file_has_no_header() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    // another file of the same name and size in the same place, with other contents
    let mut vol = block_on(Volume::mount(&mut card)).unwrap();
    block_on(vol.delete(&file)).unwrap();
    let junk = vec![0x5a; 1 << 20];
    block_on(
        vol.create(
            &NewFile::new(RING_NAME)
                .with_data(&junk)
                .size(file.data_length)
                .at_cluster(file.first_cluster),
        ),
    )
    .unwrap();
    assert_eq!(unbound(card), Unbound::NoHeader);
}

#[test]
fn damaged_header_slot_a_still_mounts_from_b() {
    let mut card = logged_card(1);
    let b = bound(card);
    let (start, seq) = (b.ring.start_lba() as usize * 512, b.superblock.write_seq);
    card = b.ring.into_device();
    // A and B both hold a superblock after a format and a sync; A damaged
    card.data[start + 100] ^= 0xff;
    let b = bound(card);
    assert!(b.superblock.write_seq <= seq);
    let start_b = start + (4 * MB / 2) as usize;
    let mut card = b.ring.into_device();
    card.data[start_b + 100] ^= 0xff;
    assert_eq!(unbound(card), Unbound::NoHeader);
}

#[test]
fn ring_copied_from_another_card_is_other_card() {
    let a = logged_card(0xAAAA_0001);
    let mut b = logged_card(0xBBBB_0002);
    // same layout on both: copy card A's ring file contents into card B's ring file
    let (fa, fb) = {
        let mut a = a;
        let fa = ring_file(&mut a);
        let fb = ring_file(&mut b);
        let info = *block_on(Volume::mount(&mut a)).unwrap().info();
        let at = info.cluster_lba(fa.first_cluster).unwrap() as usize * 512;
        let n = fa.data_length as usize;
        b.data[at..at + n].copy_from_slice(&a.data[at..at + n]);
        (fa, fb)
    };
    assert_eq!(fa.first_cluster, fb.first_cluster);
    assert_eq!(unbound(b), Unbound::OtherCard);
}

#[test]
fn attributes_changed_warns_and_restores() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    {
        let mut vol = block_on(Volume::mount(&mut card)).unwrap();
        block_on(vol.set_attributes(&file, attr::ARCHIVE)).unwrap();
    }
    let mut b = bound(card);
    assert!(b.warnings.attributes_changed);
    assert_eq!(b.attributes, attr::ARCHIVE);
    block_on(b.restore_attributes()).unwrap();
    assert!(!b.warnings.attributes_changed);
    let card = b.ring.into_device();
    let b = bound(card);
    assert!(!b.warnings.attributes_changed);
    assert_eq!(b.attributes & RING_ATTRIBUTES, RING_ATTRIBUTES);
}

#[test]
fn not_exfat_is_a_volume_problem() {
    let card = SimCard::new(16 * MB);
    assert_eq!(unbound(card), Unbound::Volume(exfat_lite::Error::NotExfat));
}

#[test]
fn logring_refuses_another_binding() {
    let b = bound(logged_card(1));
    let mut cfg = b.config(base());
    cfg.binding[0] ^= 1;
    assert!(matches!(
        block_on(Lr::mount(b.ring, cfg, info())),
        Err(Error::Unbound)
    ));
}

#[test]
fn recreate_keeps_rw_files() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    {
        let mut vol = block_on(Volume::mount(&mut card)).unwrap();
        block_on(vol.create(&NewFile::new("notes.txt").with_data(b"mine\n"))).unwrap();
        block_on(vol.delete(&file)).unwrap();
    }
    let (u, card) = match block_on(open(card)).unwrap() {
        Opened::Unbound(u, d) => (u, d),
        Opened::Bound(_) => panic!(),
    };
    assert_eq!(u, Unbound::Missing);
    let f = block_on(recreate_ring(card, None, &opts(), 0)).unwrap();
    let cfg = f.config(base());
    let mut lr = block_on(Lr::format(f.ring, cfg, info())).unwrap();
    block_on(lr.push(1, b"after", None)).unwrap();
    block_on(lr.sync()).unwrap();
    let mut card = lr.into_medium().into_device();
    {
        let mut vol = block_on(Volume::mount(&mut card)).unwrap();
        let notes = block_on(vol.find("notes.txt")).unwrap().unwrap();
        let mut buf = [0u8; 5];
        block_on(vol.read(&notes, 0, &mut buf)).unwrap();
        assert_eq!(&buf, b"mine\n");
        assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
    }
    let b = bound(card);
    assert_eq!(b.ring.start_lba() % (4 * MB / 512), 0);
}

#[test]
fn recreate_replaces_a_replaced_ring_file() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    {
        let mut vol = block_on(Volume::mount(&mut card)).unwrap();
        block_on(vol.delete(&file)).unwrap();
        block_on(vol.create(&NewFile::new(RING_NAME).with_data(b"not a ring"))).unwrap();
    }
    let f = block_on(recreate_ring(card, None, &opts(), 128 << 10)).unwrap();
    let (ring, cfg) = f.into_parts(base());
    assert_eq!(cfg.flush_bytes, 128 << 10);
    let lr = block_on(Lr::format(ring, cfg, info())).unwrap();
    let b = bound(lr.into_medium().into_device());
    assert_eq!(b.superblock.flush_bytes, 128 << 10);
}

#[test]
fn recreate_leaves_a_broken_fat_chain_alone() {
    let mut card = logged_card(1);
    let file = ring_file(&mut card);
    // NoFatChain cleared but no chain in the FAT: deleting would have to guess which clusters are its own
    patch_entry_set(&mut card, &file, |set| {
        set[ENTRY_SIZE + 1] &= !exfat_lite::dir::STREAM_NO_FAT_CHAIN;
    });
    assert!(matches!(
        block_on(recreate_ring(card, None, &opts(), 0)).err(),
        Some(FormatError::Fs(exfat_lite::Error::Corrupt(_)))
    ));
}

#[test]
fn wrapping_fake_card_is_caught() {
    let card = SimCard::fake(16 * MB, 64 * MB, Fake::Wrap);
    let mut scratch = vec![0u8; 4096];
    let err = block_on(format_card(
        card,
        None,
        &opts(),
        &mut no_clock(),
        &mut scratch,
    ))
    .err();
    match err {
        Some(FormatError::FakeCard(a)) => {
            assert_eq!(a.usable_sectors, 16 * MB / 512);
            assert_eq!(a.reported_sectors, 64 * MB / 512);
            assert_eq!(a.lba, 16 * MB / 512);
        }
        _ => panic!("fake card not caught"),
    }
}

#[test]
fn dropping_fake_card_is_caught() {
    let card = SimCard::fake(32 * MB, 128 * MB, Fake::Drop);
    let mut scratch = vec![0u8; 4096];
    let err = block_on(format_card(
        card,
        None,
        &opts(),
        &mut no_clock(),
        &mut scratch,
    ))
    .err();
    match err {
        Some(FormatError::FakeCard(a)) => assert_eq!(a.usable_sectors, 32 * MB / 512),
        _ => panic!("fake card not caught"),
    }
}

#[test]
fn honest_card_passes_the_alias_test() {
    let mut card = SimCard::new(16 * MB);
    assert_eq!(
        block_on(crate::exfat::alias_test(&mut card, 64, 9)).unwrap(),
        None
    );
}

/// µs for a write of `n` bytes at `mb_s` MB/s plus 300 µs of command overhead.
fn at(n: usize, mb_s: f64) -> u64 {
    300 + (n as f64 / (mb_s * 1e6) * 1e6) as u64
}

/// A Samsung EVO (2019, 128 GB) as measured on omarchy-m1, 16 Oct 2026: a dip at 64 KB.
fn evo(n: usize) -> u64 {
    match n {
        x if x <= 16 << 10 => at(n, 11.9),
        x if x <= 32 << 10 => at(n, 12.5),
        x if x <= 64 << 10 => at(n, 9.5),
        x if x <= 128 << 10 => at(n, 17.2),
        x if x <= 256 << 10 => at(n, 19.0),
        x if x <= 512 << 10 => at(n, 19.5),
        _ => at(n, 20.0),
    }
}

/// A Kingston Industrial: flat from 256 KB.
fn kingston(n: usize) -> u64 {
    match n {
        x if x <= 16 << 10 => at(n, 25.0),
        x if x <= 32 << 10 => at(n, 33.0),
        x if x <= 64 << 10 => at(n, 43.4),
        x if x <= 128 << 10 => at(n, 60.0),
        _ => at(n, 80.0),
    }
}

fn sweep_on(cost: fn(usize) -> u64, scratch: usize) -> crate::exfat::CardReport {
    let card = SimCard {
        cost,
        ..SimCard::new(64 * MB)
    };
    let clock = card.clock.clone();
    let mut now = move || clock.get();
    let mut buf = vec![0u8; scratch];
    let o = FormatOptions {
        check: CheckOptions {
            sweep_bytes: 2 * MB as u32,
            ..CheckOptions::default()
        },
        ..opts()
    };
    block_on(format_card(card, None, &o, &mut now, &mut buf))
        .unwrap()
        .report
}

#[test]
fn sweep_skips_the_evo_dip() {
    let r = sweep_on(evo, 1 << 20);
    assert!(r.sweep.sizes.iter().all(|s| s.bytes > 0), "{:?}", r.sweep);
    assert_eq!(r.card_flush_bytes, 256 << 10, "{:?}", r.sweep);
    let k = |b: u32| {
        r.sweep
            .sizes
            .iter()
            .find(|s| s.write_bytes == b)
            .unwrap()
            .kb_per_s()
    };
    assert!(k(64 << 10) < k(32 << 10) && k(64 << 10) < k(128 << 10));
    assert!(r.sweep.worst_stall_us > 0);
}

#[test]
fn sweep_on_a_flat_card_picks_where_it_flattens() {
    let r = sweep_on(kingston, 1 << 20);
    assert_eq!(r.card_flush_bytes, 256 << 10);
}

#[test]
fn sweep_measures_only_what_the_scratch_holds() {
    let r = sweep_on(evo, 64 << 10);
    let measured: Vec<u32> = r
        .sweep
        .sizes
        .iter()
        .filter(|s| s.bytes > 0)
        .map(|s| s.write_bytes)
        .collect();
    assert_eq!(measured, [16 << 10, 32 << 10, 64 << 10]);
    assert_eq!(r.card_flush_bytes, 32 << 10);
}

#[test]
fn unmeasured_card_falls_back() {
    let mut scratch = vec![0u8; 64 << 10];
    let f = block_on(format_card(
        SimCard::new(64 * MB),
        None,
        &opts(),
        &mut no_clock(),
        &mut scratch,
    ))
    .unwrap();
    assert_eq!(f.report.card_flush_bytes, 0);
    assert!(f.report.alias_checked);
    assert_eq!(f.config(base()).flush_bytes, base().flush_bytes);
    assert_eq!(f.segment_size, crate::exfat::FALLBACK_SEGMENT);
}

/// SD Status bytes with the given fields set (bit 511 first).
fn sd_status(fields: &[(u32, u32, u32)]) -> [u8; 64] {
    let mut raw = [0u8; 64];
    for &(hi, lo, v) in fields {
        for b in lo..=hi {
            if v >> (b - lo) & 1 != 0 {
                raw[((511 - b) / 8) as usize] |= 1 << (b % 8);
            }
        }
    }
    raw
}

#[test]
fn sd_status_fields() {
    let raw = sd_status(&[
        (447, 440, 4),   // class 10
        (431, 428, 9),   // AU 4 MB
        (423, 408, 300), // erase size
        (407, 402, 21),  // erase timeout
        (401, 400, 2),   // erase offset
        (399, 396, 3),   // U3
        (395, 392, 0xC), // UHS AU 16 MB
        (391, 384, 30),  // V30
        (377, 368, 32),  // VSC AU 32 MB
        (339, 336, 2),   // A2
    ]);
    let s = SdStatus::parse(&raw);
    assert_eq!(
        s,
        SdStatus {
            speed_class: 10,
            au_bytes: 4 << 20,
            erase_size_aus: 300,
            erase_timeout_s: 21,
            erase_offset_s: 2,
            uhs_speed_grade: 3,
            uhs_au_bytes: 16 << 20,
            video_speed_class: 30,
            vsc_au_bytes: 32 << 20,
            app_perf_class: 2,
        }
    );
    // garbage parses too
    let _ = SdStatus::parse(&[0xff; 64]);
}

#[test]
fn geometry_from_the_card() {
    let with = |au: u32, uhs: u32, vsc: u32| {
        CardGeometry::from_card(Some(&CardInfo {
            sd_status: Some(SdStatus {
                au_bytes: au,
                uhs_au_bytes: uhs,
                vsc_au_bytes: vsc,
                ..SdStatus::default()
            }),
            cid: None,
        }))
    };
    let none = CardGeometry::from_card(None);
    assert_eq!(
        (none.au_bytes, none.segment_bytes, none.au_reported),
        (4 << 20, 4 << 20, false)
    );
    let zero = with(0, 0, 0);
    assert_eq!((zero.segment_bytes, zero.au_reported), (4 << 20, false));
    let g = with(4 << 20, 16 << 20, 0);
    assert_eq!((g.au_bytes, g.segment_bytes), (16 << 20, 16 << 20));
    let small = with(512 << 10, 0, 0);
    assert_eq!((small.au_bytes, small.segment_bytes), (512 << 10, 4 << 20));
    let odd = with(12 << 20, 0, 0);
    assert_eq!(odd.segment_bytes, 12 << 20);
    let huge = with(4 << 20, 0, 512 << 20);
    assert_eq!(huge.au_bytes, 4 << 20);
    assert_eq!(g.round_flush(20 << 10), 32 << 10);
}

#[test]
fn format_with_a_16_mb_au() {
    let card_info = CardInfo {
        sd_status: Some(SdStatus {
            au_bytes: 16 << 20,
            ..SdStatus::default()
        }),
        cid: None,
    };
    let mut scratch = vec![0u8; 4096];
    let o = FormatOptions {
        check: CheckOptions {
            aliasing: false,
            sweep: false,
            ..CheckOptions::default()
        },
        ..opts()
    };
    let f = block_on(format_card(
        SimCard::new(256 * MB),
        Some(&card_info),
        &o,
        &mut no_clock(),
        &mut scratch,
    ))
    .unwrap();
    assert_eq!(f.segment_size, 16 << 20);
    assert_eq!(f.ring.start_lba() % (16 * MB / 512), 0);
    let (ring, cfg) = f.into_parts(base());
    let lr = block_on(Lr::format(ring, cfg, info())).unwrap();
    let b = bound(lr.into_medium().into_device());
    assert_eq!(b.superblock.segment_size, 16 << 20);
}

#[test]
fn fsck_exfat_is_clean_after_format_and_logging() {
    let card = logged_card(7);
    // the volume starts after the MBR's gap: fsck.exfat reads a volume from offset 0 of a file
    let lba = u32::from_le_bytes(card.data[454..458].try_into().unwrap()) as usize;
    let dir = std::env::temp_dir().join(format!("logring-fsck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("volume.img");
    std::fs::write(&path, &card.data[lba * 512..]).unwrap();
    let out = std::process::Command::new("fsck.exfat")
        .arg("-n")
        .arg(&path)
        .output();
    std::fs::remove_dir_all(&dir).unwrap();
    match out {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.status.success() && text.contains("clean"), "{text}");
        }
        Err(_) => eprintln!("fsck.exfat not installed: skipped"),
    }
}

#[test]
fn damaged_cards_never_panic() {
    let card = logged_card(3);
    let copy = |card: &SimCard| SimCard {
        data: card.data.clone(),
        claimed: card.claimed,
        ..SimCard::new(0)
    };
    let file = ring_file(&mut copy(&card));
    let vi = *block_on(Volume::mount(&mut copy(&card))).unwrap().info();
    let ring_at = vi.cluster_lba(file.first_cluster).unwrap() as usize * 512;
    let meta_end = vi.heap_lba() as usize * 512 + 4 * vi.cluster_bytes() as usize;
    let mut rng = super::Rng::new(77);
    let (mut bound_n, mut unbound_n) = (0, 0);
    for _ in 0..300 {
        let mut c = copy(&card);
        for _ in 0..rng.range(1, 8) {
            // the MBR, boot regions, FAT, bitmap, root directory, or the ring header
            let at = if rng.chance(50) {
                rng.range(0, meta_end as u64 - 1) as usize
            } else {
                ring_at + rng.range(0, 1023) as usize
            };
            c.data[at] = rng.next() as u8;
        }
        match block_on(open(c)).unwrap() {
            Opened::Bound(b) => {
                bound_n += 1;
                let (ring, cfg) = b.into_parts(base());
                let _ = block_on(Lr::mount(ring, cfg, info()));
            }
            Opened::Unbound(..) => unbound_n += 1,
        }
    }
    assert!(
        bound_n > 0 && unbound_n > 0,
        "{bound_n} bound, {unbound_n} unbound"
    );
}
