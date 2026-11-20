//! `power_cut.svg`: the data segment a power cut tore, block by block, and the segment the next mount opened. The
//! ring is the one of `story::power_cut`: the drive's own layout, the power failing in the middle of a flush.

use anyhow::Context;
use logring::format::{BLOCK, BlockCheck, Role, check_block};
use logring::scan::Scan;
use logring_example::drive;
use logring_example::story::{self, PowerCut};

use crate::svg::Svg;

/// Blocks per row and the drawn size of a block.
const PER_ROW: u32 = 32;
const BLOCK_W: f64 = 22.0;
const BLOCK_H: f64 = 26.0;
const GAP: f64 = 2.0;
const ROW_H: f64 = 34.0;
const LEFT: f64 = 8.0;
/// Tints of the flushes, in turn.
const TINTS: [&str; 2] = ["tint-blue", "tint-teal"];

/// What a block of the segment holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// A block of flush `k` of the segment.
    Flush(usize),
    Torn,
    Empty,
}

/// The blocks of data segment `seq` in `card`.
fn cells(card: &[u8], scan: &Scan, seq: u64) -> anyhow::Result<Vec<Cell>> {
    let base = story::segment_base(scan, seq);
    let blocks = scan.superblock.segment_size as usize / BLOCK;
    let mut out = Vec::with_capacity(blocks);
    let mut flush = None;
    for i in 0..blocks {
        let at = base + i * BLOCK;
        let block: &[u8; BLOCK] = card
            .get(at..at + BLOCK)
            .context("a block inside the card")?
            .try_into()?;
        let c = match check_block(
            block,
            scan.superblock.log_id,
            Role::Data,
            seq,
            u32::try_from(i)?,
        ) {
            BlockCheck::Ok { flush_off: 0 } => {
                let k = flush.map_or(0, |k| k + 1);
                flush = Some(k);
                Cell::Flush(k)
            }
            BlockCheck::Ok { .. } => Cell::Flush(flush.context("a flush starts at block 0")?),
            BlockCheck::Torn => Cell::Torn,
            BlockCheck::Other => Cell::Empty,
        };
        out.push(c);
    }
    Ok(out)
}

fn pos(b: usize, top: f64) -> (f64, f64) {
    let b = u32::try_from(b).unwrap_or(u32::MAX);
    let x = LEFT + f64::from(b % PER_ROW) * (BLOCK_W + GAP);
    let y = top + f64::from(b / PER_ROW) * ROW_H;
    (x, y)
}

fn draw_cells(svg: &mut Svg, cells: &[Cell], top: f64, newest: &str) {
    let last_flush = cells.iter().filter_map(|c| match c {
        Cell::Flush(k) => Some(*k),
        _ => None,
    });
    let last_flush = last_flush.max();
    for (b, c) in cells.iter().enumerate() {
        let (x, y) = pos(b, top);
        match c {
            Cell::Flush(k) if Some(*k) == last_flush => {
                svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, newest);
            }
            Cell::Flush(k) => svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, TINTS[k % TINTS.len()]),
            Cell::Torn => svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, "tint-accent"),
            Cell::Empty => {}
        }
        svg.rect((x, y), (BLOCK_W, BLOCK_H), 0.0, "thin");
    }
}

/// The figure.
pub fn power_cut() -> anyhow::Result<String> {
    let PowerCut {
        mut remounted,
        torn_seq,
        next_seq,
        recovery,
        ..
    } = story::power_cut();
    let scan = story::scan(&mut remounted, drive::config());
    let torn = cells(&remounted, &scan, torn_seq)?;
    let next = cells(&remounted, &scan, next_seq)?;
    let rows = u32::try_from(torn.len())?.div_ceil(PER_ROW);
    let next_rows = u32::try_from(next.iter().rposition(|c| *c != Cell::Empty).unwrap_or(0) + 1)?
        .div_ceil(PER_ROW);
    let width = LEFT * 2.0 + f64::from(PER_ROW) * (BLOCK_W + GAP);
    let top = 28.0;
    let next_top = top + f64::from(rows) * ROW_H + 52.0;
    let height = next_top + f64::from(next_rows) * ROW_H + 64.0;
    let torn_at = torn
        .iter()
        .position(|c| *c == Cell::Torn)
        .context("a torn block")?;
    let mut svg = Svg::new(
        width,
        height,
        &format!(
            "Data segment {torn_seq} after the power cut: its flushes, the interrupted one ending in torn block {torn_at}; the next mount opened segment {next_seq}"
        ),
    );
    svg.text(
        LEFT,
        top - 10.0,
        "start",
        "strong",
        &format!("segment seq {torn_seq}, session 1: as the power cut left it"),
    );
    draw_cells(&mut svg, &torn, top, "tint-amber");
    let (x, y) = pos(torn_at, top);
    svg.path(
        &format!("M{:.1} {:.1} v14", x + BLOCK_W / 2.0, y + BLOCK_H + 2.0),
        "accent-line",
    );
    svg.text(
        x + BLOCK_W / 2.0,
        y + BLOCK_H + 28.0,
        "middle",
        "accent small",
        &format!("block {torn_at}: torn, its CRC fails"),
    );
    svg.text(
        LEFT,
        next_top - 10.0,
        "start",
        "strong",
        &format!(
            "segment seq {next_seq}, session {}: the next mount",
            recovery.session
        ),
    );
    let shown = usize::try_from(next_rows * PER_ROW)?.min(next.len());
    draw_cells(&mut svg, &next[..shown], next_top, "tint-green");
    let ly = next_top + f64::from(next_rows) * ROW_H + 18.0;
    svg.rect((LEFT, ly - 10.0), (7.0, 12.0), 0.0, "tint-blue");
    svg.rect((LEFT + 7.0, ly - 10.0), (7.0, 12.0), 0.0, "tint-teal");
    let legend: [(&str, &str); 4] = [
        ("", "complete flushes"),
        ("tint-amber", "the flush being written"),
        ("tint-accent", "torn block"),
        ("tint-green", "the new session's first flushes"),
    ];
    let mut lx = LEFT;
    for (class, text) in legend {
        if !class.is_empty() {
            svg.rect((lx, ly - 10.0), (14.0, 12.0), 0.0, class);
        }
        svg.text(lx + 20.0, ly, "start", "", text);
        lx += 20.0 + 7.0 * text.len() as f64 + 24.0;
    }
    svg.text(
        LEFT,
        ly + 22.0,
        "start",
        "soft",
        "blocks without a fill: not written in this lap",
    );
    Ok(svg.finish())
}
