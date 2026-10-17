//! The `logring` command line tool: `info`, `ls`, `export`, `cat`, `check`, `make-image`.
//!
//! Scaffold only (P2628#16oct-1016, R0): subcommands are stubs until the reader (R5), view (R8) and PC export
//! (R10) land. See `AGENTS.md` and `FEATURES.md`.

use clap::{CommandFactory, Parser, Subcommand};

/// `logring <SUBCOMMAND>`: read a ww_logring card, image or split file.
#[derive(Parser)]
#[command(name = "logring", version = version())]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Card/image facts: sessions, recordings, tracks, counters, health.
    Info,
    /// The same tree the USB view shows (by recording, by time, by track).
    Ls,
    /// Export a track or recording to CSV, MCAP or MF4.
    Export,
    /// Decoded text of a track or time range.
    Cat,
    /// Check exFAT + ring binding + ring format, like fsck.
    Check,
    /// Format an image file and fill it from a simulated device (test data, demos).
    MakeImage,
    /// Print shell completion for the given shell.
    Completions { shell: clap_complete::Shell },
}

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
    let cli = Cli::parse();
    match cli.command {
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "logring",
                &mut std::io::stdout(),
            );
        }
        _ => anyhow::bail!("not implemented yet (P2628 R5/R8/R10)"),
    }
    Ok(())
}
