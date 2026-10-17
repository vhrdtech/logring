//! `logring`'s command line: `info`, `export`, `cat`, `check`, `make-image` (`ls` stays out until VIEW-1, the
//! USB view's tree; `prepare` is S2's).

use clap::{Parser, Subcommand, ValueEnum, ValueHint};
use std::path::PathBuf;

/// `logring <SUBCOMMAND>`: read a logring card, ring file, image or split file.
#[derive(Parser)]
#[command(
    name = "logring",
    version = crate::version(),
    color = clap::ColorChoice::Auto,
    after_help = "\
EXAMPLES:
    logring info recording.wlr
    logring export recording.wlr --track can.rx --format mf4 -o can.mf4
    logring export recording.wlr --format mcap -o recording.mcap
    logring cat recording.wlr --track system.markers
    logring check /media/card
    logring make-image demo.wlr --tracks 3 --records 500"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Card/image facts: sessions, bundles, tracks and their record counts.
    #[command(
        after_help = "EXAMPLES:\n    logring info recording.wlr\n    logring info /media/card"
    )]
    Info {
        /// A ring file, raw image, split `.wlr`, or a directory holding `logring.wlr` (a mounted card).
        #[arg(value_hint = ValueHint::AnyPath)]
        path: PathBuf,
    },
    /// Export a track (or, for MCAP, every resolved track) to CSV, MCAP or MF4.
    #[command(after_help = "\
EXAMPLES:
    logring export recording.wlr --track temperature --format csv -o temperature.csv
    logring export recording.wlr --track can.rx --format mf4 -o can.mf4
    logring export recording.wlr --format mcap -o recording.mcap
    logring export recording.wlr --format csv -o tracks/   (one CSV per track)")]
    Export {
        #[arg(value_hint = ValueHint::AnyPath)]
        path: PathBuf,
        /// The track's dotted name (`logring info` lists them) or stable key (`key.<16 hex digits>`); every
        /// resolved track when left out (MCAP: one file with a topic per track; CSV/MF4: one file per track
        /// under `--out`, which must then be a directory).
        #[arg(long)]
        track: Option<String>,
        #[arg(long, value_enum)]
        format: Format,
        /// Output file (`--track` given) or directory (every track); the current directory when left out.
        #[arg(short = 'o', long, value_hint = ValueHint::AnyPath)]
        out: Option<PathBuf>,
    },
    /// Decoded JSON, one line per record, of one track.
    #[command(after_help = "EXAMPLES:\n    logring cat recording.wlr --track temperature")]
    Cat {
        #[arg(value_hint = ValueHint::AnyPath)]
        path: PathBuf,
        /// The track's dotted name or stable key, as in `export`.
        #[arg(long)]
        track: String,
    },
    /// Check a ring like fsck: torn blocks/records, bundle and track resolution; non-zero exit on trouble.
    #[command(
        after_help = "EXAMPLES:\n    logring check recording.wlr\n    logring check /media/card"
    )]
    Check {
        #[arg(value_hint = ValueHint::AnyPath)]
        path: PathBuf,
    },
    /// Format a new image file and fill it from a simulated device (test data, demos).
    #[command(
        after_help = "EXAMPLES:\n    logring make-image demo.wlr\n    logring make-image demo.wlr --tracks 5 --records 2000"
    )]
    MakeImage {
        #[arg(value_hint = ValueHint::AnyPath)]
        path: PathBuf,
        /// Simulated numeric data tracks.
        #[arg(long, default_value_t = 3)]
        tracks: u32,
        /// Records pushed to each track.
        #[arg(long, default_value_t = 200)]
        records: u64,
    },
    /// Print shell completion for the given shell (bash, zsh, fish, elvish, powershell): source the output, or
    /// `source <(COMPLETE=bash logring)` for the live, dynamic form (`just install`'s README).
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Export format, `logring export --format`.
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    /// Flattened columns, one row per record.
    Csv,
    /// One JSON channel per track, with a JSON Schema (Foxglove MCAP).
    Mcap,
    /// ASAM MDF4: plain channels, or `CAN_DataFrame` bus logging for a CAN-shaped track.
    Mf4,
}
