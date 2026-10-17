//! Reading only (a device that serves its log, not one that writes it): open the ring, its recordings, a
//! segment summary, walk a track from a position and from a time.
#![no_std]
#![no_main]

use logring_size::{RawCard, block_on, input, now, output};
use ww_logring::reader::{Reader, TrackSummary};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    block_on(async {
        let mut m = RawCard;
        let Ok(mut rd) = Reader::open(&mut m, None).await else {
            loop {
                output(0xdead);
            }
        };
        let _ = rd.recordings(&mut m, |r| output(r.id)).await;
        let mut tracks = [TrackSummary::default(); 8];
        if let Ok(Some(s)) = rd
            .summary::<8, _>(&mut m, input() as u64, &mut tracks)
            .await
        {
            output(s.records as u32);
        }
        let mut buf = [0u8; 256];
        if let Ok(mut w) = rd
            .seek_track(&mut m, input() as u64, input() as u64, input() as u64)
            .await
        {
            while let Ok(Some(f)) = w.next(&mut rd, &mut m, &mut buf).await {
                output(f.head.pos as u32);
            }
        }
        if let Some(t) = now()
            && let Ok(Some(mut w)) = rd.seek_time::<8, _>(&mut m, input(), &t).await
        {
            while let Ok(Some(f)) = w.next(&mut rd, &mut m, &mut buf).await {
                output(f.track);
            }
        }
        loop {
            output(rd.torn());
        }
    })
}
