//! `logring make-image`: a fresh ring image filled by a simulated device with `tracks` plain `f32` sensor
//! streams (`sensor_0`, `sensor_1`, ...), for demos and tests without a real device. A genuine (small) API
//! bundle is written with it, so the exported file decodes exactly like a real one.

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use logring::format::header::{Durability, TrackDesc};
use logring::tests_support::block_on;
use logring::writer::{Buffers, Config, Logring, SessionInfo};
use logring::{FullPolicy, UNib32};
use std::path::Path;
use wire_weaver::shrink_wrap::SerializeShrinkWrapOwned;
use wire_weaver::ww_version::{FullVersion, Version};
use ww_numeric::{NumericAnyTypeOwned, NumericBaseType, NumericValue};
use ww_self::{
    ApiBundleOwned, ApiItemKindOwned, ApiItemOwned, ApiLevelOwned, Multiplicity, TypeOwned,
    ValueOwned,
};
use ww_stream::store::log::SimSd;

const BUF: usize = 4096;
/// Upper bound of `--tracks` (the system track takes one more slot of `Buffers`' const generic).
const MAX_TRACKS: usize = 64;

fn bundle(n_tracks: u32) -> ApiBundleOwned {
    let items = (1..=n_tracks)
        .map(|i| ApiItemOwned {
            id: UNib32(i),
            kind: ApiItemKindOwned::Stream {
                ty: TypeOwned::NumericAny(NumericAnyTypeOwned::Base(NumericBaseType::F32)),
                is_up: true,
            },
            multiplicity: Multiplicity::Flat,
            since: None,
            ident: format!("sensor_{}", i - 1),
            docs: vec![],
            optional: None,
        })
        .collect();
    ApiBundleOwned {
        magic: ww_self::MAGIC,
        ww_self_version: ww_self::VERSION,
        root: ApiLevelOwned {
            docs: vec![],
            crate_idx: UNib32(0),
            trait_name: "DemoApi".to_string(),
            items,
        },
        types: vec![],
        traits: vec![],
        ext_crates: vec![],
        root_omitted: None,
    }
}

pub fn run(path: &Path, n_tracks: u32, records: u64) -> Result<()> {
    if n_tracks == 0 || n_tracks as usize >= MAX_TRACKS {
        bail!("--tracks must be 1..{MAX_TRACKS}");
    }
    let bundle = bundle(n_tracks);
    let bundle_bytes = bundle
        .to_ww_bytes_owned()
        .context("encoding the demo bundle")?;
    let ty = TypeOwned::NumericAny(NumericAnyTypeOwned::Base(NumericBaseType::F32));

    let paths: Vec<[UNib32; 1]> = (1..=n_tracks).map(|i| [UNib32(i)]).collect();
    let tracks: Vec<TrackDesc> = paths
        .iter()
        .map(|p| TrackDesc {
            signature: None,
            global_id: None,
            durability: Durability::Normal,
            rate: None,
            ..TrackDesc::new(p)
        })
        .collect();

    let info = SessionInfo {
        api: FullVersion::new("logring_make_image", Version::new(1, 0, 0)),
        device_id: "make-image",
        fw: "logring_make_image 1.0.0",
        hw: "simulated",
        tracks: &tracks,
        bundle: Some(&bundle_bytes),
    };

    // enough payload for every track's records, generously: ~16 bytes/record plus heads and tables
    let item_bytes: u64 = 16;
    let payload = n_tracks as u64 * records * item_bytes + 64 * 1024;
    let segment_size: u32 = 1 << 16;
    let data_segments = (payload / segment_size as u64 + 4).max(4);
    let image_size = segment_size as u64 * (1 + 2 + data_segments);
    let cfg = Config {
        segment_size,
        meta_segments: 2,
        flush_bytes: 4096,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 0x6c67,
        ..Config::default()
    };

    const TRACKS_CONST: usize = MAX_TRACKS;
    let medium = SimSd::from_image(vec![0u8; image_size as usize], 1);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS_CONST>::new());
    let mut lr = Logring::new(medium, &mut bufs, cfg, info).context("formatting the image")?;
    block_on(lr.mount()).context("mounting the fresh image")?;

    let bar = ProgressBar::new(n_tracks as u64 * records);
    bar.set_style(
        ProgressStyle::with_template("{bar:40} {pos}/{len} records")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    for rec in 0..records {
        for track in 1..=n_tracks {
            let value = (track as f32) + (rec as f32 * 0.1).sin();
            let bytes = ValueOwned::Numeric(NumericValue::F32(value))
                .ser_shrink_wrap_dyn(&ty, &bundle)
                .context("encoding a demo record")?;
            block_on(lr.push(track, &bytes, None)).context("pushing a demo record")?;
            bar.inc(1);
        }
    }
    bar.finish_and_clear();
    block_on(lr.sync()).context("syncing the image")?;

    let image = lr.into_medium().image();
    std::fs::write(path, &image).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "{} ({} tracks, {} records each, {} bytes)",
        path.display(),
        n_tracks,
        records,
        image.len()
    );
    Ok(())
}
