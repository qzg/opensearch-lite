use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::storage::{Database, StoreError, StoreResult};

#[derive(Debug, Clone)]
pub struct FieldCapsRequest {
    pub indices: Vec<String>,
    pub fields: Vec<String>,
    pub ignore_unavailable: bool,
    pub allow_no_indices: bool,
}

#[derive(Debug, Clone)]
struct FieldTypeCaps {
    indices: BTreeSet<String>,
    searchable: bool,
    aggregatable: bool,
}

pub fn field_caps_response(db: &Database, request: FieldCapsRequest) -> StoreResult<Value> {
    if request.fields.is_empty() {
        return Err(StoreError::new(
            400,
            "illegal_argument_exception",
            "field_caps requires at least one field pattern",
        ));
    }

    let names = resolve_indices(db, &request)?;
    if names.is_empty() {
        return if request.allow_no_indices || request.ignore_unavailable {
            Ok(json!({ "indices": [], "fields": {} }))
        } else {
            Err(StoreError::new(
                404,
                "index_not_found_exception",
                "no such index",
            ))
        };
    }

    let mut fields = BTreeMap::<String, BTreeMap<String, FieldTypeCaps>>::new();
    for name in &names {
        let Some(index) = db.indexes.get(name) else {
            continue;
        };
        let mapped = mapped_fields(&index.mappings);
        let mapped_names = mapped
            .iter()
            .map(|(field, _)| field.clone())
            .collect::<BTreeSet<_>>();
        for (field, ty) in mapped {
            insert_field_caps(&mut fields, &field, &ty, name);
        }
        for document in index.documents.values() {
            for (field, ty) in observed_fields(&document.source) {
                if mapped_names.contains(&field) {
                    continue;
                }
                insert_field_caps(&mut fields, &field, &ty, name);
            }
        }
    }

    let mut output_fields = serde_json::Map::new();
    for (field, types) in fields {
        if !field_matches_any(&field, &request.fields) {
            continue;
        }
        let mut type_map = serde_json::Map::new();
        for (ty, caps) in types {
            type_map.insert(
                ty.clone(),
                json!({
                    "type": ty,
                    "metadata_field": false,
                    "searchable": caps.searchable,
                    "aggregatable": caps.aggregatable,
                    "indices": caps.indices.into_iter().collect::<Vec<_>>()
                }),
            );
        }
        output_fields.insert(field, Value::Object(type_map));
    }

    Ok(json!({
        "indices": names,
        "fields": output_fields
    }))
}

fn resolve_indices(db: &Database, request: &FieldCapsRequest) -> StoreResult<Vec<String>> {
    let requested = if request.indices.is_empty()
        || request
            .indices
            .iter()
            .any(|index| matches!(index.as_str(), "_all" | "*"))
    {
        db.indexes.keys().cloned().collect::<Vec<_>>()
    } else {
        request.indices.clone()
    };
    let mut names = Vec::new();
    for requested_name in requested {
        if requested_name.contains('*') {
            let mut matches = db
                .indexes
                .keys()
                .filter(|name| wildcard_matches(&requested_name, name))
                .cloned()
                .collect::<Vec<_>>();
            if matches.is_empty() && !request.allow_no_indices && !request.ignore_unavailable {
                return Err(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{requested_name}]"),
                ));
            }
            names.append(&mut matches);
            continue;
        }
        let resolved = db.resolve_indices(&requested_name);
        if resolved.is_empty() {
            if request.ignore_unavailable {
                continue;
            }
            return Err(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{requested_name}]"),
            ));
        } else {
            names.extend(resolved);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn mapped_fields(mappings: &Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    collect_mapped_fields(mappings, "", &mut fields);
    fields
}

fn collect_mapped_fields(mappings: &Value, prefix: &str, fields: &mut Vec<(String, String)>) {
    let Some(properties) = mappings.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, mapping) in properties {
        let full_name = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if let Some(ty) = mapping_type(mapping) {
            fields.push((full_name.clone(), ty));
        }
        collect_mapped_fields(mapping, &full_name, fields);
    }
}

fn mapping_type(mapping: &Value) -> Option<String> {
    mapping
        .get("type")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            mapping
                .get("properties")
                .is_some()
                .then(|| "object".to_string())
        })
}

fn observed_fields(source: &Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    collect_observed_fields(source, "", &mut fields);
    fields
}

fn collect_observed_fields(value: &Value, prefix: &str, fields: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            if !prefix.is_empty() {
                fields.push((prefix.to_string(), "object".to_string()));
            }
            for (name, value) in object {
                let full_name = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                collect_observed_fields(value, &full_name, fields);
            }
        }
        Value::Array(values) => {
            let mut types = BTreeSet::new();
            for value in values {
                match value {
                    Value::Object(_) => collect_observed_fields(value, prefix, fields),
                    Value::Array(_) => collect_observed_fields(value, prefix, fields),
                    scalar => {
                        if let Some(ty) = observed_scalar_type(scalar) {
                            types.insert(ty);
                        }
                    }
                }
            }
            for ty in types {
                fields.push((prefix.to_string(), ty));
            }
        }
        scalar => {
            if let Some(ty) = observed_scalar_type(scalar) {
                fields.push((prefix.to_string(), ty));
            }
        }
    }
}

fn observed_scalar_type(value: &Value) -> Option<String> {
    match value {
        Value::Bool(_) => Some("boolean".to_string()),
        Value::Number(number) if number.is_i64() || number.is_u64() => Some("long".to_string()),
        Value::Number(_) => Some("double".to_string()),
        Value::String(_) => Some("keyword".to_string()),
        _ => None,
    }
}

fn insert_field_caps(
    fields: &mut BTreeMap<String, BTreeMap<String, FieldTypeCaps>>,
    field: &str,
    ty: &str,
    index: &str,
) {
    let caps = fields
        .entry(field.to_string())
        .or_default()
        .entry(ty.to_string())
        .or_insert_with(|| caps_for_type(ty));
    caps.indices.insert(index.to_string());
}

fn caps_for_type(ty: &str) -> FieldTypeCaps {
    FieldTypeCaps {
        indices: BTreeSet::new(),
        searchable: !matches!(ty, "object" | "nested"),
        aggregatable: !matches!(ty, "text" | "object" | "nested"),
    }
}

fn field_matches_any(field: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| pattern == "*" || wildcard_matches(pattern, field))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut remaining = value;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            first = false;
            continue;
        }
        if first && !pattern.starts_with('*') {
            let Some(stripped) = remaining.strip_prefix(part) else {
                return false;
            };
            remaining = stripped;
        } else if let Some(position) = remaining.find(part) {
            remaining = &remaining[position + part.len()..];
        } else {
            return false;
        }
        first = false;
    }
    pattern.ends_with('*') || remaining.is_empty()
}
