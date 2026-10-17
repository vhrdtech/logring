//! Mount whatever bytes the fuzzer makes up as a ring image, log into it, scan it: nothing a medium holds may
//! panic the writer or the scanner. Run with the seed images (real rings, so the mutations reach the parsers):
//! `cd fuzz && cargo +nightly fuzz run mount corpus/mount seeds` (`cargo install cargo-fuzz`; new inputs go to
//! the gitignored `corpus/`, the seeds stay as they are). The seeds come from
//! `cargo test -p ww_logring --features std write_fuzz_seeds -- --ignored`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ww_logring::harness;

fuzz_target!(|data: &[u8]| {
    harness::mount_garbage(data);
});
