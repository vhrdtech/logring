//! Reading the drive's `can0` track live, the way a host's WireWeaver API does it: [TrackStore] makes one track
//! of the ring a `ww_stream` [StreamStore], so a host reads back whatever reached the card without a card image.

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use logring::tests_support::block_on;
use logring::{Buffers, Logring, TrackStore};
use logring_example::SimCard;
use logring_example::api::CanFrame;
use logring_example::card::SIM_CARD_BYTES;
use logring_example::drive::{self, track};
use ww_api::shrink_wrap::{DeserializeShrinkWrap, RefVec, SerializeShrinkWrap};
use ww_stream::store::StreamStore;

#[test]
fn can0_reads_back_through_a_track_store() {
    let mut card = vec![0u8; SIM_CARD_BYTES];
    let mut bufs: Buffers<{ drive::BUF }, { drive::TRACKS }> = Buffers::new();
    block_on(async {
        let mut lr = Logring::new(
            SimCard::new(&mut card),
            &mut bufs,
            drive::config(),
            drive::session(),
        )
        .unwrap();
        lr.mount().await.unwrap();
        let shared = Mutex::<NoopRawMutex, _>::new(lr);
        let mut store = TrackStore::new(&shared, track::CAN0).await.unwrap();

        // --8<-- [start:live_read]
        let frame = CanFrame {
            id: 0x101,
            data: RefVec::Slice {
                slice: &[1, 2, 3, 4, 5, 6, 7, 8],
            },
        };
        let mut item = [0u8; 32];
        let bytes = frame.to_ww_bytes(&mut item).unwrap();
        shared
            .lock()
            .await
            .push(track::CAN0, bytes, None)
            .await
            .unwrap();
        shared.lock().await.sync().await.unwrap();

        // a host reads back only what reached the card, not what the stage buffer still holds
        let mut buf = [0u8; 64];
        let event = store
            .read(0, &mut buf)
            .await
            .unwrap()
            .expect("the frame is on the card");
        let got = CanFrame::from_ww_bytes(event.item).unwrap();
        assert_eq!(got.id, 0x101);
        assert!(
            store.read(1, &mut buf).await.unwrap().is_none(),
            "nothing past what was flushed"
        );
        // --8<-- [end:live_read]
    });
}
