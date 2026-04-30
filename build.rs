use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

fn main() {
    println!("cargo:rerun-if-changed=vendor/opensearch-rest-api-spec");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let api_dir = manifest_dir.join("vendor/opensearch-rest-api-spec/rest-api-spec/api");
    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("generated_api_spec.rs");

    let mut routes = Vec::new();
    for entry in fs::read_dir(&api_dir).unwrap_or_else(|error| {
        panic!(
            "failed to read vendored OpenSearch REST API spec at {}: {error}",
            api_dir.display()
        )
    }) {
        let entry = entry.expect("vendored spec entry is readable");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("_common.json") {
            continue;
        }
        routes.extend(routes_for_file(&path));
    }
    routes.sort_by(|left, right| left.name.cmp(&right.name));

    let mut generated = String::from(
        "use super::{AccessClass, Tier};\n\n\
#[derive(Debug, Clone, Copy)]\n\
pub struct ApiRoute {\n\
    pub name: &'static str,\n\
    pub tier: Tier,\n\
    pub access: AccessClass,\n\
    pub methods: &'static [&'static str],\n\
    pub paths: &'static [&'static str],\n\
}\n\n\
pub fn inventory() -> &'static [ApiRoute] {\n\
    &[\n",
    );
    for route in routes {
        generated.push_str("        ApiRoute {\n");
        generated.push_str(&format!("            name: {:?},\n", route.name));
        generated.push_str(&format!("            tier: {},\n", route.tier));
        generated.push_str(&format!("            access: {},\n", route.access));
        generated.push_str(&format!("            methods: &{:?},\n", route.methods));
        generated.push_str(&format!("            paths: &{:?},\n", route.paths));
        generated.push_str("        },\n");
    }
    generated.push_str("    ]\n}\n");

    fs::write(&out_path, generated).expect("generated API spec can be written");
}

#[derive(Debug)]
struct Route {
    name: String,
    tier: &'static str,
    access: &'static str,
    methods: Vec<String>,
    paths: Vec<String>,
}

fn routes_for_file(path: &Path) -> Vec<Route> {
    let raw = fs::read_to_string(path).expect("vendored API spec file can be read");
    let value: Value = serde_json::from_str(&raw).expect("vendored API spec file is JSON");
    let Some((name, body)) = value.as_object().and_then(|object| object.iter().next()) else {
        return Vec::new();
    };
    let Some(paths) = body
        .get("url")
        .and_then(|url| url.get("paths"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut routes = Vec::new();
    for route in paths {
        let Some(route_path) = route.get("path").and_then(Value::as_str) else {
            continue;
        };
        let mut methods = Vec::new();
        if let Some(route_methods) = route.get("methods").and_then(Value::as_array) {
            for method in route_methods.iter().filter_map(Value::as_str) {
                let method = method.to_ascii_uppercase();
                if !methods.contains(&method) {
                    methods.push(method);
                }
            }
        }
        methods.sort();
        routes.push(Route {
            name: name.to_string(),
            tier: tier_for(name, &methods),
            access: access_for(name, &methods),
            methods,
            paths: vec![route_path.to_string()],
        });
    }
    routes
}

fn access_for(name: &str, methods: &[String]) -> &'static str {
    if is_admin_api(name) {
        return "AccessClass::Admin";
    }
    if is_read_api(name)
        || methods
            .iter()
            .all(|method| matches!(method.as_str(), "GET" | "HEAD"))
    {
        return "AccessClass::Read";
    }
    "AccessClass::Write"
}

fn is_admin_api(name: &str) -> bool {
    name.starts_with("security.")
        || name.starts_with("snapshot.")
        || (name.starts_with("tasks.") && name != "tasks.get")
        || name.starts_with("cluster.put_")
        || name.starts_with("cluster.delete_")
}

fn is_read_api(name: &str) -> bool {
    matches!(
        name,
        "info"
            | "ping"
            | "get"
            | "get_source"
            | "exists"
            | "exists_source"
            | "search"
            | "count"
            | "field_caps"
            | "mget"
            | "msearch"
            | "scroll"
            | "clear_scroll"
            | "indices.get"
            | "indices.exists"
            | "indices.get_mapping"
            | "indices.get_field_mapping"
            | "indices.get_settings"
            | "indices.stats"
            | "indices.exists_index_template"
            | "indices.get_index_template"
            | "indices.exists_alias"
            | "indices.get_alias"
            | "cluster.health"
            | "cluster.get_settings"
            | "nodes.info"
            | "nodes.stats"
            | "tasks.get"
            | "cat.indices"
    ) || name.starts_with("cat.")
}

fn tier_for(name: &str, methods: &[String]) -> &'static str {
    if matches!(
        name,
        "info"
            | "ping"
            | "bulk"
            | "index"
            | "create"
            | "get"
            | "delete"
            | "update"
            | "search"
            | "count"
            | "mget"
            | "msearch"
            | "scroll"
            | "clear_scroll"
            | "get_source"
            | "exists_source"
            | "delete_by_query"
            | "update_by_query"
            | "reindex"
            | "indices.create"
            | "indices.get"
            | "indices.delete"
            | "indices.exists"
            | "indices.get_mapping"
            | "indices.put_mapping"
            | "indices.get_field_mapping"
            | "indices.get_settings"
            | "indices.put_settings"
            | "indices.stats"
            | "indices.refresh"
            | "indices.delete_template"
            | "indices.put_index_template"
            | "indices.get_index_template"
            | "indices.delete_index_template"
            | "indices.put_alias"
            | "indices.get_alias"
            | "indices.delete_alias"
            | "indices.update_aliases"
            | "field_caps"
            | "cluster.stats"
            | "tasks.get"
            | "cat.plugins"
            | "cat.templates"
    ) {
        return "Tier::Implemented";
    }
    if name.starts_with("cat.")
        || matches!(
            name,
            "cluster.health" | "cluster.get_settings" | "nodes.info" | "nodes.stats"
        )
    {
        return "Tier::BestEffort";
    }
    if methods
        .iter()
        .all(|method| matches!(method.as_str(), "GET" | "HEAD"))
        || matches!(
            name,
            "count"
                | "explain"
                | "field_caps"
                | "mget"
                | "msearch"
                | "msearch_template"
                | "mtermvectors"
                | "rank_eval"
                | "render_search_template"
                | "search_shards"
                | "search_template"
                | "termvectors"
        )
    {
        return "Tier::AgentRead";
    }
    "Tier::Unsupported"
}
