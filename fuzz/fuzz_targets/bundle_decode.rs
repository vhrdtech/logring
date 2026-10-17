//! The PC tool's dynamic decode path on an untrusted bundle (a card's own copy, or one from
//! `~/.wire_weaver/definitions`): parse ([logring_export::schema::parse]), resolve a track's type by a few
//! short paths ([logring_export::schema::resolve_track_type]), then decode the same bytes against it
//! ([ww_self::ValueOwned::des_shrink_wrap_dyn]). Nothing here may panic on malformed bytes. Run with
//! `just fuzz-bundle` (`cargo install cargo-fuzz`; new inputs go to the gitignored `corpus/`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use ww_self::ValueOwned;

fuzz_target!(|data: &[u8]| {
    let Ok(bundle) = logring_export::schema::parse(data) else {
        return;
    };
    // a handful of short paths (ids from the input itself, so a bundle built to describe them is reachable)
    for len in 0..4 {
        let path: Vec<u32> = data.iter().take(len).map(|b| u32::from(*b)).collect();
        if let Ok(resolved) = logring_export::schema::resolve_track_type(&bundle.api, &path) {
            let _ = ValueOwned::des_shrink_wrap_dyn(data, resolved.ty, &bundle.api);
        }
    }
});
