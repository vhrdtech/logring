//! `trailer.svg`: one block's 8-byte trailer, from the same ring `flush.rs` draws: the tag that says the block
//! belongs to this segment and lap, the flush offset that finds the flush it is part of, and the CRC-32C that
//! catches a torn write.

use anyhow::{Context, ensure};
use logring::format::{BLOCK, PAYLOAD, TRAILER};

use crate::flush::written;
use crate::svg::Svg;

/// The block drawn: not the segment's first, so its flush offset is worth showing.
const BLOCK_INDEX: usize = 4;

const BYTE_W: f64 = 54.0;
const BYTE_H: f64 = 34.0;
const GAP: f64 = 3.0;

/// The figure.
pub fn trailer() -> anyhow::Result<String> {
    let (card, scan) = written()?;
    let seg = scan.data.first().context("a data segment")?;
    ensure!(
        (BLOCK_INDEX as u32) < seg.blocks,
        "the segment has only {} blocks",
        seg.blocks
    );
    let sb = &scan.superblock;
    let slot = 1 + u64::from(sb.meta_segments) + seg.seq % u64::from(sb.data_segments);
    let base = usize::try_from(slot * u64::from(sb.segment_size))?;
    let at = base + BLOCK_INDEX * BLOCK + PAYLOAD;
    let trailer: [u8; TRAILER] = card
        .get(at..at + TRAILER)
        .context("the trailer inside the card")?
        .try_into()
        .expect("TRAILER bytes");
    let tag = u16::from_le_bytes([trailer[0], trailer[1]]);
    let flush_off = u16::from_le_bytes([trailer[2], trailer[3]]);
    let crc = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);

    let width = TRAILER as f64 * (BYTE_W + GAP) + 16.0;
    let mut svg = Svg::new(
        width,
        92.0,
        &format!(
            "Block {BLOCK_INDEX}'s trailer: tag {tag:#06x}, flush offset {flush_off}, CRC {crc:#010x}"
        ),
    );
    let tints = [
        "tint-blue",
        "tint-blue",
        "tint-amber",
        "tint-amber",
        "tint-teal",
        "tint-teal",
        "tint-teal",
        "tint-teal",
    ];
    for (i, tint) in tints.iter().enumerate() {
        let x = 8.0 + i as f64 * (BYTE_W + GAP);
        svg.rect((x, 8.0), (BYTE_W, BYTE_H), 0.0, tint);
        svg.rect((x, 8.0), (BYTE_W, BYTE_H), 0.0, "thin");
        svg.text(
            x + BYTE_W / 2.0,
            8.0 + BYTE_H / 2.0 + 4.0,
            "middle",
            "mono",
            &format!("{:02x}", trailer[i]),
        );
    }
    let fields = [
        (0usize, 2usize, "tag", format!("{tag:#06x}")),
        (2, 2, "flush offset", flush_off.to_string()),
        (4, 4, "CRC-32C", format!("{crc:#010x}")),
    ];
    for (start, len, name, value) in fields {
        let x0 = 8.0 + start as f64 * (BYTE_W + GAP);
        let x1 = 8.0 + (start + len) as f64 * (BYTE_W + GAP) - GAP;
        svg.bracket_under(x0, x1, 8.0 + BYTE_H + 8.0, "line");
        svg.text(
            (x0 + x1) / 2.0,
            8.0 + BYTE_H + 22.0,
            "middle",
            "",
            &format!("{name}: {value}"),
        );
    }
    Ok(svg.finish())
}
