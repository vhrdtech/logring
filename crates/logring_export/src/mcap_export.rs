//! MCAP export: one channel per [Track], JSON messages ([value_json::value_to_json]) with a JSON Schema
//! ([value_json::jsonschema_of]) attached to the channel.

use crate::decode::Track;
use crate::value_json::{jsonschema_of, value_to_json};
use anyhow::{Result, anyhow};
use mcap::records::MessageHeader;
use std::collections::BTreeMap;
use std::io::{Seek, Write};

/// Writes every track in `tracks` as its own topic to `out`: tracks with no resolved type (`ty: None`) are
/// skipped (nothing to put in a schema or a message; [crate::decode::fallback_name] still names them in
/// `logring info`).
pub fn write_tracks(tracks: &[Track], out: impl Write + Seek) -> Result<()> {
    let mut writer = mcap::Writer::new(out).map_err(|e| anyhow!("mcap: {e}"))?;
    for track in tracks {
        let (Some(ty), Some(bundle)) = (track.ty, track.bundle) else {
            continue;
        };
        let schema_json = serde_json::to_vec(&jsonschema_of(ty, bundle))?;
        let schema_id = writer
            .add_schema(&track.name, "jsonschema", &schema_json)
            .map_err(|e| anyhow!("mcap: {e}"))?;
        let channel_id = writer
            .add_channel(schema_id, &track.name, "json", &BTreeMap::new())
            .map_err(|e| anyhow!("mcap: {e}"))?;
        for (seq, item) in track.items.iter().enumerate() {
            let json = value_to_json(&item.value);
            let data = serde_json::to_vec(&json)?;
            let log_time = item
                .head
                .t
                .as_ref()
                .and_then(logring::reader::time_ps)
                .map(|ps| ps as u64)
                .unwrap_or(item.head.pos);
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id,
                        sequence: seq as u32,
                        log_time,
                        publish_time: log_time,
                    },
                    &data,
                )
                .map_err(|e| anyhow!("mcap: {e}"))?;
        }
    }
    writer.finish().map_err(|e| anyhow!("mcap: {e}"))?;
    Ok(())
}
