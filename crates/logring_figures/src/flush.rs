//! `flush.svg`: the first data segment of a ring the drive wrote for four seconds, block by block. Each block
//! belongs to a flush, which its trailer's `flush_off` says; a flush's records end inside its last block and padding
//! fills the rest of it.

use anyhow::Context;
use logring::Config;
use logring::format::{BLOCK, PAYLOAD};
use logring::scan::Scan;
use logring::tests_support::block_on;
use logring_example::SimCard;
use logring_example::bench::Bench;
use logring_example::story;

use crate::OrDebug;
use crate::svg::Svg;

/// The drawn ring: 8 KiB segments of 16 blocks, flushes of 1 KiB, so a few seconds fill most of a segment.
const SEGMENT: u32 = 8 << 10;
fn config() -> Config {
    Config {
        segment_size: SEGMENT,
        meta_segments: 1,
        flush_bytes: 1 << 10,
        ..logring_example::drive::config()
    }
}
/// The bench: four seconds, 100 frames a second, so that `flush_bytes` ends some flushes and the tick others.
const BENCH: Bench = Bench {
    seconds: 4,
    can_hz: 100,
    fault_at_s: None,
};

/// Blocks per row and the drawn size of a block.
const PER_ROW: u32 = 8;
const BLOCK_W: f64 = 96.0;
const BLOCK_H: f64 = 34.0;
const GAP: f64 = 4.0;
const ROW_H: f64 = 74.0;
/// The trailer's drawn width: its 8 bytes four times as wide as the payload's, so they show.
const TRAILER_W: f64 = BLOCK_W * 8.0 / BLOCK as f64 * 4.0;
/// The tint of flush `k`.
const TINTS: [&str; 3] = ["tint-blue", "tint-teal", "tint-amber"];

/// The ring on a simulated card after the bench, and its scan.
fn written() -> anyhow::Result<(Vec<u8>, Scan)> {
    let mut card = vec![0u8; 8 * SEGMENT as usize];
    let (_, err) = story::boot(SimCard::new(&mut card), config(), BENCH);
    if let Some(e) = err {
        anyhow::bail!("the bench: {e:?}");
    }
    let scan = block_on(Scan::open(&mut SimCard::new(&mut card), Some(SEGMENT))).or_debug()?;
    Ok((card, scan))
}

/// The figure.
pub fn flush() -> anyhow::Result<String> {
    let (card, scan) = written()?;
    let seg = scan.data.first().context("a data segment")?;
    let flushes = story::flushes(&card, &scan, seg);
    let total = SEGMENT / BLOCK as u32;
    let rows = total.div_ceil(PER_ROW);
    let width = f64::from(PER_ROW) * (BLOCK_W + GAP) + 16.0;
    let height = f64::from(rows) * ROW_H + 64.0;
    let mut svg = Svg::new(
        width,
        height,
        &format!(
            "The first data segment of the drive's ring: {} flushes in {} of its {total} blocks, each padded to its last block's end",
            flushes.len(),
            seg.blocks
        ),
    );
    let pos = |b: u32| {
        let x = 8.0 + f64::from(b % PER_ROW) * (BLOCK_W + GAP);
        let y = 8.0 + f64::from(b / PER_ROW) * ROW_H;
        (x, y)
    };
    let payload_w = BLOCK_W - TRAILER_W;
    for (k, f) in flushes.iter().enumerate() {
        let tint = TINTS[k % TINTS.len()];
        for i in 0..f.blocks {
            let b = f.first_block + i;
            let (x, y) = pos(b);
            let used = if i + 1 == f.blocks {
                f.last_used as f64 / PAYLOAD as f64
            } else {
                1.0
            };
            svg.rect((x, y), (payload_w * used, BLOCK_H), 0.0, tint);
            if used < 1.0 {
                svg.rect(
                    (x + payload_w * used, y),
                    (payload_w * (1.0 - used), BLOCK_H),
                    0.0,
                    "faint hatch",
                );
            }
            svg.rect((x + payload_w, y), (TRAILER_W, BLOCK_H), 0.0, "tint-ink");
            svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, "thin");
            svg.text(x + 4.0, y + 12.0, "start", "mono", &b.to_string());
        }
        let (x0, y0) = pos(f.first_block);
        let (x1, y1) = pos(f.first_block + f.blocks - 1);
        let records = match f.records {
            1 => "1 record".to_owned(),
            n => format!("{n} records"),
        };
        let label = if f.first_block == 0 {
            format!("head, {records}")
        } else {
            records
        };
        if y0 == y1 {
            svg.bracket_under(x0, x1 + BLOCK_W, y0 + BLOCK_H + 8.0, "line");
            svg.text(
                (x0 + x1 + BLOCK_W) / 2.0,
                y0 + BLOCK_H + 22.0,
                "middle",
                "",
                &label,
            );
        } else {
            svg.bracket_under(
                x0,
                8.0 + f64::from(PER_ROW) * (BLOCK_W + GAP) - GAP,
                y0 + BLOCK_H + 8.0,
                "line",
            );
            svg.text(x0 + 4.0, y0 + BLOCK_H + 22.0, "start", "", &label);
            svg.bracket_under(8.0, x1 + BLOCK_W, y1 + BLOCK_H + 8.0, "line");
        }
    }
    for b in seg.blocks..total {
        let (x, y) = pos(b);
        svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, "thin");
        svg.text(x + 4.0, y + 12.0, "start", "mono soft", &b.to_string());
    }
    let ly = f64::from(rows) * ROW_H + 24.0;
    svg.rect((8.0, ly - 10.0), (14.0, 12.0), 0.0, "tint-blue");
    svg.text(28.0, ly, "start", "", "a flush: its head and records");
    svg.rect((230.0, ly - 10.0), (14.0, 12.0), 0.0, "faint hatch");
    svg.text(250.0, ly, "start", "", "padding");
    svg.rect((330.0, ly - 10.0), (TRAILER_W, 12.0), 0.0, "tint-ink");
    svg.text(
        330.0 + TRAILER_W + 6.0,
        ly,
        "start",
        "",
        "trailer, 8 bytes (drawn 4 times wider)",
    );
    if seg.blocks < total {
        svg.text(
            8.0,
            ly + 22.0,
            "start",
            "soft",
            "blocks without a fill: not written yet",
        );
    }
    Ok(svg.finish())
}
