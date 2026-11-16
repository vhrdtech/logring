//! Writes the API snapshots of every crate in the lock that has them (`ww_snapshots::build`), embedded so a bundle's
//! skipped standard types resolve offline.

fn main() {
    ww_snapshots::build::workspace_set("snapshot_set.rs")
        .unwrap_or_else(|e| panic!("embedding API snapshots: {e:#}"));
}
