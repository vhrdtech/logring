//! Turning a resolved [TypeOwned] into a JSON Schema fragment, and a decoded [ValueOwned] into the matching
//! [serde_json::Value] — the common ground CSV and MCAP export both flatten or embed.

use serde_json::{Map, Value as Json, json};
use ww_numeric::{NumericAnyTypeOwned, NumericBaseType, NumericValue};
use ww_self::{ApiBundleOwned, FieldsOwned, FieldsValueOwned, TypeOwned, ValueOwned};

/// A JSON number for any [NumericValue], exact for integers that fit an `i64`/`u64`, `f64` otherwise.
pub fn numeric_to_json(v: &NumericValue) -> Json {
    if let Some(i) = v.to_i128() {
        if let Ok(i) = i64::try_from(i) {
            return json!(i);
        }
        if let Ok(u) = u64::try_from(i) {
            return json!(u);
        }
    }
    json!(v.to_f64())
}

/// A plain string for any [NumericValue] (CSV cells, column names).
pub fn numeric_to_string(v: &NumericValue) -> String {
    match v.to_i128() {
        Some(i) => i.to_string(),
        None => v.to_f64().to_string(),
    }
}

/// Flattens `value`'s numeric and boolean leaves into `(dotted.name, f64)` pairs: strings and byte blobs have
/// no numeric representation and are dropped. Shared by MF4's plain-channel export and `logring_py`'s
/// `to_numpy()`/`to_pandas()`.
pub fn flatten_numeric(prefix: &str, value: &ValueOwned, out: &mut Vec<(String, f64)>) {
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

/// The decoded value as JSON: structs and named-field enum variants become objects, tuples and unnamed fields
/// become arrays, matching the shape [jsonschema_of] describes for the same type.
pub fn value_to_json(value: &ValueOwned) -> Json {
    match value {
        ValueOwned::Bool(b) => Json::Bool(*b),
        ValueOwned::Numeric(n) => numeric_to_json(n),
        ValueOwned::String(s) => Json::String(s.clone()),
        ValueOwned::Vec(items) | ValueOwned::Array(items) | ValueOwned::Tuple(items) => {
            Json::Array(items.iter().map(value_to_json).collect())
        }
        ValueOwned::Struct { fields } => fields_value_to_json(fields),
        ValueOwned::Enum { variant, fields } => {
            let mut obj = Map::new();
            obj.insert(variant.clone(), fields_value_to_json(fields));
            Json::Object(obj)
        }
        ValueOwned::Option(opt) => opt.as_ref().map_or(Json::Null, |v| value_to_json(v)),
        ValueOwned::Result(res) => match res {
            Ok(v) => json!({ "Ok": value_to_json(v) }),
            Err(e) => json!({ "Err": value_to_json(e) }),
        },
        ValueOwned::Range(r) => {
            json!({ "start": numeric_to_json(&r.start), "end": numeric_to_json(&r.end) })
        }
        ValueOwned::RangeInclusive(r) => {
            json!({ "start": numeric_to_json(r.start()), "end": numeric_to_json(r.end()) })
        }
    }
}

fn fields_value_to_json(fields: &FieldsValueOwned) -> Json {
    match fields {
        FieldsValueOwned::Named(named) => {
            let mut obj = Map::new();
            for (name, v) in named {
                obj.insert(name.clone(), value_to_json(v));
            }
            Json::Object(obj)
        }
        FieldsValueOwned::Unnamed(values) => {
            Json::Array(values.iter().map(value_to_json).collect())
        }
        FieldsValueOwned::Unit => Json::Null,
    }
}

/// A JSON Schema (draft 2020-12) fragment describing `ty`, resolving [TypeOwned::OutOfLine] and
/// [TypeOwned::Instance] against `bundle`'s `types` array; opaque (`true`, "anything") where the bundle doesn't
/// have a type's definition (a `Skipped*` location `wire_weaver_snapshots` didn't fill in either).
pub fn jsonschema_of(ty: &TypeOwned, bundle: &ApiBundleOwned) -> Json {
    use ww_self::TypeLocationOwned;
    match ty {
        TypeOwned::Bool => json!({ "type": "boolean" }),
        TypeOwned::NumericAny(n) => {
            if base_is_float(base_of(n)) {
                json!({ "type": "number" })
            } else {
                json!({ "type": "integer" })
            }
        }
        TypeOwned::String => json!({ "type": "string" }),
        TypeOwned::Vec(inner) | TypeOwned::Series(inner) => {
            json!({ "type": "array", "items": jsonschema_of(inner, bundle) })
        }
        TypeOwned::Array { ty: inner, .. } => {
            json!({ "type": "array", "items": jsonschema_of(inner, bundle) })
        }
        TypeOwned::Tuple(items) => {
            json!({ "type": "array", "prefixItems": items.iter().map(|t| jsonschema_of(t, bundle)).collect::<Vec<_>>() })
        }
        TypeOwned::Struct(s) => fields_schema(&s.fields, bundle),
        TypeOwned::Enum(e) => {
            let variants: Vec<Json> = e
                .variants
                .iter()
                .map(|v| json!({ "type": "object", "properties": { v.ident.clone(): fields_schema(&v.fields, bundle) }, "required": [v.ident.clone()] }))
                .collect();
            json!({ "oneOf": variants })
        }
        TypeOwned::Option { some_ty } => {
            let mut schema = jsonschema_of(some_ty, bundle);
            if let Json::Object(obj) = &mut schema {
                obj.insert("nullable".into(), json!(true));
            }
            schema
        }
        TypeOwned::Result { ok_ty, err_ty } => json!({
            "oneOf": [
                { "type": "object", "properties": { "Ok": jsonschema_of(ok_ty, bundle) }, "required": ["Ok"] },
                { "type": "object", "properties": { "Err": jsonschema_of(err_ty, bundle) }, "required": ["Err"] },
            ]
        }),
        TypeOwned::Box(inner) => jsonschema_of(inner, bundle),
        TypeOwned::Range(base) | TypeOwned::RangeInclusive(base) => {
            let n = numeric_base_schema(base);
            json!({ "type": "object", "properties": { "start": n.clone(), "end": n } })
        }
        TypeOwned::MaxLen { ty: inner, .. } => jsonschema_of(inner, bundle),
        TypeOwned::Delta(base) | TypeOwned::DeltaOfDelta(base) | TypeOwned::XorFloat(base) => {
            json!({ "type": "array", "items": numeric_base_schema(base) })
        }
        TypeOwned::OutOfLine { type_idx } => match bundle.types.get(type_idx.0 as usize) {
            Some(TypeLocationOwned::InLine { ty, .. }) => jsonschema_of(ty, bundle),
            _ => json!(true),
        },
        TypeOwned::Instance { type_idx, .. } => match bundle.types.get(type_idx.0 as usize) {
            Some(TypeLocationOwned::InLine { ty, .. }) => jsonschema_of(ty, bundle),
            _ => json!(true),
        },
        TypeOwned::Flag | TypeOwned::Param(_) => json!(true),
    }
}

fn numeric_base_schema(base: &NumericBaseType) -> Json {
    if base_is_float(base) {
        json!({ "type": "number" })
    } else {
        json!({ "type": "integer" })
    }
}

/// The base numeric type a [NumericAnyTypeOwned] builds on (plain, scaled or shifted).
fn base_of(n: &NumericAnyTypeOwned) -> &NumericBaseType {
    match n {
        NumericAnyTypeOwned::Base(b) => b,
        NumericAnyTypeOwned::SubType { base, .. } => base,
        NumericAnyTypeOwned::ShiftScale { base, .. } => base,
    }
}

fn base_is_float(base: &NumericBaseType) -> bool {
    matches!(
        base,
        NumericBaseType::F16 | NumericBaseType::F32 | NumericBaseType::F64
    )
}

fn fields_schema(fields: &FieldsOwned, bundle: &ApiBundleOwned) -> Json {
    match fields {
        FieldsOwned::Named(named) => {
            let mut props = Map::new();
            let mut required = Vec::new();
            for f in named {
                if let Some(ident) = &f.ident {
                    props.insert(ident.clone(), jsonschema_of(&f.ty, bundle));
                    required.push(json!(ident));
                }
            }
            json!({ "type": "object", "properties": props, "required": required })
        }
        FieldsOwned::Unnamed(unnamed) => {
            json!({ "type": "array", "prefixItems": unnamed.iter().map(|f| jsonschema_of(&f.ty, bundle)).collect::<Vec<_>>() })
        }
        FieldsOwned::Unit => json!(true),
    }
}
