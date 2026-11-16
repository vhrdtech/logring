//! The `logring` commands as a library: `info`, `export`, `cat`, `check`, `make-image`. The `logring` binary runs
//! them, and so does `ww logring` (the `logring_ww_cli` crate registers them with `ww`'s command registry).
//!
//! See `AGENTS.md` and `FEATURES.md` (PC-1). `ls` (the USB view's tree) waits on VIEW-1; `prepare` on S2.

pub mod cli;
mod cmd;
mod color;
mod make_image;

pub use cli::{Command, EXAMPLES, Format};

/// Runs one command. `Ok(false)`: `check` found trouble, the program exits non-zero.
pub fn run(command: Command) -> anyhow::Result<bool> {
    match command {
        Command::Info { path } => cmd::info(&path).map(|()| true),
        Command::Export {
            path,
            track,
            format,
            out,
        } => cmd::export(&path, track.as_deref(), format, out).map(|()| true),
        Command::Cat { path, track } => cmd::cat(&path, &track).map(|()| true),
        Command::Check { path } => cmd::check(&path),
        Command::MakeImage {
            path,
            tracks,
            records,
        } => cmd::make_image(&path, tracks, records).map(|()| true),
    }
}

/// `program`'s help examples: those of [EXAMPLES] and of each command, the program named `program` instead of
/// `logring` (`ww logring`).
pub fn name_examples(command: clap::Command, program: &str) -> clap::Command {
    let rename = |text: &str| text.replace("    logring ", &format!("    {program} "));
    let command = match command.get_after_help() {
        Some(help) => {
            let help = rename(&help.to_string());
            command.after_help(help)
        }
        None => command,
    };
    command.mut_subcommands(|sub| name_examples(sub, program))
}
