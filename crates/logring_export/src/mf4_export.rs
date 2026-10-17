//! MF4 (ASAM MDF4) export of one decoded [Track]: standard ASAM `CAN_DataFrame` bus logging
//! ([mdf4_rs::can::RawCanLogger]) for a track whose items look like CAN frames (a struct with an id and a data
//! byte array), plain `f64` channels for everything else.
//!
//! `mdf4-rs` was picked over `mf4-rs` for this (P2628#16oct-1001h): pure Rust (`#![forbid(unsafe_code)]`),
//! MIT/Apache-2.0 like this repo, actively published, minimal dependencies, and it already implements the
//! ASAM MDF4 bus-logging convention for raw CAN frames (`RawCanLogger`) instead of leaving it to be hand-rolled
//! here. DBC decoding is a separate, optional feature of the crate that this exporter doesn't use: `AGENTS.md`
//! keeps DBC out of what logring stores, applying it only in whatever tool opens the MF4 alongside a DBC file
//! (asammdf, CANape, ...).

use crate::decode::{Item, Track};
use anyhow::{Result, anyhow, bail};
use mdf4_rs::can::RawCanLogger;
use mdf4_rs::{DataType, DecodedValue, MdfWriter};
use std::collections::BTreeSet;
use std::path::Path;
use ww_self::{FieldsValueOwned, ValueOwned};

/// Writes `track` to `path`: CAN bus logging when its items look like CAN frames, plain numeric channels
/// otherwise. `Err` when the track has nothing decoded (an unresolved type, or no records) or no numeric
/// field to put in a channel.
pub fn export_track(track: &Track, path: impl AsRef<Path>) -> Result<()> {
    if track.items.is_empty() {
        bail!("{}: nothing decoded to export", track.name);
    }
    match can_frames(track) {
        Some(frames) => write_can(&track.name, &frames, path),
        None => write_numeric(track, path),
    }
}

/// A CAN frame recognized in a decoded item: an id field (`id`/`can_id`/`arbitration_id`) and a byte payload
/// (`data`/`payload`/`bytes`) of at most 64 bytes (CAN FD's limit).
struct CanFrame {
    id: u32,
    extended: bool,
    data: Vec<u8>,
}

const ID_NAMES: &[&str] = &["id", "can_id", "arbitration_id"];
const DATA_NAMES: &[&str] = &["data", "payload", "bytes"];
const EXTENDED_NAMES: &[&str] = &["extended", "is_extended", "ide"];

fn named_field<'v>(fields: &'v FieldsValueOwned, names: &[&str]) -> Option<&'v ValueOwned> {
    match fields {
        FieldsValueOwned::Named(named) => named
            .iter()
            .find(|(n, _)| names.contains(&n.as_str()))
            .map(|(_, v)| v),
        _ => None,
    }
}

fn as_u32(v: &ValueOwned) -> Option<u32> {
    match v {
        ValueOwned::Numeric(n) => u32::try_from(n.to_i128()?).ok(),
        _ => None,
    }
}

