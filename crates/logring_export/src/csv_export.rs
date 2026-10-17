//! CSV export of one decoded [Track]: one row per record, columns `pos`, `t` (device time, blank when the
//! device had no clock yet) and the value's fields flattened by dotted name (`a.b`, `a.0` for a tuple or array
//! element, `a.Variant.b` through an enum field) — the same flattening for every record, so columns a later
//! (or sparser) record doesn't have come out blank rather than shifting the row.

use crate::decode::{Item, Track};
use crate::value_json::numeric_to_string;
use std::io::Write;
use ww_self::{FieldsValueOwned, ValueOwned};

/// Writes `track`'s records as CSV to `out`: a header row, then one row per record in file order.
pub fn write_track_csv(track: &Track, out: impl Write) -> csv::Result<()> {
    let mut columns: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut rows: Vec<Vec<(String, String)>> = Vec::with_capacity(track.items.len());
    for item in &track.items {
        let mut cells = Vec::new();
        flatten("value", &item.value, &mut cells);
        for (name, _) in &cells {
            if seen.insert(name.clone()) {
                columns.push(name.clone());
            }
        }
        rows.push(cells);
    }

    let mut wr = csv::Writer::from_writer(out);
    let mut header = vec!["pos".to_string(), "t_ps".to_string()];
    header.extend(columns.iter().cloned());
    wr.write_record(&header)?;
    for (item, cells) in track.items.iter().zip(rows.iter()) {
        let by_name: std::collections::HashMap<&str, &str> = cells
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let mut row = vec![item.head.pos.to_string(), format_time(item)];
        for name in &columns {
            row.push(
                by_name
                    .get(name.as_str())
                    .copied()
                    .unwrap_or("")
                    .to_string(),
            );
        }
        wr.write_record(&row)?;
    }
    wr.flush()?;
    Ok(())
}

fn format_time(item: &Item) -> String {
    item.head
        .t
        .as_ref()
        .and_then(logring::reader::time_ps)
        .map(|ps| ps.to_string())
        .unwrap_or_default()
}

/// Flattens `value` into `(dotted.name, cell text)` pairs under `prefix`.
fn flatten(prefix: &str, value: &ValueOwned, out: &mut Vec<(String, String)>) {
    match value {
        ValueOwned::Bool(b) => out.push((prefix.to_string(), b.to_string())),
        ValueOwned::Numeric(n) => out.push((prefix.to_string(), numeric_to_string(n))),
        ValueOwned::String(s) => out.push((prefix.to_string(), s.clone())),
        ValueOwned::Vec(items) | ValueOwned::Array(items) | ValueOwned::Tuple(items) => {
            for (i, v) in items.iter().enumerate() {
                flatten(&format!("{prefix}.{i}"), v, out);
            }
        }
        ValueOwned::Struct { fields } => flatten_fields(prefix, fields, out),
        ValueOwned::Enum { variant, fields } => {
            flatten_fields(&format!("{prefix}.{variant}"), fields, out)
        }
        ValueOwned::Option(opt) => match opt {
            Some(v) => flatten(prefix, v, out),
            None => out.push((prefix.to_string(), String::new())),
        },
        ValueOwned::Result(res) => match res {
            Ok(v) => flatten(&format!("{prefix}.Ok"), v, out),
            Err(v) => flatten(&format!("{prefix}.Err"), v, out),
        },
        ValueOwned::Range(r) => {
            out.push((format!("{prefix}.start"), numeric_to_string(&r.start)));
            out.push((format!("{prefix}.end"), numeric_to_string(&r.end)));
        }
        ValueOwned::RangeInclusive(r) => {
            out.push((format!("{prefix}.start"), numeric_to_string(r.start())));
            out.push((format!("{prefix}.end"), numeric_to_string(r.end())));
        }
    }
}

fn flatten_fields(prefix: &str, fields: &FieldsValueOwned, out: &mut Vec<(String, String)>) {
    match fields {
        FieldsValueOwned::Named(named) => {
            for (name, v) in named {
                flatten(&format!("{prefix}.{name}"), v, out);
            }
        }
        FieldsValueOwned::Unnamed(values) => {
            for (i, v) in values.iter().enumerate() {
                flatten(&format!("{prefix}.{i}"), v, out);
            }
        }
        FieldsValueOwned::Unit => {}
    }
}
