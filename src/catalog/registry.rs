use serde_json::{json, Value};

use crate::{
    responses::{open_search_error, Response},
    storage::Database,
};

pub const COMPONENT_TEMPLATE: &str = "component_template";
pub const LEGACY_TEMPLATE: &str = "legacy_template";
pub const INGEST_PIPELINE: &str = "ingest_pipeline";
pub const SEARCH_PIPELINE: &str = "search_pipeline";
pub const SCRIPT: &str = "script";

pub fn get_component_templates(db: &Database, name: Option<&str>) -> Response {
    let objects = registry_objects(db, COMPONENT_TEMPLATE, name);
    if name.is_some() && objects.is_empty() {
        return missing(
            "component_template_missing_exception",
            "component template",
            name,
        );
    }
    Response::json(
        200,
        json!({
            "component_templates": objects.into_iter().map(|(name, raw)| {
                json!({
                    "name": name,
                    "component_template": raw
                })
            }).collect::<Vec<_>>()
        }),
    )
}

pub fn get_named_object(
    db: &Database,
    namespace: &str,
    name: Option<&str>,
    missing_type: &'static str,
    label: &'static str,
) -> Response {
    let objects = registry_objects(db, namespace, name);
    if name.is_some() && objects.is_empty() {
        return missing(missing_type, label, name);
    }
    Response::json(
        200,
        Value::Object(objects.into_iter().collect::<serde_json::Map<_, _>>()),
    )
}

pub fn get_script(db: &Database, name: &str) -> Response {
    let Some(raw) = db
        .registries
        .get(SCRIPT)
        .and_then(|registry| registry.get(name))
    else {
        return open_search_error(
            404,
            "resource_not_found_exception",
            format!("stored script [{name}] not found"),
            Some("Register the script first, or test arbitrary script behavior against full OpenSearch."),
        );
    };
    Response::json(
        200,
        json!({
            "_id": name,
            "found": true,
            "script": raw.get("script").cloned().unwrap_or_else(|| raw.clone())
        }),
    )
}

fn registry_objects(db: &Database, namespace: &str, name: Option<&str>) -> Vec<(String, Value)> {
    db.registries
        .get(namespace)
        .into_iter()
        .flat_map(|registry| registry.iter())
        .filter(|(object_name, _)| {
            name.map(|name| name == object_name.as_str())
                .unwrap_or(true)
        })
        .map(|(name, raw)| (name.clone(), raw.clone()))
        .collect()
}

fn missing(error_type: &'static str, label: &'static str, name: Option<&str>) -> Response {
    open_search_error(
        404,
        error_type,
        format!("{} [{}] missing", label, name.unwrap_or("<missing-name>")),
        Some("Create the registry object first, or test this API against full OpenSearch."),
    )
}
