//! End to end: a real ring image written by [logring::writer::Logring] with a genuine API bundle, opened and
//! exported by `logring_export`. CSV round-trips the value; the MCAP bytes parse back with `mcap`; the MF4
//! bytes load back with `mdf4-rs` (an `asammdf` check belongs to the CLI's own end-to-end test, R10's "done
//! when").

use logring::format::header::{Durability, TrackDesc};
use logring::tests_support::block_on;
use logring::writer::{Buffers, Config, Logring, SessionInfo};
use logring::{FullPolicy, UNib32};
use std::io::Cursor;
use wire_weaver::shrink_wrap::SerializeShrinkWrapOwned;
use wire_weaver::ww_version::{FullVersion, Version};
use ww_numeric::NumericAnyTypeOwned;
use ww_numeric::NumericBaseType;
use ww_numeric::NumericValue;
use ww_self::{
    ApiBundleOwned, ApiItemKindOwned, ApiItemOwned, ApiLevelOwned, Multiplicity, TypeOwned,
    ValueOwned,
};
use ww_stream::store::log::SimSd;

const BUF: usize = 1024;
const TRACKS: usize = 4;
const IMAGE: usize = 4096 * 8;

fn one_u32_stream_bundle() -> ApiBundleOwned {
    ApiBundleOwned {
        magic: ww_self::MAGIC,
        ww_self_version: ww_self::VERSION,
        root: ApiLevelOwned {
            docs: vec![],
            crate_idx: UNib32(0),
            trait_name: "Root".to_string(),
            items: vec![ApiItemOwned {
                id: UNib32(1),
                kind: ApiItemKindOwned::Stream {
                    ty: TypeOwned::NumericAny(NumericAnyTypeOwned::Base(NumericBaseType::U32)),
                    is_up: true,
                },
                multiplicity: Multiplicity::Flat,
                since: None,
                ident: "value".to_string(),
                docs: vec![],
                optional: None,
            }],
        },
        types: vec![],
        traits: vec![],
        ext_crates: vec![],
        root_omitted: None,
    }
}

/// Writes a tiny real ring to a temp file: one track (`value: u32`, path `[1]`) with a genuine bundle, a
/// handful of records, synced.
fn write_test_ring() -> tempfile::NamedTempFile {
    let bundle = one_u32_stream_bundle();
    let bundle_bytes = bundle.to_ww_bytes_owned().unwrap();
    let ty = TypeOwned::NumericAny(NumericAnyTypeOwned::Base(NumericBaseType::U32));

    let path = [UNib32(1)];
    let tracks = [TrackDesc {
        signature: None,
        global_id: None,
        durability: Durability::Normal,
        rate: None,
        ..TrackDesc::new(&path)
    }];
    let info = SessionInfo {
        api: FullVersion::new("test_fw", Version::new(1, 0, 0)),
        device_id: "test",
        fw: "test_fw 1.0.0",
        hw: "none",
        tracks: &tracks,
        bundle: Some(&bundle_bytes),
    };
    let cfg = Config {
        segment_size: 4096,
        meta_segments: 2,
        flush_bytes: 1024,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 1,
        ..Config::default()
    };
    let medium = SimSd::from_image(vec![0u8; IMAGE], 1);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS>::new());
    let mut lr = Logring::new(medium, &mut bufs, cfg, info).unwrap();
    block_on(lr.mount()).unwrap();
    for v in [10u32, 20, 30] {
        let value = ValueOwned::Numeric(NumericValue::U32(v));
        let bytes = value.ser_shrink_wrap_dyn(&ty, &bundle).unwrap();
        block_on(lr.push(1, &bytes, None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let image = lr.into_medium().image();

    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &image).unwrap();
    file
}

#[test]
fn decodes_and_exports_a_real_ring() {
    let file = write_test_ring();
    let log = logring_export::open(file.path()).unwrap();
    assert_eq!(log.sessions.len(), 1);
    assert_eq!(log.bundles.len(), 1);

    let bundles = logring_export::parse_bundles(&log);
    assert_eq!(bundles.len(), 1);
    let tracks = logring_export::decode::tracks(&log, &bundles);
    let track = tracks
        .iter()
        .find(|t| t.name == "value")
        .expect("the value track");
    assert_eq!(track.decode_errors, 0);
    let values: Vec<u32> = track
        .items
        .iter()
        .map(|it| match &it.value {
            ValueOwned::Numeric(NumericValue::U32(v)) => *v,
            other => panic!("unexpected value {other:?}"),
        })
        .collect();
    assert_eq!(values, vec![10, 20, 30]);

    let mut csv_bytes = Vec::new();
    logring_export::csv_export::write_track_csv(track, &mut csv_bytes).unwrap();
    let csv_text = String::from_utf8(csv_bytes).unwrap();
    assert!(csv_text.contains("value"));
    assert!(csv_text.contains("10"));
    assert!(csv_text.contains("30"));

    let mut mcap_bytes = Cursor::new(Vec::new());
    logring_export::mcap_export::write_tracks(&tracks, &mut mcap_bytes).unwrap();
    let mcap_bytes = mcap_bytes.into_inner();
    let messages: Vec<_> = mcap::MessageStream::new(&mcap_bytes)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(messages.len(), 3);

    let mf4 = tempfile::NamedTempFile::new().unwrap();
    logring_export::mf4_export::export_track(track, mf4.path()).unwrap();
    let mdf = mdf4_rs::MDF::from_file(mf4.path().to_str().unwrap()).unwrap();
    assert!(!mdf.channel_groups().is_empty());
}
