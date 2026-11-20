//! `logring_figures DIR`: write every figure of the book pages into `DIR` (`just book-figures`: `docs/plots`).

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir: PathBuf = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("usage: logring_figures DIR"))?;
    std::fs::create_dir_all(&dir)?;
    for (name, svg) in logring_figures::figures()? {
        std::fs::write(dir.join(name), svg)?;
    }
    Ok(())
}
