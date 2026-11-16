//! The `logring` command line tool: `info`, `export`, `cat`, `check`, `make-image`.
//!
//! See `AGENTS.md` and `FEATURES.md` (PC-1). `ls` (the USB view's tree) waits on VIEW-1;
//! `prepare` on S2.

use clap::{CommandFactory, Parser};
use cli::{Cli, Command};

mod cli;
mod cmd;
mod color;
mod complete;
mod make_image;

fn version() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("GIT_SHA"),
        ", commit ",
        env!("COMMIT_TIME"),
        ")"
    )
}

/// The API snapshots of every crate in the build's lock: a bundle's skipped standard types resolve with nothing on
/// disk and no network.
static SNAPSHOTS: ww_snapshots::Set = include!(concat!(env!("OUT_DIR"), "/snapshot_set.rs"));

fn main() -> anyhow::Result<()> {
    ww_snapshots::embed(&SNAPSHOTS);
    // answers shell completion requests (COMPLETE=<shell> set) and exits, otherwise does nothing
    complete::complete(Cli::command);

    let cli = Cli::parse();
    match cli.command {
        Command::Info { path } => cmd::info(&path),
        Command::Export {
            path,
            track,
            format,
            out,
        } => cmd::export(&path, track.as_deref(), format, out),
        Command::Cat { path, track } => cmd::cat(&path, &track),
        Command::Check { path } => {
            if cmd::check(&path)? {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        Command::MakeImage {
            path,
            tracks,
            records,
        } => cmd::make_image(&path, tracks, records),
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "logring",
                &mut std::io::stdout(),
            );
            Ok(())
        }
    }
}
