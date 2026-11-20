// Every figure in `docs/plots/` is the one `logring_figures` draws from the format as it is.

use std::path::Path;

#[test]
fn figures_in_docs_are_current() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/plots");
    let mut stale = Vec::new();
    for (name, svg) in logring_figures::figures().unwrap() {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg.as_str()) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "docs/plots/{stale:?} differ from what logring_figures draws: run `just book-figures` and check them"
    );
}
