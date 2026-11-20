//! The figures of logring's book pages that show bytes, blocks or numbers, each drawn from a ring that the drive
//! example (`logring_example`) writes on a simulated card and logring reads back, so a figure cannot drift from the
//! format:
//!
//! - `flush.svg`: the first blocks of a data segment, grouped into flushes, each flush padded to its last block's
//!   end, every block ending in its 8-byte trailer.
//!
//! [figures] draws them; `logring_figures DIR` writes them into `DIR` (`just book-figures`: `docs/plots`).

mod flush;
mod svg;

/// Every figure: its file name and its SVG text.
pub fn figures() -> anyhow::Result<Vec<(&'static str, String)>> {
    Ok(vec![("flush.svg", flush::flush()?)])
}

/// logring's errors that are not [std::error::Error], as [anyhow::Error] by their [Debug](core::fmt::Debug) text.
trait OrDebug<T> {
    fn or_debug(self) -> anyhow::Result<T>;
}

impl<T, E: core::fmt::Debug> OrDebug<T> for Result<T, E> {
    fn or_debug(self) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}