fn as_bool(v: &ValueOwned) -> Option<bool> {
    match v {
        ValueOwned::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_bytes(v: &ValueOwned) -> Option<Vec<u8>> {
    match v {
        ValueOwned::Vec(items) | ValueOwned::Array(items) => items
            .iter()
            .map(|it| match it {
                ValueOwned::Numeric(n) => u8::try_from(n.to_i128()?).ok(),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn frame_of(value: &ValueOwned) -> Option<CanFrame> {
    let ValueOwned::Struct { fields } = value else {
        return None;
    };
    let id = as_u32(named_field(fields, ID_NAMES)?)?;
    let data = as_bytes(named_field(fields, DATA_NAMES)?)?;
    if data.len() > 64 {
        return None;
    }
    let extended = named_field(fields, EXTENDED_NAMES)
        .and_then(as_bool)
        .unwrap_or(id > 0x7FF);
    Some(CanFrame { id, extended, data })
}

/// A device time in microseconds for `item`, falling back to its stream position (monotonic, just not real
/// time) when the device had no clock set yet.
fn timestamp_us(item: &Item) -> u64 {
    item.head
        .t
        .as_ref()
        .and_then(logring::reader::time_ps)
        .map(|ps| (ps / 1_000_000) as u64)
        .unwrap_or(item.head.pos)
}

/// `Some` when every item of `track` looks like a CAN frame: a track comes from one `#[ww_trait]` stream, so
/// it is either a CAN track throughout or not at all.
fn can_frames(track: &Track) -> Option<Vec<(u64, CanFrame)>> {
    let mut out = Vec::with_capacity(track.items.len());
    for item in &track.items {
        out.push((timestamp_us(item), frame_of(&item.value)?));
    }
    (!out.is_empty()).then_some(out)
}

fn write_can(name: &str, frames: &[(u64, CanFrame)], path: impl AsRef<Path>) -> Result<()> {
    let path = path
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 path"))?;
    let mut logger =
        RawCanLogger::new_file_with_source_name(path, name).map_err(|e| anyhow!("mdf4: {e}"))?;
    for (t_us, frame) in frames {
        if frame.extended {
            logger.log_extended(frame.id, *t_us, &frame.data);
        } else {
            logger.log(frame.id, *t_us, &frame.data);
        }
    }
    logger.finalize_file().map_err(|e| anyhow!("mdf4: {e}"))
}

/// Flattens `value`'s numeric and boolean leaves into `(dotted.name, f64)` pairs (strings and byte blobs that
/// aren't [frame_of]'s payload have no numeric representation and are dropped).
fn flatten_numeric(prefix: &str, value: &ValueOwned, out: &mut Vec<(String, f64)>) {
    match value {
        ValueOwned::Bool(b) => out.push((prefix.to_string(), if *b { 1.0 } else { 0.0 })),
        ValueOwned::Numeric(n) => out.push((prefix.to_string(), n.to_f64())),
        ValueOwned::String(_) => {}
        ValueOwned::Vec(items) | ValueOwned::Array(items) | ValueOwned::Tuple(items) => {
            for (i, v) in items.iter().enumerate() {
                flatten_numeric(&format!("{prefix}.{i}"), v, out);
            }
        }
        ValueOwned::Struct { fields } => flatten_numeric_fields(prefix, fields, out),
        ValueOwned::Enum { variant, fields } => {
            flatten_numeric_fields(&format!("{prefix}.{variant}"), fields, out)
        }
        ValueOwned::Option(opt) => {
            if let Some(v) = opt {
                flatten_numeric(prefix, v, out);
            }
        }
        ValueOwned::Result(res) => match res {
            Ok(v) => flatten_numeric(&format!("{prefix}.Ok"), v, out),
            Err(v) => flatten_numeric(&format!("{prefix}.Err"), v, out),
        },
        ValueOwned::Range(r) => {
            out.push((format!("{prefix}.start"), r.start.to_f64()));
            out.push((format!("{prefix}.end"), r.end.to_f64()));
        }
        ValueOwned::RangeInclusive(r) => {
            out.push((format!("{prefix}.start"), r.start().to_f64()));
            out.push((format!("{prefix}.end"), r.end().to_f64()));
        }
    }
}

fn flatten_numeric_fields(prefix: &str, fields: &FieldsValueOwned, out: &mut Vec<(String, f64)>) {
    match fields {
        FieldsValueOwned::Named(named) => {
            for (name, v) in named {
                flatten_numeric(&format!("{prefix}.{name}"), v, out);
            }
        }
        FieldsValueOwned::Unnamed(values) => {
            for (i, v) in values.iter().enumerate() {
                flatten_numeric(&format!("{prefix}.{i}"), v, out);
            }
        }
        FieldsValueOwned::Unit => {}
    }
}

fn write_numeric(track: &Track, path: impl AsRef<Path>) -> Result<()> {
    let mut columns: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut rows: Vec<Vec<(String, f64)>> = Vec::with_capacity(track.items.len());
    for item in &track.items {
        let mut cells = Vec::new();
        flatten_numeric("value", &item.value, &mut cells);
        for (name, _) in &cells {
            if seen.insert(name.clone()) {
                columns.push(name.clone());
            }
        }
        rows.push(cells);
    }
    if columns.is_empty() {
        bail!("{}: no numeric field to export to MF4", track.name);
    }

    let path = path
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 path"))?;
    let mut writer = MdfWriter::new(path).map_err(|e| anyhow!("mdf4: {e}"))?;
    writer.init_mdf_file().map_err(|e| anyhow!("mdf4: {e}"))?;
    let cg = writer
        .add_channel_group(None, |_| {})
        .map_err(|e| anyhow!("mdf4: {e}"))?;
    let time_ch = writer
        .add_channel(&cg, None, |ch| {
            ch.data_type = DataType::FloatLE;
            ch.name = Some("t_s".to_string());
            ch.bit_count = 64;
        })
        .map_err(|e| anyhow!("mdf4: {e}"))?;
    writer
        .set_time_channel(&time_ch)
        .map_err(|e| anyhow!("mdf4: {e}"))?;
    let mut prev = time_ch;
    for name in &columns {
        prev = writer
            .add_channel(&cg, Some(&prev), |ch| {
                ch.data_type = DataType::FloatLE;
                ch.name = Some(name.clone());
                ch.bit_count = 64;
            })
            .map_err(|e| anyhow!("mdf4: {e}"))?;
    }

    writer
        .start_data_block_for_cg(&cg, 0)
        .map_err(|e| anyhow!("mdf4: {e}"))?;
    for (item, cells) in track.items.iter().zip(rows.iter()) {
        let by_name: std::collections::HashMap<&str, f64> =
            cells.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        let t_s = item
            .head
            .t
            .as_ref()
            .and_then(logring::reader::time_ps)
            .map(|ps| ps as f64 / 1e12)
            .unwrap_or(item.head.pos as f64);
        let mut values = vec![DecodedValue::Float(t_s)];
        for name in &columns {
            values.push(DecodedValue::Float(
                by_name.get(name.as_str()).copied().unwrap_or(f64::NAN),
            ));
        }
        writer
            .write_record(&cg, &values)
            .map_err(|e| anyhow!("mdf4: {e}"))?;
    }
    writer
        .finish_data_block(&cg)
        .map_err(|e| anyhow!("mdf4: {e}"))?;
    writer.finalize().map_err(|e| anyhow!("mdf4: {e}"))?;
    Ok(())
}
