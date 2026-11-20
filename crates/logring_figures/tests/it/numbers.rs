// Every number the guide's pages quote is one the drive example's story prints: the test runs the story and finds
// each number, in its sentence, on its page. A change to the example that moves a number fails here until the page
// says the new one.

use logring_example::drive::{self, Error};
use logring_example::sd::Mounted;
use logring_example::story::{self, SystemRecord};
use std::path::Path;

fn page(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .join(name);
    std::fs::read_to_string(&path).unwrap()
}

/// `n` as the book writes it: digits grouped by spaces from 10 000 on.
fn num(n: u64) -> String {
    let digits = n.to_string();
    if n < 10_000 {
        return digits;
    }
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The page `name` with its lines joined, so a phrase may cross a line break.
fn text(name: &str) -> String {
    page(name).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has(page: &str, text: &str, phrase: &str) {
    assert!(
        text.contains(phrase),
        "docs/{page} does not say \"{phrase}\""
    );
}

#[test]
fn first_log_quotes_the_first_minute() {
    let m = story::first_minute();
    let t = text("first_log.md");
    let p = "first_log.md";
    has(
        p,
        &t,
        &format!(
            "{} frames, {} voltages",
            num(m.totals.can0.into()),
            m.totals.vbat
        ),
    );
    has(
        p,
        &t,
        &format!(
            "{} log lines and {} changes of mode",
            m.totals.log, m.totals.mode
        ),
    );
    has(p, &t, &format!("in {} flushes", m.flushes));
    has(
        p,
        &t,
        &format!(
            "filled {} segments of {} KiB",
            m.segments,
            m.segment_bytes >> 10
        ),
    );
    has(
        p,
        &t,
        &format!("take {} KiB of the card", m.data_bytes >> 10),
    );
    has(p, &t, &format!("item is {} bytes", m.frame_item_bytes));
    has(
        p,
        &t,
        &format!("the size of the [[stage buffer]], {} bytes", drive::BUF),
    );
}

#[test]
fn tracks_quotes_the_two_boots() {
    let b = story::two_boots();
    let t = text("tracks.md");
    let p = "tracks.md";
    has(
        p,
        &t,
        &format!("{} seconds into a run", story::BOOT_1.fault_at_s.unwrap()),
    );
    has(
        p,
        &t,
        &format!("the first boot records {} frames", num(b.first.can0.into())),
    );
    has(
        p,
        &t,
        &format!(
            "first frame gets position {}",
            num(b.second_first_frame_pos)
        ),
    );
    let segments = |s: u32| b.segment_sessions.iter().filter(|&&x| x == s).count();
    has(
        p,
        &t,
        &format!(
            "first boot runs for {} seconds and writes {} segments",
            story::BOOT_1.seconds,
            segments(1)
        ),
    );
    has(
        p,
        &t,
        &format!(
            "{} seconds long, writes {} segments",
            story::BOOT_2.seconds,
            segments(2)
        ),
    );
    let rows: Vec<String> = b
        .system
        .iter()
        .map(|r| match r {
            SystemRecord::SessionStart { session } => {
                format!("| session start | session {session} |")
            }
            SystemRecord::RecordingStart { id, trigger } => {
                format!("| recording start | id {id}, trigger {trigger} |")
            }
            SystemRecord::RecordingStop { id, reason } => {
                format!("| recording stop | id {id}, reason {reason} |")
            }
            SystemRecord::Marker { kind, value } => {
                format!("| marker | kind {kind}, value {} |", value.unwrap())
            }
        })
        .collect();
    has(p, &t, &rows.join(" "));
}

#[test]
fn flushes_quotes_the_flush_sizes_and_the_power_cut() {
    let t = text("flushes.md");
    let p = "flushes.md";
    for r in story::flush_sizes() {
        has(
            p,
            &t,
            &format!(
                "| {} KiB | {} | {} | {} KiB | {} KiB | {} |",
                r.flush_bytes >> 10,
                r.flushes,
                r.ticked,
                r.data_bytes >> 10,
                r.padding_bytes >> 10,
                r.frames_in_ram_max
            ),
        );
    }
    let first = story::first_minute();
    has(p, &t, &format!("stage buffer holds {} bytes", drive::BUF));
    has(
        p,
        &t,
        &format!(
            "than the {} that the stage buffer can hold",
            first.totals.frames_in_ram_max
        ),
    );
    // the drive's flush is larger than its stage buffer: the stage buffer bounds the loss
    assert!(
        drive::config().flush_bytes as usize > drive::BUF,
        "the page says a flush is larger"
    );
    let c = story::power_cut();
    has(p, &t, &format!("has written {} bytes", num(c.budget)));
    has(
        p,
        &t,
        &format!("{} seconds into the run", c.pushed.t_ms / 1000),
    );
    has(
        p,
        &t,
        &format!("had pushed {} frames", num(c.pushed.can0.into())),
    );
    has(
        p,
        &t,
        &format!("holds {} of them", num(c.on_card.can0.into())),
    );
    has(
        p,
        &t,
        &format!("the cut cost {} frames", c.pushed.can0 - c.on_card.can0),
    );
    assert_eq!(
        c.on_card.mode, c.pushed.mode,
        "every change of mode is on the card"
    );
    assert_eq!(c.pushed.mode, 2, "the page says both changes of mode");
    has(
        p,
        &t,
        &format!(
            "counts {} torn block and {} torn record",
            c.recovery.torn_blocks, c.recovery.torn_records
        ),
    );
    has(
        p,
        &t,
        &format!("whose seq {} follows the torn one", c.next_seq),
    );
    assert_eq!(
        c.next_seq,
        c.torn_seq + 1,
        "the next segment follows the torn one"
    );
}

#[test]
fn sd_card_quotes_the_card() {
    let s = story::sd_card();
    let t = text("sd_card.md");
    let p = "sd_card.md";
    assert_eq!((s.first, s.second), (Mounted::Formatted, Mounted::Bound));
    has(
        p,
        &t,
        &format!("card of {} GB", s.card_bytes / 1_000_000_000),
    );
    has(
        p,
        &t,
        &format!(
            "starts at byte {}, {} MiB into the card",
            num(s.ring_start),
            s.ring_start >> 20
        ),
    );
    has(
        p,
        &t,
        &format!("and is {} MiB long", num(s.ring_bytes >> 20)),
    );
    has(
        p,
        &t,
        &format!("then the [[data ring]] with {} segments", s.data_segments),
    );
    has(p, &t, &format!("with {} segments", s.data_segments));
    has(p, &t, &format!("follows with {} segments", s.meta_segments));
    has(p, &t, &format!("{} MiB of free room", s.rw_bytes >> 20));
    has(
        p,
        &t,
        &format!(
            "reports {} MiB, so its segments are {} MiB",
            s.geometry.au_bytes >> 20,
            s.segment_bytes >> 20
        ),
    );
    has(
        p,
        &t,
        &format!("takes {} KiB of the card's data ring", s.minute_bytes >> 10),
    );
    has(
        p,
        &t,
        &format!("last {}.{} days", s.days_tenths / 10, s.days_tenths % 10),
    );
    has(
        p,
        &t,
        &format!(
            "The {} segments of {} MiB",
            s.data_segments,
            s.segment_bytes >> 20
        ),
    );
}

#[test]
fn ring_full_quotes_the_ten_minutes() {
    let f = story::ring_full();
    let s = story::sd_card();
    let t = text("ring_full.md");
    let p = "ring_full.md";
    let days = format!("{}.{} days", s.days_tenths / 10, s.days_tenths % 10);
    has(p, &t, &format!("holds the last {days}"));
    has(
        p,
        &t,
        &format!("Its ring has {} data segments", f.data_segments),
    );
    has(
        p,
        &t,
        &format!(
            "seqs {} to {} at the end of the run",
            f.oldest_seq, f.newest_seq
        ),
    );
    has(p, &t, &format!("overwrote {} segments", f.segments_reused));
    has(
        p,
        &t,
        &format!("wrote the superblock {} times", f.superblock_writes),
    );
    has(
        p,
        &t,
        &format!("up to seq {} in the same time", f.newest_seq),
    );
    has(
        p,
        &t,
        &format!(
            "until {}.{} seconds into the run",
            f.refused_at_ms / 1000,
            f.refused_at_ms % 1000 / 100
        ),
    );
    has(
        p,
        &t,
        &format!("the first {} frames", num(f.kept_frames.into())),
    );
    assert_eq!(
        f.switched_mount,
        Error::Ring(logring::Error::Full),
        "the page says the mount gets Full"
    );
    assert_eq!(story::FULL_RUN.seconds, 600, "the page says ten minutes");
}
