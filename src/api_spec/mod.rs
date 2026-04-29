pub mod generated;
pub mod tier;

use http::Method;

pub use generated::{inventory, ApiRoute};
pub use tier::Tier;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteMatch {
    pub api_name: &'static str,
    pub tier: Tier,
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
            Method::GET | Method::HEAD => RouteMatch {
                api_name: "info",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("info"),
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
    if path.starts_with("/_cat/") {
        return get_only(method, "cat", Tier::BestEffort);
    }
    if path == "/_bulk" || segments.get(1) == Some(&"_bulk") {
        return match *method {
            Method::POST | Method::PUT => RouteMatch {
                api_name: "bulk",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("bulk"),
        };
    }
    if path == "/_search" || segments.get(1) == Some(&"_search") {
        return match *method {
            Method::GET | Method::POST => RouteMatch {
                api_name: "search",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("search"),
        };
    }
    if path == "/_count" || segments.get(1) == Some(&"_count") {
        return match *method {
            Method::GET | Method::POST => RouteMatch {
                api_name: "count",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("count"),
        };
    }
    if path == "/_mget" || segments.get(1) == Some(&"_mget") {
        return match *method {
            Method::GET | Method::POST => RouteMatch {
                api_name: "mget",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("mget"),
        };
    }
    if path == "/_msearch" || segments.get(1) == Some(&"_msearch") {
        return match *method {
            Method::GET | Method::POST => RouteMatch {
                api_name: "msearch",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("msearch"),
        };
    }
    if path == "/_refresh" || segments.get(1) == Some(&"_refresh") {
        return match *method {
            Method::GET | Method::POST => RouteMatch {
                api_name: "indices.refresh",
                tier: Tier::Implemented,
            },
            _ => unsupported_method("indices.refresh"),
        };
    }
    if segments.first() == Some(&"_mapping") || segments.get(1) == Some(&"_mapping") {
        return match *method {
            Method::GET | Method::PUT => RouteMatch {
                api_name: if *method == Method::PUT {
                    "indices.put_mapping"
                } else {
                    "indices.get_mapping"
                },
                tier: Tier::Implemented,
            },
            _ => unsupported_method("indices.get_mapping"),
        };
    }
    if segments.first() == Some(&"_settings") || segments.get(1) == Some(&"_settings") {
        return match *method {
            Method::GET | Method::PUT => RouteMatch {
                api_name: if *method == Method::PUT {
                    "indices.put_settings"
                } else {
                    "indices.get_settings"
                },
                tier: Tier::Implemented,
            },
            _ => unsupported_method("indices.get_settings"),
        };
    }
    if segments.first() == Some(&"_index_template") {
        return match *method {
            Method::PUT | Method::GET | Method::HEAD | Method::DELETE => RouteMatch {
                api_name: match *method {
                    Method::GET => "indices.get_index_template",
                    Method::HEAD => "indices.exists_index_template",
                    Method::DELETE => "indices.delete_index_template",
                    _ => "indices.put_index_template",
                },
                tier: Tier::Implemented,
            },
            _ => unsupported_method("indices.put_index_template"),
        };
    }
    if segments.first() == Some(&"_alias")
        || segments.first() == Some(&"_aliases")
        || segments.get(1) == Some(&"_alias")
        || segments.get(1) == Some(&"_aliases")
    {
        return match *method {
            Method::PUT | Method::GET | Method::HEAD | Method::DELETE | Method::POST => {
                RouteMatch {
                    api_name: match *method {
                        Method::GET => "indices.get_alias",
                        Method::HEAD => "indices.exists_alias",
                        Method::DELETE => "indices.delete_alias",
                        Method::POST => "indices.update_aliases",
                        _ => "indices.put_alias",
                    },
                    tier: Tier::Implemented,
                }
            }
            _ => unsupported_method("indices.put_alias"),
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
            RouteMatch {
                api_name,
                tier: Tier::Implemented,
            }
        } else {
            unsupported_method(api_name)
        };
    }
    if segments.len() == 1 && !segments[0].starts_with('_') {
        return match *method {
            Method::PUT | Method::DELETE | Method::GET | Method::HEAD => RouteMatch {
                api_name: match *method {
                    Method::PUT => "indices.create",
                    Method::DELETE => "indices.delete",
                    Method::GET | Method::HEAD => "indices.get",
                    _ => unreachable!(),
                },
                tier: Tier::Implemented,
            },
            _ => unsupported_method("indices"),
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
        return RouteMatch {
            api_name: route.name,
            tier: route.tier,
        };
    }

    if let Some(route) = inventory().iter().find(|route| {
        route
            .paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
    }) {
        return unsupported_method(route.name);
    }

    if *method == Method::GET {
        return RouteMatch {
            api_name: "agent.read",
            tier: Tier::AgentRead,
        };
    }

    RouteMatch {
        api_name: "unsupported",
        tier: Tier::Unsupported,
    }
}

fn get_only(method: &Method, api_name: &'static str, tier: Tier) -> RouteMatch {
    match *method {
        Method::GET | Method::HEAD => RouteMatch { api_name, tier },
        _ => unsupported_method(api_name),
    }
}

fn unsupported_method(api_name: &'static str) -> RouteMatch {
    RouteMatch {
        api_name,
        tier: Tier::Unsupported,
    }
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
