//! Colour for `info`/`check`'s own output (clap's `--help`/error colour follows `ColorChoice::Auto` on its
//! own): off when stdout isn't a terminal, or `NO_COLOR` is set (tpm CLAUDE.md "CLI quality").

use std::io::IsTerminal;

pub(crate) fn enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Bold red, plain when colour is off.
pub(crate) fn bad(s: &str) -> String {
    if enabled() {
        format!("\x1b[1;31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Bold green, plain when colour is off.
pub(crate) fn good(s: &str) -> String {
    if enabled() {
        format!("\x1b[1;32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Bold, plain when colour is off.
pub(crate) fn bold(s: &str) -> String {
    if enabled() {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Dim, plain when colour is off.
pub(crate) fn dim(s: &str) -> String {
    if enabled() {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
