//! `ww logring`: the `logring` commands ([logring_cli]) registered with `ww`'s command registry
//! ([wire_weaver_cli::registry]). A binary that links this crate and `wire_weaver_cli` has them, with no plugin.

use anyhow::Result;
use clap::{FromArgMatches, Subcommand};
use wire_weaver_cli::registry::{BoxFuture, CommandDef, Ran, Run};

/// `ww logring`: the `logring` commands under one name, their help examples naming `ww logring`.
fn command() -> clap::Command {
    let command = logring_cli::Command::augment_subcommands(clap::Command::new("logring"))
        .about("Read a logring card, ring file, image or split file: info, export, cat, check, make-image")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .after_help(logring_cli::EXAMPLES);
    logring_cli::name_examples(command, "ww logring")
}

/// Runs the command `matches` hold (those of `ww logring`). `Ok(false)`: `check` found trouble.
fn run_matches(matches: &clap::ArgMatches) -> Result<bool> {
    logring_cli::run(logring_cli::Command::from_arg_matches(matches)?)
}

fn run(run: Run<'_>) -> BoxFuture<'_, Result<Ran>> {
    Box::pin(async move {
        let passed = run_matches(&run.matches)?;
        Ok(Ran {
            disconnect: true,
            passed,
        })
    })
}

inventory::submit! {
    CommandDef::new("logring", 230, command, run)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ww logring ARGS..` as `ww` parses it: the matches of `logring`.
    fn parse(args: &[&str]) -> clap::ArgMatches {
        let matches = wire_weaver_cli::cli::root_command()
            .try_get_matches_from(["ww", "logring"].iter().chain(args))
            .expect("ww parses the command line");
        let (name, sub) = matches.subcommand().expect("a subcommand");
        assert_eq!(name, "logring");
        sub.clone()
    }

    /// The registry has `logring`, with every command of the `logring` binary.
    #[test]
    fn registers_every_logring_command() {
        let def = wire_weaver_cli::registry::find("logring").expect("logring is registered");
        let names: Vec<String> = (def.command)()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        assert_eq!(names, ["info", "export", "cat", "check", "make-image"]);
    }

    /// The examples name `ww logring`, not the standalone binary.
    #[test]
    fn examples_name_ww_logring() {
        let mut command = command();
        let help = command.render_long_help().to_string();
        assert!(help.contains("    ww logring info recording.wlr"), "{help}");
        let export = command
            .find_subcommand_mut("export")
            .expect("export")
            .render_long_help()
            .to_string();
        assert!(
            export.contains("    ww logring export recording.wlr"),
            "{export}"
        );
        assert!(!export.contains("\n    logring "), "{export}");
    }

    /// `ww logring make-image`, `info`, `export` and `check` run on a file, as from a shell.
    #[test]
    fn runs_through_ww_parsing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let image = dir.path().join("demo.wlr");
        let image = image.to_str().expect("a UTF-8 path");
        let make = ["make-image", image, "--tracks", "2", "--records", "20"];
        assert!(run_matches(&parse(&make)).expect("make-image runs"));
        assert!(run_matches(&parse(&["info", image])).expect("info runs"));
        let csv = dir.path().join("sensor_0.csv");
        let csv = csv.to_str().expect("a UTF-8 path");
        let export = [
            "export", image, "--track", "sensor_0", "--format", "csv", "-o", csv,
        ];
        assert!(run_matches(&parse(&export)).expect("export runs"));
        assert!(
            std::fs::read_to_string(csv)
                .expect("the CSV")
                .lines()
                .count()
                > 20
        );
        assert!(run_matches(&parse(&["check", image])).expect("check runs"));
    }

    /// An unknown track: `cat` fails with the error the `logring` binary gives.
    #[test]
    fn cat_refuses_an_unknown_track() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let image = dir.path().join("demo.wlr");
        let image = image.to_str().expect("a UTF-8 path");
        run_matches(&parse(&[
            "make-image",
            image,
            "--tracks",
            "1",
            "--records",
            "5",
        ]))
        .expect("make-image runs");
        let err =
            run_matches(&parse(&["cat", image, "--track", "nope"])).expect_err("no such track");
        assert!(
            format!("{err:#}").contains("no track named or keyed"),
            "{err:#}"
        );
    }
}
