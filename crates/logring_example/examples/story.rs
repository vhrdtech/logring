//! `cargo run -p logring_example --features std --example story`: every number the book's guide quotes, chapter by
//! chapter, from the runs of `logring_example::story`.

use logring_example::story;

fn main() {
    let m = story::first_minute();
    let t = m.totals;
    println!("# A first log (first_log.md)");
    println!(
        "first minute: {} log lines, {} modes, {} frames, {} voltages, {} recordings",
        t.log, t.mode, t.can0, t.vbat, t.recordings
    );
    println!(
        "{} data segments of {} KiB, {} flushes, {} KiB of the data ring, a frame item of {} bytes",
        m.segments,
        m.segment_bytes >> 10,
        m.flushes,
        m.data_bytes >> 10,
        m.frame_item_bytes
    );

    println!("\n# Tracks, sessions and recordings (tracks.md)");
    let b = story::two_boots();
    println!(
        "boot 1: {} frames; boot 2: {} frames, its first frame at position {}",
        b.first.can0, b.second.can0, b.second_first_frame_pos
    );
    println!("data segments by session: {:?}", b.segment_sessions);
    for r in &b.system {
        println!("system track: {r:?}");
    }

    println!("\n# Flushes and power cuts (flushes.md)");
    println!("| flush_bytes | flushes | by the tick | data KiB | padding KiB |");
    for r in story::flush_sizes() {
        println!(
            "| {} KiB | {} | {} | {} | {} |",
            r.flush_bytes >> 10,
            r.flushes,
            r.ticked,
            r.data_bytes >> 10,
            r.padding_bytes >> 10
        );
    }
    let c = story::power_cut();
    println!(
        "power cut after {} bytes, at bench time {} ms: pushed {} frames, {} modes, {} log lines",
        c.budget, c.pushed.t_ms, c.pushed.can0, c.pushed.mode, c.pushed.log
    );
    println!(
        "next mount: {} torn blocks, {} torn records; on the card {} frames (lost {}), {} modes, newest frame at {} ms",
        c.recovery.torn_blocks,
        c.recovery.torn_records,
        c.on_card.can0,
        c.pushed.can0 - c.on_card.can0,
        c.on_card.mode,
        c.last_frame_ms
    );
    println!(
        "torn segment seq {}, next mount's segment seq {}",
        c.torn_seq, c.next_seq
    );

    println!("\n# The SD card (sd_card.md)");
    let s = story::sd_card();
    println!(
        "{} bytes card, AU {} MiB, segment {} MiB; first boot {:?}, second boot {:?}",
        s.card_bytes,
        s.geometry.au_bytes >> 20,
        s.segment_bytes >> 20,
        s.first,
        s.second
    );
    println!(
        "ring file at byte {} ({} MiB), {} bytes long ({} MiB); rw {} MiB; {} meta + {} data segments",
        s.ring_start,
        s.ring_start >> 20,
        s.ring_bytes,
        s.ring_bytes >> 20,
        s.rw_bytes >> 20,
        s.meta_segments,
        s.data_segments
    );
    println!(
        "a minute of the bench: {} KiB; the data ring holds {}.{} days",
        s.minute_bytes >> 10,
        s.days_tenths / 10,
        s.days_tenths % 10
    );

    println!("\n# When the ring is full (ring_full.md)");
    let f = story::ring_full();
    println!(
        "{} data segments; after {} s the newest seq is {}, the oldest {}, {} reused; {} superblock writes",
        f.data_segments,
        story::FULL_RUN.seconds,
        f.newest_seq,
        f.oldest_seq,
        f.segments_reused,
        f.superblock_writes
    );
    println!(
        "drop newest: refused at {} ms of the bench, {} frames kept",
        f.refused_at_ms, f.kept_frames
    );
}
