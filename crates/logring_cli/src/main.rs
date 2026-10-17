//! The `logring` command line tool: `info`, `export`, `cat`, `check`, `make-image`.
//!
//! See `AGENTS.md` and `FEATURES.md` (PC-1, P2628#16oct-1001h). `ls` (the USB view's tree) waits on VIEW-1;
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

fn main() -> anyhow::Result<()> {
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
