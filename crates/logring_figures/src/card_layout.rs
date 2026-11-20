//! `card_layout.svg`: the drive's 32 GB SD card after the format, to scale, and the start of its ring file, segment
//! by segment, from `story::sd_card`.

use logring_example::story::{self, SdCard};

use crate::svg::Svg;

const LEFT: f64 = 8.0;
const BAR_W: f64 = 800.0;
const BAR_H: f64 = 30.0;
/// Segments of the ring file the second bar shows.
const SHOWN: u32 = 12;

const MIB: u64 = 1 << 20;

/// The figure.
pub fn card_layout() -> anyhow::Result<String> {
    let SdCard {
        card_bytes,
        ring_start,
        ring_bytes,
        rw_bytes,
        segment_bytes,
        meta_segments,
        data_segments,
        ..
    } = story::sd_card();
    let width = LEFT * 2.0 + BAR_W;
    let height = 250.0;
    let mut svg = Svg::new(
        width,
        height,
        &format!(
            "A {} GB card: the exFAT region and {} MiB left free for files, then the ring file of {} MiB: the superblock segment, {meta_segments} meta segments and {data_segments} data segments of {} MiB",
            card_bytes / 1_000_000_000,
            rw_bytes / MIB,
            ring_bytes / MIB,
            segment_bytes / (1 << 20)
        ),
    );
    // the card, to scale
    let scale = BAR_W / card_bytes as f64;
    let y = 30.0;
    svg.text(
        LEFT,
        y - 10.0,
        "start",
        "strong",
        &format!("the card, {} bytes, to scale", card_bytes),
    );
    let ring_x = LEFT + ring_start as f64 * scale;
    let ring_w = ring_bytes as f64 * scale;
    svg.rect((LEFT, y), (ring_x - LEFT, BAR_H), 0.0, "tint-green");
    svg.rect((ring_x, y), (ring_w, BAR_H), 0.0, "tint-blue");
    svg.rect((LEFT, y), (BAR_W, BAR_H), 0.0, "thin");
    svg.text(
        ring_x + ring_w / 2.0,
        y + BAR_H / 2.0 + 4.0,
        "middle",
        "",
        &format!(
            "logring.wlr: {} MiB from byte {} on",
            ring_bytes / MIB,
            ring_start
        ),
    );
    svg.path(
        &format!("M{:.1} {:.1} v12", LEFT + 1.0, y + BAR_H + 2.0),
        "line",
    );
    svg.text(
        LEFT,
        y + BAR_H + 28.0,
        "start",
        "small",
        &format!(
            "first {} MiB: partition table, exFAT, README.txt and {} MiB free for files",
            ring_start / MIB,
            rw_bytes / MIB
        ),
    );
    // the start of the ring file, segment by segment
    let y2 = 130.0;
    svg.text(
        LEFT,
        y2 - 10.0,
        "start",
        "strong",
        &format!(
            "the ring file's first {SHOWN} segments of {} MiB",
            segment_bytes / (1 << 20)
        ),
    );
    let seg_w = BAR_W / f64::from(SHOWN);
    for i in 0..SHOWN {
        let x = LEFT + f64::from(i) * seg_w;
        let (class, label) = if i == 0 {
            ("faint", "superblock".to_owned())
        } else if i <= meta_segments {
            ("tint-amber", format!("meta {}", i - 1))
        } else {
            ("tint-blue", format!("data {}", i - 1 - meta_segments))
        };
        svg.rect((x, y2), (seg_w, BAR_H), 0.0, class);
        svg.rect((x, y2), (seg_w, BAR_H), 0.0, "thin");
        svg.text(
            x + seg_w / 2.0,
            y2 + BAR_H / 2.0 + 4.0,
            "middle",
            "small",
            &label,
        );
    }
    // the two superblock slots: block 0 and the block half a segment in
    for (k, at) in [("A", 0.0), ("B", seg_w / 2.0)] {
        let x = LEFT + at;
        svg.rect((x, y2), (3.0, BAR_H), 0.0, "tint-accent");
        svg.text(
            x + 2.0,
            y2 + BAR_H + 16.0,
            "start",
            "accent small",
            &format!("slot {k}"),
        );
    }
    svg.bracket_under(
        LEFT + seg_w * f64::from(1 + meta_segments),
        LEFT + BAR_W,
        y2 + BAR_H + 30.0,
        "line",
    );
    svg.text(
        LEFT + seg_w * f64::from(1 + meta_segments) + 4.0,
        y2 + BAR_H + 46.0,
        "start",
        "small",
        &format!(
            "the data ring: {data_segments} segments, data 0 to data {}",
            data_segments - 1
        ),
    );
    Ok(svg.finish())
}
