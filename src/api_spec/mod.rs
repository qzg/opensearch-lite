pub mod generated;
pub mod tier;

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
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

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
    if control_namespace(&segments) {
        return route("security_or_control", Tier::Unsupported, AccessClass::Admin);
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
    if path == "/_nodes" || path.starts_with("/_nodes/") {
        return get_only(method, "nodes.info", Tier::BestEffort);
    }
    if path == "/_cluster/settings" {
        return get_only(method, "cluster.get_settings", Tier::BestEffort);
    }
    if path == "/_cluster/stats" {
        return get_only(method, "cluster.stats", Tier::Implemented);
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
    if path == "/_field_caps" || segments.get(1) == Some(&"_field_caps") {
        return match *method {
            Method::GET | Method::POST => route("field_caps", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("field_caps", method),
        };
    }
    if path == "/_bulk" || segments.get(1) == Some(&"_bulk") {
        return match *method {
            Method::POST | Method::PUT => route("bulk", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("bulk", method),
        };
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
    if path == "/_search" || segments.get(1) == Some(&"_search") {
        return match *method {
            Method::GET | Method::POST => route("search", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("search", method),
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
    if segments.as_slice() == ["_update_by_query"] {
        return unsupported_method("update_by_query", method);
    }
    if matches!(segments.as_slice(), [_, "_update_by_query"]) {
        return match *method {
            Method::POST => route("update_by_query", Tier::Implemented, AccessClass::Write),
            _ => unsupported_method("update_by_query", method),
        };
    }
    if path == "/_count" || segments.get(1) == Some(&"_count") {
        return match *method {
            Method::GET | Method::POST => route("count", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("count", method),
        };
    }
    if path == "/_mget" || segments.get(1) == Some(&"_mget") {
        return match *method {
            Method::GET | Method::POST => route("mget", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("mget", method),
        };
    }
    if path == "/_msearch" || segments.get(1) == Some(&"_msearch") {
        return match *method {
            Method::GET | Method::POST => route("msearch", Tier::Implemented, AccessClass::Read),
            _ => unsupported_method("msearch", method),
        };
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
    if path == "/_refresh" || segments.get(1) == Some(&"_refresh") {
        return match *method {
            Method::GET | Method::POST => {
                route("indices.refresh", Tier::Implemented, AccessClass::Write)
            }
            _ => unsupported_method("indices.refresh", method),
        };
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
    if segments.first() == Some(&"_mapping") || segments.get(1) == Some(&"_mapping") {
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
    if segments.first() == Some(&"_settings") || segments.get(1) == Some(&"_settings") {
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
    if segments.first() == Some(&"_index_template") {
        return match *method {
            Method::PUT | Method::GET | Method::HEAD | Method::DELETE => {
                let api_name = match *method {
                    Method::GET => "indices.get_index_template",
                    Method::HEAD => "indices.exists_index_template",
                    Method::DELETE => "indices.delete_index_template",
                    _ => "indices.put_index_template",
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
            _ => unsupported_method("indices.put_index_template", method),
        };
    }
    if segments.first() == Some(&"_template") {
        return match *method {
            Method::DELETE if segments.len() == 2 => route(
                "indices.delete_template",
                Tier::Implemented,
                AccessClass::Write,
            ),
            _ => unsupported_method("indices.delete_template", method),
        };
    }
    if segments.first() == Some(&"_alias")
        || segments.first() == Some(&"_aliases")
        || segments.get(1) == Some(&"_alias")
        || segments.get(1) == Some(&"_aliases")
    {
        return match *method {
            Method::PUT | Method::GET | Method::HEAD | Method::DELETE | Method::POST => {
                let api_name = match *method {
                    Method::GET => "indices.get_alias",
                    Method::HEAD => "indices.exists_alias",
                    Method::DELETE => "indices.delete_alias",
                    Method::POST => "indices.update_aliases",
                    _ => "indices.put_alias",
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
            _ => unsupported_method("indices.put_alias", method),
        };
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

fn control_namespace(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["_plugins", "_security", ..]
            | ["_opendistro", "_security", ..]
            | ["_security", ..]
            | ["_snapshot", ..]
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
