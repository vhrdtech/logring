//! Links the bins with `size.x`: flash and RAM regions big enough for any variant, nothing target-specific.
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rustc-link-arg-bins=-Tsize.x");
    println!("cargo:rerun-if-changed=size.x");
}
