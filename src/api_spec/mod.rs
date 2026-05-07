pub mod generated;
pub mod tier;

use crate::rest_path::decode_path_param;
use http::Method;

pub use generated::{inventory, ApiRoute};
pub use tier::Tier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessClass {
    Read,
    Write,
    Admin,
}

impl AccessClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteMatch {
    pub api_name: &'static str,
    pub tier: Tier,
    pub access: AccessClass,
}

pub fn classify(method: &Method, path: &str) -> RouteMatch {
    let path = path.trim_end_matches('/');
    let decoded_segments = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(decode_path_param)
        .collect::<Vec<_>>();
    let segments: Vec<&str> = decoded_segments.iter().map(String::as_str).collect();

    if path.is_empty() || path == "/" {
        return match *method {
            Method::GET | Method::HEAD => route("info", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("info", method),
        };
    }

    if segments.as_slice() == ["_tasks"] {
        return route("security_or_control", Tier::Unsupported, AccessClass::Admin);
    }
    if matches!(segments.as_slice(), ["_tasks", _]) {
        return match *method {
            Method::GET => route("tasks.get", Tier::Implemented, AccessClass::Read),
            _ => route("tasks.get", Tier::Unsupported, AccessClass::Admin),
        };
    }
    if segments.first() == Some(&"_snapshot") {
        return classify_snapshot(method, &segments);
    }
    if pit_family(&segments) {
        return classify_pit(method, &segments);
    }
    if segments.as_slice() == ["_plugins", "_security", "api", "account"] {
        return match *method {
            Method::GET => route("security.account", Tier::Mocked, AccessClass::Read),
            _ => route("security.account", Tier::Unsupported, AccessClass::Admin),
        };
    }
    if control_namespace(&segments) {
        return route("security_or_control", Tier::Unsupported, AccessClass::Admin);
    }

    if segments.as_slice() == ["_plugins", "_query", "_datasources"] {
        return match *method {
            Method::GET => route("query.datasources", Tier::Mocked, AccessClass::Read),
            _ => route("query.datasources", Tier::Unsupported, AccessClass::Write),
        };
    }
    if matches!(
        segments.as_slice(),
        ["_plugins", "_query", "_datasources", ..]
    ) {
        return unsupported_method("query.datasources", method);
    }

    if path == "/_reindex" {
        return match *method {
            Method::POST => route("reindex", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("reindex", method),
        };
    }
    if path == "/_search/scroll" || path == "/_scroll" {
        return match *method {
            Method::GET | Method::POST => route("scroll", Tier::Implemented, AccessClass::Read),
            Method::DELETE => route("clear_scroll", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("scroll", method),
        };
    }
    if matches!(
        segments.as_slice(),
        ["_search", "scroll", _] | ["_scroll", _]
    ) {
        return match *method {
            Method::GET | Method::POST => route("scroll", Tier::Implemented, AccessClass::Read),
            Method::DELETE => route("clear_scroll", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("scroll", method),
        };
    }
    if path == "/_cluster/health" {
        return get_only(method, "cluster.health", Tier::BestEffort);
    }
    if nodes_stats_path(&segments) {
        return get_only(method, "nodes.stats", Tier::BestEffort);
    }
    if nodes_stats_family(&segments) {
        return unsupported_method("nodes.stats", method);
    }
    if nodes_info_path(&segments) {
        return get_only(method, "nodes.info", Tier::BestEffort);
    }
    if segments.first() == Some(&"_nodes") {
        return unsupported_method("nodes.info", method);
    }
    if path == "/_cluster/settings" {
        return match *method {
            Method::GET | Method::HEAD => {
                route("cluster.get_settings", Tier::BestEffort, AccessClass::Read)
            }
            Method::PUT => route("cluster.put_settings", Tier::Mocked, AccessClass::Admin),
            _ => unsupported_method("cluster.get_settings", method),
        };
    }
    if path == "/_cluster/stats" {
        return get_only(method, "cluster.stats", Tier::Implemented);
    }
    if matches!(segments.as_slice(), ["_resolve", "index", _]) {
        return match *method {
            Method::GET => route(
                "indices.resolve_index",
                Tier::Implemented,
                AccessClass::Read,
            ),
            _ => unsupported_method("indices.resolve_index", method),
        };
    }
    if segments.first() == Some(&"_resolve") {
        return unsupported_method("indices.resolve_index", method);
    }
    if matches!(segments.as_slice(), ["_analyze"] | [_, "_analyze"]) {
        return match *method {
            Method::GET | Method::POST => {
                route("indices.analyze", Tier::Implemented, AccessClass::Read)
            }
            _ => unsupported_method("indices.analyze", method),
        };
    }
    if segments.first() == Some(&"_analyze") || segments.get(1) == Some(&"_analyze") {
        return unsupported_method("indices.analyze", method);
    }
    if segments.as_slice() == ["_validate", "query"]
        || matches!(segments.as_slice(), [_, "_validate", "query"])
    {
        return match *method {
            Method::GET | Method::POST => route(
                "indices.validate_query",
                Tier::Implemented,
                AccessClass::Read,
            ),
            _ => unsupported_method("indices.validate_query", method),
        };
    }
    if segments.first() == Some(&"_validate") || segments.get(1) == Some(&"_validate") {
        return unsupported_method("indices.validate_query", method);
    }
    if path.starts_with("/_cat/") {
        if segments.get(1) == Some(&"plugins") {
            return get_only(method, "cat.plugins", Tier::Implemented);
        }
        if segments.get(1) == Some(&"templates") {
            return get_only(method, "cat.templates", Tier::Implemented);
        }
        return get_only(
            method,
            if segments.get(1) == Some(&"indices") {
                "cat.indices"
            } else {
                "cat"
            },
            Tier::BestEffort,
        );
    }
    if matches!(segments.as_slice(), ["_field_caps"] | [_, "_field_caps"]) {
        return match *method {
            Method::GET | Method::POST => route("field_caps", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("field_caps", method),
        };
    }
    if segments.first() == Some(&"_field_caps") || segments.get(1) == Some(&"_field_caps") {
        return unsupported_method("field_caps", method);
    }
    if matches!(segments.as_slice(), ["_bulk"] | [_, "_bulk"]) {
        return match *method {
            Method::POST | Method::PUT => route("bulk", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("bulk", method),
        };
    }
    if segments.first() == Some(&"_bulk") || segments.get(1) == Some(&"_bulk") {
        return unsupported_method("bulk", method);
    }
    if segments.len() == 3 && segments.get(1) == Some(&"_source") {
        return match *method {
            Method::GET | Method::HEAD => route(
                if *method == Method::HEAD {
                    "exists_source"
                } else {
                    "get_source"
                },
                Tier::Implemented,
                AccessClass::Read,
            ),
            _ => unsupported_method("get_source", method),
        };
    }
    if matches!(segments.as_slice(), ["_search"] | [_, "_search"]) {
        return match *method {
            Method::GET | Method::POST => route("search", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("search", method),
        };
    }
    if segments.get(1) == Some(&"_search") {
        return unsupported_method("search", method);
    }
    if matches!(segments.as_slice(), ["_delete_by_query", _, "_rethrottle"]) {
        return match *method {
            Method::POST => route(
                "delete_by_query_rethrottle",
                Tier::Mocked,
                AccessClass::Write,
            ),
            _ => unsupported_method("delete_by_query_rethrottle", method),
        };
    }
    if segments.as_slice() == ["_delete_by_query"] {
        return unsupported_method("delete_by_query", method);
    }
    if matches!(segments.as_slice(), [_, "_delete_by_query"]) {
        return match *method {
            Method::POST => route("delete_by_query", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("delete_by_query", method),
        };
    }
    if segments.first() == Some(&"_delete_by_query") || segments.get(1) == Some(&"_delete_by_query")
    {
        return unsupported_method("delete_by_query", method);
    }
    if matches!(segments.as_slice(), ["_update_by_query", _, "_rethrottle"]) {
        return match *method {
            Method::POST => route(
                "update_by_query_rethrottle",
                Tier::Mocked,
                AccessClass::Write,
            ),
            _ => unsupported_method("update_by_query_rethrottle", method),
        };
    }
    if segments.as_slice() == ["_update_by_query"] {
        return unsupported_method("update_by_query", method);
    }
    if matches!(segments.as_slice(), [_, "_update_by_query"]) {
        return match *method {
            Method::POST => route("update_by_query", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("update_by_query", method),
        };
    }
    if segments.first() == Some(&"_update_by_query") || segments.get(1) == Some(&"_update_by_query")
    {
        return unsupported_method("update_by_query", method);
    }
    if matches!(segments.as_slice(), ["_count"] | [_, "_count"]) {
        return match *method {
            Method::GET | Method::POST => route("count", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("count", method),
        };
    }
    if segments.first() == Some(&"_count") || segments.get(1) == Some(&"_count") {
        return unsupported_method("count", method);
    }
    if matches!(segments.as_slice(), ["_mget"] | [_, "_mget"]) {
        return match *method {
            Method::GET | Method::POST => route("mget", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("mget", method),
        };
    }
    if segments.first() == Some(&"_mget") || segments.get(1) == Some(&"_mget") {
        return unsupported_method("mget", method);
    }
    if matches!(segments.as_slice(), ["_msearch"] | [_, "_msearch"]) {
        return match *method {
            Method::GET | Method::POST => route("msearch", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("msearch", method),
        };
    }
    if segments.first() == Some(&"_msearch") || segments.get(1) == Some(&"_msearch") {
        return unsupported_method("msearch", method);
    }
    if segments.first() == Some(&"_stats") {
        if segments.len() > 2 {
            return unsupported_method("indices.stats", method);
        }
        return match *method {
            Method::GET => route("indices.stats", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("indices.stats", method),
        };
    }
    if segments.get(1) == Some(&"_stats") {
        if segments.len() > 3 {
            return unsupported_method("indices.stats", method);
        }
        return match *method {
            Method::GET => route("indices.stats", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("indices.stats", method),
        };
    }
    if matches!(segments.as_slice(), ["_refresh"] | [_, "_refresh"]) {
        return match *method {
            Method::GET | Method::POST => {
                route("indices.refresh", Tier::Implemented, AccessClass::Write)
            }
            _ => unsupported_method("indices.refresh", method),
        };
    }
    if segments.first() == Some(&"_refresh") || segments.get(1) == Some(&"_refresh") {
        return unsupported_method("indices.refresh", method);
    }
    if segments.first() == Some(&"_mapping") && segments.get(1) == Some(&"field") {
        if segments.len() != 3 {
            return unsupported_method("indices.get_field_mapping", method);
        }
        return match *method {
            Method::GET => route(
                "indices.get_field_mapping",
                Tier::Implemented,
                AccessClass::Read,
            ),
            _ => unsupported_method("indices.get_field_mapping", method),
        };
    }
    if segments.get(1) == Some(&"_mapping") && segments.get(2) == Some(&"field") {
        if segments.len() != 4 {
            return unsupported_method("indices.get_field_mapping", method);
        }
        return match *method {
            Method::GET => route(
                "indices.get_field_mapping",
                Tier::Implemented,
                AccessClass::Read,
            ),
            _ => unsupported_method("indices.get_field_mapping", method),
        };
    }
    if matches!(segments.as_slice(), ["_mapping"] | [_, "_mapping"]) {
        return match *method {
            Method::GET | Method::PUT => {
                let api_name = if *method == Method::PUT {
                    "indices.put_mapping"
                } else {
                    "indices.get_mapping"
                };
                route(
                    api_name,
                    Tier::Implemented,
                    if *method == Method::PUT {
                        AccessClass::Write
                    } else {
                        AccessClass::Read
                    },
                )
            }
            _ => unsupported_method("indices.get_mapping", method),
        };
    }
    if segments.first() == Some(&"_mapping") || segments.get(1) == Some(&"_mapping") {
        return unsupported_method("indices.get_mapping", method);
    }
    if matches!(segments.as_slice(), ["_settings"] | [_, "_settings"]) {
        return match *method {
            Method::GET | Method::PUT => {
                let api_name = if *method == Method::PUT {
                    "indices.put_settings"
                } else {
                    "indices.get_settings"
                };
                route(
                    api_name,
                    Tier::Implemented,
                    if *method == Method::PUT {
                        AccessClass::Write
                    } else {
                        AccessClass::Read
                    },
                )
            }
            _ => unsupported_method("indices.get_settings", method),
        };
    }
    if segments.first() == Some(&"_settings") || segments.get(1) == Some(&"_settings") {
        return unsupported_method("indices.get_settings", method);
    }
    if segments.first() == Some(&"_index_template") {
        return match *method {
            Method::GET if segments.len() <= 2 => route(
                "indices.get_index_template",
                Tier::Implemented,
                AccessClass::Read,
            ),
            Method::HEAD if segments.len() == 2 => route(
                "indices.exists_index_template",
                Tier::Implemented,
                AccessClass::Read,
            ),
            Method::PUT if segments.len() == 2 => route(
                "indices.put_index_template",
                Tier::Implemented,
                AccessClass::Write,
            ),
            Method::DELETE if segments.len() == 2 => route(
                "indices.delete_index_template",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("indices.put_index_template", method),
        };
    }
    if segments.first() == Some(&"_component_template") {
        return match *method {
            Method::GET if segments.len() <= 2 => route(
                "cluster.get_component_template",
                Tier::Implemented,
                AccessClass::Read,
            ),
            Method::HEAD if segments.len() == 2 => route(
                "cluster.exists_component_template",
                Tier::Implemented,
                AccessClass::Read,
            ),
            Method::PUT if segments.len() == 2 => route(
                "cluster.put_component_template",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            Method::DELETE if segments.len() == 2 => route(
                "cluster.delete_component_template",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            _ => unsupported_method("cluster.get_component_template", method),
        };
    }
    if segments.first() == Some(&"_template") {
        return match *method {
            Method::PUT if segments.len() == 2 => {
                route("indices.put_template", Tier::AgentWrite, AccessClass::Write)
            }
            Method::GET if segments.len() <= 2 => {
                route("indices.get_template", Tier::Implemented, AccessClass::Read)
            }
            Method::HEAD if segments.len() == 2 => route(
                "indices.exists_template",
                Tier::Implemented,
                AccessClass::Read,
            ),
            Method::DELETE if segments.len() == 2 => route(
                "indices.delete_template",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("indices.delete_template", method),
        };
    }
    if segments.first() == Some(&"_ingest") && segments.get(1) == Some(&"pipeline") {
        return match *method {
            Method::GET if segments.len() <= 3 => {
                route("ingest.get_pipeline", Tier::Implemented, AccessClass::Read)
            }
            Method::PUT if segments.len() == 3 => {
                route("ingest.put_pipeline", Tier::Implemented, AccessClass::Write)
            }
            Method::DELETE if segments.len() == 3 => route(
                "ingest.delete_pipeline",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("ingest.get_pipeline", method),
        };
    }
    if segments.first() == Some(&"_search") && segments.get(1) == Some(&"pipeline") {
        return match *method {
            Method::GET if segments.len() <= 3 => {
                route("search_pipeline.get", Tier::Implemented, AccessClass::Read)
            }
            Method::PUT if segments.len() == 3 => {
                route("search_pipeline.put", Tier::Implemented, AccessClass::Write)
            }
            Method::DELETE if segments.len() == 3 => route(
                "search_pipeline.delete",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("search_pipeline.get", method),
        };
    }
    if segments.as_slice() == ["_scripts", "painless", "_execute"] {
        return unsupported_method("scripts_painless_execute", method);
    }
    if segments.first() == Some(&"_scripts") {
        return match (method, segments.as_slice()) {
            (&Method::GET, ["_scripts", _]) => {
                route("get_script", Tier::Implemented, AccessClass::Read)
            }
            (&Method::PUT | &Method::POST, ["_scripts", _] | ["_scripts", _, _]) => {
                route("put_script", Tier::Implemented, AccessClass::Write)
            }
            (&Method::DELETE, ["_scripts", _]) => {
                route("delete_script", Tier::Implemented, AccessClass::Write)
            }
            _ => unsupported_method("get_script", method),
        };
    }
    if matches!(segments.as_slice(), ["_alias"] | ["_aliases"]) {
        return match *method {
            Method::GET => route("indices.get_alias", Tier::Implemented, AccessClass::Read),
            Method::POST => route(
                "indices.update_aliases",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("indices.get_alias", method),
        };
    }
    if matches!(segments.as_slice(), ["_alias", _]) {
        return match *method {
            Method::GET => route("indices.get_alias", Tier::Implemented, AccessClass::Read),
            Method::HEAD => route("indices.exists_alias", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("indices.get_alias", method),
        };
    }
    if matches!(
        segments.as_slice(),
        [_, "_alias"] | [_, "_aliases"] | [_, "_alias", _] | [_, "_aliases", _]
    ) {
        return match (method, segments.as_slice()) {
            (&Method::PUT, [_, "_alias", _]) => {
                route("indices.put_alias", Tier::Implemented, AccessClass::Write)
            }
            (&Method::GET, [_, "_alias"] | [_, "_aliases"]) => {
                route("indices.get_alias", Tier::Implemented, AccessClass::Read)
            }
            (&Method::GET, [_, "_alias", _] | [_, "_aliases", _]) => {
                route("indices.get_alias", Tier::Implemented, AccessClass::Read)
            }
            (&Method::HEAD, [_, "_alias", _] | [_, "_aliases", _]) => {
                route("indices.exists_alias", Tier::Implemented, AccessClass::Read)
            }
            (&Method::DELETE, [_, "_alias", _]) => route(
                "indices.delete_alias",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("indices.put_alias", method),
        };
    }
    if segments.first() == Some(&"_alias")
        || segments.first() == Some(&"_aliases")
        || segments.get(1) == Some(&"_alias")
        || segments.get(1) == Some(&"_aliases")
    {
        return unsupported_method("indices.put_alias", method);
    }
    if segments.len() >= 2 && matches!(segments[1], "_doc" | "_create" | "_update") {
        let api_name = match segments[1] {
            "_update" => "update",
            "_create" => "create",
            _ => "index",
        };
        let implemented = match segments.as_slice() {
            [_, "_doc"] => *method == Method::POST,
            [_, "_doc", _] => matches!(
                *method,
                Method::PUT | Method::POST | Method::GET | Method::HEAD | Method::DELETE
            ),
            [_, "_create", _] => matches!(*method, Method::PUT | Method::POST),
            [_, "_update", _] => *method == Method::POST,
            _ => false,
        };
        return if implemented {
            route(
                api_name,
                Tier::Implemented,
                if matches!(*method, Method::GET | Method::HEAD) {
                    AccessClass::Read
                } else {
                    AccessClass::Write
                },
            )
        } else {
            unsupported_method(api_name, method)
        };
    }
    if matches!(segments.as_slice(), [_, "_explain", _]) {
        return match *method {
            Method::GET | Method::POST => route("explain", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("explain", method),
        };
    }
    if segments.len() == 1 && !segments[0].starts_with('_') {
        return match *method {
            Method::PUT | Method::DELETE | Method::GET | Method::HEAD => {
                let api_name = match *method {
                    Method::PUT => "indices.create",
                    Method::DELETE => "indices.delete",
                    Method::GET => "indices.get",
                    Method::HEAD => "indices.exists",
                    _ => unreachable!(),
                };
                route(
                    api_name,
                    Tier::Implemented,
                    if matches!(*method, Method::GET | Method::HEAD) {
                        AccessClass::Read
                    } else {
                        AccessClass::Write
                    },
                )
            }
            _ => unsupported_method("indices", method),
        };
    }

    if let Some(route) = inventory().iter().find(|route| {
        route
            .methods
            .iter()
            .any(|candidate| *candidate == method.as_str())
            && route
                .paths
                .iter()
                .any(|pattern| path_matches(pattern, path))
    }) {
        return route_match(route.name, route.tier, route.access);
    }

    if let Some(route) = inventory().iter().find(|route| {
        route
            .paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
    }) {
        return unsupported_method(route.name, method);
    }

    if *method == Method::GET {
        return route("agent.read", Tier::AgentRead, AccessClass::Read);
    }

    unsupported_method("unsupported", method)
}

fn get_only(method: &Method, api_name: &'static str, tier: Tier) -> RouteMatch {
    match *method {
        Method::GET | Method::HEAD => route(api_name, tier, AccessClass::Read),
        _ => unsupported_method(api_name, method),
    }
}

fn classify_snapshot(method: &Method, segments: &[&str]) -> RouteMatch {
    if segments.iter().skip(1).any(|segment| *segment == "_status") {
        return route("snapshot.status", Tier::Unsupported, AccessClass::Admin);
    }

    match segments {
        ["_snapshot"] => match *method {
            Method::GET => route(
                "snapshot.get_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            _ => route(
                "snapshot.get_repository",
                Tier::Unsupported,
                AccessClass::Admin,
            ),
        },
        ["_snapshot", _] => match *method {
            Method::GET => route(
                "snapshot.get_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            Method::PUT | Method::POST => route(
                "snapshot.create_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            Method::DELETE => route(
                "snapshot.delete_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            _ => route(
                "snapshot.get_repository",
                Tier::Unsupported,
                AccessClass::Admin,
            ),
        },
        ["_snapshot", _, "_verify"] => match *method {
            Method::POST => route(
                "snapshot.verify_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            _ => route(
                "snapshot.verify_repository",
                Tier::Unsupported,
                AccessClass::Admin,
            ),
        },
        ["_snapshot", _, "_cleanup"] => match *method {
            Method::POST => route(
                "snapshot.cleanup_repository",
                Tier::Implemented,
                AccessClass::Admin,
            ),
            _ => route(
                "snapshot.cleanup_repository",
                Tier::Unsupported,
                AccessClass::Admin,
            ),
        },
        ["_snapshot", _, "_restore"] => {
            route("snapshot.restore", Tier::Unsupported, AccessClass::Admin)
        }
        ["_snapshot", _, "_clone"] => {
            route("snapshot.clone", Tier::Unsupported, AccessClass::Admin)
        }
        ["_snapshot", _, _, "_restore"] => match *method {
            Method::POST => route("snapshot.restore", Tier::Implemented, AccessClass::Admin),
            _ => route("snapshot.restore", Tier::Unsupported, AccessClass::Admin),
        },
        ["_snapshot", _, _, "_clone", ..] => {
            route("snapshot.clone", Tier::Unsupported, AccessClass::Admin)
        }
        ["_snapshot", _, _] => match *method {
            Method::GET => route("snapshot.get", Tier::Implemented, AccessClass::Admin),
            Method::PUT | Method::POST => {
                route("snapshot.create", Tier::Implemented, AccessClass::Admin)
            }
            Method::DELETE => route("snapshot.delete", Tier::Implemented, AccessClass::Admin),
            _ => route("snapshot.get", Tier::Unsupported, AccessClass::Admin),
        },
        _ => route("snapshot", Tier::Unsupported, AccessClass::Admin),
    }
}

fn pit_family(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["_search", "point_in_time", ..] | [_, "_search", "point_in_time", ..]
    )
}

fn classify_pit(method: &Method, segments: &[&str]) -> RouteMatch {
    match segments {
        [_, "_search", "point_in_time"] => match *method {
            Method::POST => route("create_pit", Tier::Implemented, AccessClass::Read),
            _ => route("create_pit", Tier::Unsupported, AccessClass::Read),
        },
        ["_search", "point_in_time"] => match *method {
            Method::DELETE => route("delete_pit", Tier::Implemented, AccessClass::Read),
            _ => route("delete_pit", Tier::Unsupported, AccessClass::Read),
        },
        ["_search", "point_in_time", "_all"] => match *method {
            Method::GET => route("get_all_pits", Tier::Implemented, AccessClass::Read),
            Method::DELETE => route("delete_all_pits", Tier::Implemented, AccessClass::Read),
            _ => route("get_all_pits", Tier::Unsupported, AccessClass::Read),
        },
        [_, "_search", "point_in_time", ..] => {
            route("create_pit", Tier::Unsupported, AccessClass::Read)
        }
        ["_search", "point_in_time", ..] => {
            route("get_all_pits", Tier::Unsupported, AccessClass::Read)
        }
        _ => route("point_in_time", Tier::Unsupported, AccessClass::Read),
    }
}

fn unsupported_method(api_name: &'static str, method: &Method) -> RouteMatch {
    route(
        api_name,
        Tier::Unsupported,
        if matches!(*method, Method::GET | Method::HEAD) {
            AccessClass::Read
        } else {
            AccessClass::Write
        },
    )
}

fn route(api_name: &'static str, tier: Tier, access: AccessClass) -> RouteMatch {
    route_match(api_name, tier, access)
}

fn route_match(api_name: &'static str, tier: Tier, access: AccessClass) -> RouteMatch {
    RouteMatch {
        api_name,
        tier,
        access,
    }
}

fn nodes_info_path(segments: &[&str]) -> bool {
    segments.first() == Some(&"_nodes") && segments.len() <= 3
}

fn nodes_stats_path(segments: &[&str]) -> bool {
    if segments.first() != Some(&"_nodes") {
        return false;
    }
    matches!(
        segments,
        ["_nodes", "stats"]
            | ["_nodes", "stats", _]
            | ["_nodes", "stats", _, _]
            | ["_nodes", _, "stats"]
            | ["_nodes", _, "stats", _]
            | ["_nodes", _, "stats", _, _]
    )
}

fn nodes_stats_family(segments: &[&str]) -> bool {
    segments.first() == Some(&"_nodes")
        && segments.iter().skip(1).any(|segment| *segment == "stats")
}

fn control_namespace(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["_plugins", "_security", ..]
            | ["_opendistro", "_security", ..]
            | ["_security", ..]
            | ["_snapshot", ..]
            | ["_dangling", ..]
            | ["_tasks", ..]
            | ["_task", ..]
    )
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let route_segments: Vec<&str> = pattern
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let path_segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if route_segments.len() != path_segments.len() {
        return false;
    }
    route_segments
        .iter()
        .zip(path_segments.iter())
        .all(|(route, actual)| {
            if route.starts_with('{') && route.ends_with('}') {
                !actual.is_empty() && !actual.starts_with('_')
            } else {
                route == actual
            }
        })
}
