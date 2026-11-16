//! The `logring` command line tool: the commands of the `logring_cli` library, shell completion and `--version`.

use clap::{CommandFactory, Parser, Subcommand};

mod complete;

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

/// `logring <SUBCOMMAND>`: read a logring card, ring file, image or split file.
#[derive(Parser)]
#[command(
    name = "logring",
    version = version(),
    color = clap::ColorChoice::Auto,
    after_help = logring_cli::EXAMPLES,
)]
struct Cli {
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    #[command(flatten)]
    Logring(logring_cli::Command),
    /// Print shell completion for the given shell (bash, zsh, fish, elvish, powershell): source the output, or
    /// `source <(COMPLETE=bash logring)` for the live, dynamic form (`just install`'s README).
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// The API snapshots of every crate in the build's lock: a bundle's skipped standard types resolve with nothing on
/// disk and no network.
static SNAPSHOTS: ww_snapshots::Set = include!(concat!(env!("OUT_DIR"), "/snapshot_set.rs"));

fn main() -> anyhow::Result<()> {
    ww_snapshots::embed(&SNAPSHOTS);
    // answers shell completion requests (COMPLETE=<shell> set) and exits, otherwise does nothing
    complete::complete(Cli::command);

    match Cli::parse().command {
        TopCommand::Logring(command) => {
            if logring_cli::run(command)? {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        TopCommand::Completions { shell } => {
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
