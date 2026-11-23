//! Resolving a track's item type from the API bundle stored with its session (dynamic decode, no generated
//! code): the bundle names the device API's resources by id, a track's `path` ([TrackInfo::path]) is the chain
//! of resource ids from the API root to its stream (`docs/api/addressing.md` in `wire_weaver`), with one extra
//! id for each array resource on the way (the selected index).
//!
//! Traits and types a device left out of its own bundle because they come from a published crate (`Skipped*`
//! locations) are filled back in through `ww_snapshots`' resolver (the snapshot sets the program embedded, the
//! `~/.wire_weaver` cache and definition cache, Cargo's sources on this machine) before a track's type is resolved.

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
/// traits and types it left out through `ww_snapshots`, never panicking on malformed bytes.
pub fn parse(bytes: &[u8]) -> Result<Bundle> {
    let mut api = ApiBundleOwned::from_ww_bytes_owned(bytes)
        .map_err(|e| anyhow!("decoding the API bundle: {e:?}"))?;
    let inlined = ww_snapshots::Resolver::global().inline_skipped(&mut api);
    Ok(Bundle {
        api,
        unresolved: inlined.not_inlined.len(),
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
            ApiItemKindOwned::Keys { event_ty, .. } => {
                // the key companion's fixed level (`ww_self::keys`): only its `keys` stream is one
                let id = ids
                    .next()
                    .ok_or_else(|| anyhow!("path ran out at a key companion"))?;
                if id != ww_self::keys::KEYS {
                    bail!(
                        "path leads to a key companion's method or property, not its keys stream"
                    );
                }
                if ids.next().is_some() {
                    bail!("path continues past a stream resource");
                }
                names.push(
                    ww_self::keys::item_name(id)
                        .expect("KEYS has a name")
                        .to_owned(),
                );
                return Ok(Resolved {
                    ty: event_ty,
                    name: names.join("."),
                });
            }
            ApiItemKindOwned::Mount { .. } => {
                // a mount slot's level has only `presence` (a property); what is mounted is known at run time
                bail!("path leads to a mount slot (SRV-76), whose API is not in the bundle")
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

    /// A track on a key companion's `keys` stream (`keyed!(value by u32)`) has its key events as items; the
    /// companion's method and property are no stream.
    #[test]
    fn resolves_a_key_companions_keys_stream() {
        let mut bundle = one_stream_bundle();
        let mut keys = bundle.root.items[0].clone();
        keys.id = wire_weaver::shrink_wrap::UNib32(2);
        keys.ident = "value_keys".to_owned();
        keys.kind = ApiItemKindOwned::Keys {
            of: wire_weaver::shrink_wrap::UNib32(1),
            key: TypeOwned::Bool,
            event_ty: TypeOwned::Bool,
        };
        bundle.root.items.push(keys);
        let resolved = resolve_track_type(&bundle, &[2, ww_self::keys::KEYS]).unwrap();
        assert!(matches!(resolved.ty, TypeOwned::Bool));
        assert_eq!(resolved.name, "value_keys.keys");
        assert!(resolve_track_type(&bundle, &[2, ww_self::keys::INDEX_OF]).is_err());
        assert!(resolve_track_type(&bundle, &[2]).is_err());
    }

    /// A mount slot's API is known only at run time: a track path through it resolves to nothing.
    #[test]
    fn rejects_a_path_to_a_mount_slot() {
        let mut bundle = one_stream_bundle();
        let mut mount = bundle.root.items[0].clone();
        mount.id = wire_weaver::shrink_wrap::UNib32(2);
        mount.ident = "hub".to_owned();
        mount.kind = ApiItemKindOwned::Mount {
            expects: None,
            presence_ty: TypeOwned::Bool,
        };
        bundle.root.items.push(mount);
        let Err(err) = resolve_track_type(&bundle, &[2, ww_self::mount::API]) else {
            panic!("a mount slot resolved to a type");
        };
        assert!(err.to_string().contains("mount slot"), "{err}");
    }

    #[test]
    fn rejects_a_path_to_nothing() {
        let bundle = one_stream_bundle();
        assert!(resolve_track_type(&bundle, &[99]).is_err());
    }
}
