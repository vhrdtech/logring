//! Loads the `Drive` API's bundle from this crate's own source and writes it, shrink_wrap-encoded, to
//! `OUT_DIR/drive_bundle.bin`: `api.rs` `include_bytes!`s it as [api::DRIVE_BUNDLE], so the drive's session
//! stores its own bundle, as real firmware that keeps one does.

use wire_weaver::shrink_wrap::SerializeShrinkWrapOwned;

fn main() {
    let crate_path = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let (bundle, _paths) =
        wire_weaver_core::load_own_with_paths(&crate_path, Some("Drive".to_string()))
            .expect("loading the Drive API bundle from source");
    let bytes = bundle
        .to_ww_bytes_owned()
        .expect("encoding the Drive API bundle");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    std::fs::write(
        std::path::Path::new(&out_dir).join("drive_bundle.bin"),
        bytes,
    )
    .expect("writing drive_bundle.bin");
    println!("cargo::rerun-if-changed=src/api.rs");
}
