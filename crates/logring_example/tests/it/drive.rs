use logring::Buffers;
use logring::scan::Scan;
use logring::tests_support::block_on;
use logring_example::api::{CanFrame, Mode};
use logring_example::bench::{self, Bench, Totals};
use logring_example::card::SIM_CARD_BYTES;
use logring_example::drive::{self, track};
use logring_example::{Drive, SimCard};
use ww_api::shrink_wrap::DeserializeShrinkWrap;

/// Run `bench` on the card `bytes` in one session.
fn boot_and_run(bytes: &mut [u8], bench: Bench) -> Totals {
    // --8<-- [start:first_log]
    let mut bufs = Buffers::new();
    block_on(async {
        let card = SimCard::new(bytes);
        let mut drive = Drive::mount(card, &mut bufs, drive::config())
            .await
            .unwrap();
        bench::run(&mut drive, bench).await.unwrap()
    })
    // --8<-- [end:first_log]
}

fn scan(bytes: &mut [u8]) -> Scan {
    let mut card = SimCard::new(bytes);
    block_on(Scan::open(&mut card, Some(drive::config().segment_size))).unwrap()
}

fn count(scan: &Scan, track: u32) -> u32 {
    let n = scan
        .data
        .iter()
        .flat_map(|s| &s.records)
        .filter(|r| r.track == track)
        .count();
    u32::try_from(n).unwrap()
}

#[test]
fn a_minute_of_the_bench_is_on_the_card() {
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let totals = boot_and_run(&mut bytes, Bench::FIRST_MINUTE);
    assert_eq!(totals.can0, 60_000);
    assert_eq!(totals.vbat, 60);
    assert_eq!(totals.mode, 3);
    assert_eq!(totals.recordings, 1);

    let scan = scan(&mut bytes);
    assert!(scan.data.len() > 1, "a minute fills more than one segment");
    assert!(scan.data.iter().all(|s| !s.torn_block && !s.torn_record));
    assert_eq!(count(&scan, track::LOG), totals.log);
    assert_eq!(count(&scan, track::MODE), totals.mode);
    assert_eq!(count(&scan, track::CAN0), totals.can0);
    assert_eq!(count(&scan, track::VBAT), totals.vbat);

    let first = scan
        .data
        .iter()
        .flat_map(|s| &s.records)
        .find(|r| r.track == track::CAN0)
        .unwrap();
    let frame = CanFrame::from_ww_bytes(&first.item).unwrap();
    assert_eq!(frame.id, 0x101);
    assert_eq!(frame.data.len(), 8);
}

#[test]
fn a_fault_stops_the_frames_and_reaches_the_card() {
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let bench = Bench {
        seconds: 10,
        can_hz: 100,
        fault_at_s: Some(5),
    };
    let totals = boot_and_run(&mut bytes, bench);
    assert_eq!(totals.can0, 500);

    let scan = scan(&mut bytes);
    let modes: Vec<Mode> = scan
        .data
        .iter()
        .flat_map(|s| &s.records)
        .filter(|r| r.track == track::MODE)
        .map(|r| Mode::from_ww_bytes(&r.item).unwrap())
        .collect();
    assert_eq!(modes, [Mode::Idle, Mode::Run, Mode::Fault]);
}

#[test]
fn a_reboot_goes_on_in_a_new_session() {
    let mut bytes = vec![0u8; SIM_CARD_BYTES];
    let bench = Bench {
        seconds: 5,
        can_hz: 100,
        fault_at_s: None,
    };
    let first = boot_and_run(&mut bytes, bench);
    let second = boot_and_run(&mut bytes, bench);

    let scan = scan(&mut bytes);
    let sessions: std::collections::BTreeSet<u32> = scan.data.iter().map(|s| s.session).collect();
    assert_eq!(sessions.len(), 2);
    assert_eq!(count(&scan, track::CAN0), first.can0 + second.can0);
    // the second boot's frames continue the track's positions
    let last = scan
        .data
        .iter()
        .flat_map(|s| &s.records)
        .rfind(|r| r.track == track::CAN0)
        .unwrap();
    assert_eq!(last.head.pos, u64::from(first.can0 + second.can0 - 1));
}
