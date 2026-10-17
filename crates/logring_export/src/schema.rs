//! Resolving a track's item type from the API bundle stored with its session (dynamic decode, no generated
//! code): the bundle names the device API's resources by id, a track's `path` ([TrackInfo::path]) is the chain
//! of resource ids from the API root to its stream (`docs/api/addressing.md` in `wire_weaver`), with one extra
//! id for each array resource on the way (the selected index).
//!
//! Traits and types a device left out of its own bundle because they come from a published crate (`Skipped*`
//! locations) are filled back in from `wire_weaver_snapshots`' embedded snapshots of `ww_global`/`ww_stdlib`
//! crates, and from the local `~/.wire_weaver/definitions` cache populated by `wire_weaver` codegen on this
//! machine, before a track's type is resolved.

use anyhow::{Result, anyhow, bail};
use wire_weaver::shrink_wrap::DeserializeShrinkWrapOwned;
use ww_self::{
    ApiBundleOwned, ApiItemKindOwned, ApiLevelLocationOwned, ApiLevelOwned, Multiplicity, TypeOwned,
};

/// A parsed API bundle, with the stdlib traits and types it left out filled back in where known.
pub struct Bundle {
    pub api: ApiBundleOwned,
    /// Traits or types left out of the bundle that neither the embedded snapshots nor the local definition
    /// cache could fill in: tracks through them fall back to raw bytes on export.
    pub unresolved: usize,
}

/// Parses an API bundle's raw bytes (the tail of a [BundleRecord](crate::meta::BundleRecord)) and fills in the
/// traits and types it left out from `wire_weaver_snapshots`, never panicking on malformed bytes.
pub fn parse(bytes: &[u8]) -> Result<Bundle> {
    let mut api = ApiBundleOwned::from_ww_bytes_owned(bytes)
        .map_err(|e| anyhow!("decoding the API bundle: {e:?}"))?;
    let cache_dir = ww_self::definition_cache::default_dir();
    let not_inlined =
        wire_weaver_snapshots::inline_skipped_or_cached(&mut api, cache_dir.as_deref());
    Ok(Bundle {
        api,
        unresolved: not_inlined.not_inlined.len(),
    })
}

/// A track's item type, resolved from the bundle, and the dotted name of the resources on the way there
/// (`can.0.rx`, following `docs/api/addressing.md`'s example), for display and as an MCAP topic / CSV file
/// name.
pub struct Resolved<'b> {
    pub ty: &'b TypeOwned,
    pub name: String,
}

/// The item type of the stream at `path` from the bundle's API root, `Err` when the path doesn't lead to a
/// stream, sink or stream function (a stale bundle, a track from a device API this bundle doesn't describe, or
/// a trait/type the bundle still leaves out).
pub fn resolve_track_type<'b>(bundle: &'b ApiBundleOwned, path: &[u32]) -> Result<Resolved<'b>> {
    let mut level: &ApiLevelOwned = &bundle.root;
    let mut ids = path.iter().copied();
    let mut names = Vec::new();
    loop {
        let id = ids
            .next()
            .ok_or_else(|| anyhow!("path ran out before reaching a stream"))?;
        let item = level
            .items
            .iter()
            .find(|it| it.id.0 == id)
            .ok_or_else(|| anyhow!("resource id {id} not in this level of the bundle"))?;
        if matches!(item.multiplicity, Multiplicity::Array { .. }) {
            // the array's own id was just consumed; the selected index follows it in the path and names nothing
            // in the bundle (every element has the same type)
            let index = ids
                .next()
                .ok_or_else(|| anyhow!("path ran out at an array resource's index"))?;
            names.push(format!("{}.{index}", item.ident));
        } else {
            names.push(item.ident.clone());
        }
        match &item.kind {
            ApiItemKindOwned::Trait { trait_idx, .. } => {
                level = match bundle.traits.get(trait_idx.0 as usize) {
                    Some(ApiLevelLocationOwned::InLine { level, .. }) => level,
                    Some(_) => bail!(
                        "trait {} is not in the bundle (left out, unresolved)",
                        trait_idx.0
                    ),
                    None => bail!("trait index {} out of range", trait_idx.0),
                };
            }
            ApiItemKindOwned::Stream { ty, .. }
            | ApiItemKindOwned::FlowSink { ty, .. }
            | ApiItemKindOwned::StreamFn { ty, .. } => {
                if ids.next().is_some() {
                    bail!("path continues past a stream resource");
                }
                return Ok(Resolved {
                    ty,
                    name: names.join("."),
                });
            }
            ApiItemKindOwned::Method { .. } | ApiItemKindOwned::Property { .. } => {
                bail!("path leads to a method or property, not a stream")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::one_stream_bundle;

    #[test]
    fn resolves_a_flat_stream() {
        let bundle = one_stream_bundle();
        let resolved = resolve_track_type(&bundle, &[1]).unwrap();
        assert!(matches!(resolved.ty, TypeOwned::NumericAny(_)));
        assert_eq!(resolved.name, "value");
    }

    #[test]
    fn rejects_a_path_to_nothing() {
        let bundle = one_stream_bundle();
        assert!(resolve_track_type(&bundle, &[99]).is_err());
    }
}
