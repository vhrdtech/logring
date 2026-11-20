//! `ring_laps.svg`: the data segments of a ring that went round, seq by seq, each in slot `seq mod n`. The ring is
//! the drive's, shrunk to 6 data segments, after a minute of the bench.

use logring::tests_support::block_on;
use logring::{Buffers, Config};
use logring_example::SimCard;
use logring_example::bench::{self, Bench};
use logring_example::drive::{self, Drive};

use crate::OrDebug;
use crate::svg::Svg;

/// The drawn ring: the superblock segment, one meta segment and 6 data segments of 64 KiB.
const SEGMENT: u32 = 64 << 10;
const DATA: u32 = 6;
fn config() -> Config {
    Config {
        meta_segments: 1,
        ..drive::config()
    }
}

const CELL_W: f64 = 92.0;
const CELL_H: f64 = 34.0;
const GAP: f64 = 6.0;
const LEFT: f64 = 64.0;
const TOP: f64 = 40.0;

/// The figure.
pub fn ring_laps() -> anyhow::Result<String> {
    let mut card = vec![0u8; (2 + DATA as usize) * SEGMENT as usize];
    let mut bufs = Buffers::new();
    let (n, newest, oldest) = block_on(async {
        let mut drive = Drive::mount(SimCard::new(&mut card), &mut bufs, config())
            .await
            .or_debug()?;
        bench::run(&mut drive, Bench::FIRST_MINUTE)
            .await
            .or_debug()?;
        let lr = drive.logring();
        anyhow::Ok((lr.data_segments(), lr.data_seq(), lr.oldest_data_seq()))
    })?;
    anyhow::ensure!(n == u64::from(DATA), "{n} data segments");
    let laps = newest / n + 1;
    let width = LEFT + f64::from(DATA) * (CELL_W + GAP) + 8.0;
    let height = TOP + laps as f64 * (CELL_H + GAP) + 56.0;
    let mut svg = Svg::new(
        width,
        height,
        &format!(
            "Segment seqs 0 to {newest} of a ring of {n} data segments: seq s in slot s mod {n}; the card holds seqs {oldest} to {newest}"
        ),
    );
    for slot in 0..n {
        let x = LEFT + slot as f64 * (CELL_W + GAP) + CELL_W / 2.0;
        svg.text(x, TOP - 12.0, "middle", "soft", &format!("slot {slot}"));
    }
    for lap in 0..laps {
        let y = TOP + lap as f64 * (CELL_H + GAP);
        svg.text(
            LEFT - 10.0,
            y + CELL_H / 2.0 + 4.0,
            "end",
            "soft",
            &format!("lap {lap}"),
        );
        for slot in 0..n {
            let seq = lap * n + slot;
            if seq > newest {
                continue;
            }
            let x = LEFT + slot as f64 * (CELL_W + GAP);
            let (fill, text) = if seq == newest {
                ("tint-accent", "strong")
            } else if seq >= oldest {
                ("tint-blue", "")
            } else {
                ("faint", "soft")
            };
            svg.rect((x, y), (CELL_W, CELL_H), 2.0, fill);
            svg.rect((x, y), (CELL_W, CELL_H), 2.0, "thin");
            svg.text(
                x + CELL_W / 2.0,
                y + CELL_H / 2.0 + 4.0,
                "middle",
                text,
                &format!("seq {seq}"),
            );
        }
    }
    let ly = TOP + laps as f64 * (CELL_H + GAP) + 22.0;
    let legend: [(&str, &str); 3] = [
        ("faint", "overwritten"),
        ("tint-blue", "on the card"),
        ("tint-accent", "being written"),
    ];
    let mut lx = LEFT;
    for (class, text) in legend {
        svg.rect((lx, ly - 10.0), (14.0, 12.0), 0.0, class);
        svg.rect((lx, ly - 10.0), (14.0, 12.0), 0.0, "thin");
        svg.text(lx + 20.0, ly, "start", "", text);
        lx += 20.0 + 7.0 * text.len() as f64 + 28.0;
    }
    Ok(svg.finish())
}
