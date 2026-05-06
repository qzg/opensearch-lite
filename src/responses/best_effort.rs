use serde_json::{json, Map, Value};

use crate::responses::Response;

pub fn cluster_health(api_name: &str) -> Response {
    Response::json(
        200,
        json!({
            "cluster_name": "mainstack-search",
            "status": "green",
            "timed_out": false,
            "number_of_nodes": 1,
            "number_of_data_nodes": 1,
            "discovered_master": true,
            "active_primary_shards": 0,
            "active_shards": 0,
            "relocating_shards": 0,
            "initializing_shards": 0,
            "unassigned_shards": 0,
            "delayed_unassigned_shards": 0,
            "number_of_pending_tasks": 0,
            "number_of_in_flight_fetch": 0,
            "task_max_waiting_in_queue_millis": 0,
            "active_shards_percent_as_number": 100.0
        }),
    )
    .compatibility_signal(api_name, "best_effort")
}

pub fn nodes_info(
    api_name: &str,
    advertised_version: &str,
    node_ip: &str,
    publish_address: &str,
    filter_path: Option<&str>,
) -> Response {
    let body = json!({
        "cluster_name": "mainstack-search",
        "nodes": {
            "mainstack-search-local-node": {
                "name": "mainstack-search",
                "version": advertised_version,
                "ip": node_ip,
                "http": {
                    "publish_address": publish_address
                }
            }
        }
    });
    Response::json(200, apply_filter_path(body, filter_path))
        .compatibility_signal(api_name, "best_effort")
}

pub struct NodesStatsMetadata<'a> {
    pub advertised_version: &'a str,
    pub node_ip: &'a str,
    pub publish_address: &'a str,
    pub docs: usize,
    pub deleted: usize,
    pub store_bytes: usize,
    pub filter_path: Option<&'a str>,
}

pub fn nodes_stats(api_name: &str, metadata: NodesStatsMetadata<'_>) -> Response {
    let body = json!({
        "cluster_name": "mainstack-search",
        "nodes": {
            "mainstack-search-local-node": {
                "name": "mainstack-search",
                "version": metadata.advertised_version,
                "host": metadata.node_ip,
                "ip": metadata.node_ip,
                "http": {
                    "current_open": 0,
                    "total_opened": 0,
                    "publish_address": metadata.publish_address
                },
                "indices": {
                    "docs": {
                        "count": metadata.docs,
                        "deleted": metadata.deleted
                    },
                    "store": {
                        "size_in_bytes": metadata.store_bytes,
                        "reserved_in_bytes": 0
                    }
                }
            }
        }
    });
    Response::json(200, apply_filter_path(body, metadata.filter_path))
        .compatibility_signal(api_name, "best_effort")
}

pub fn empty_metadata(api_name: &str) -> Response {
    Response::json(200, json!({})).compatibility_signal(api_name, "best_effort")
}

fn apply_filter_path(body: Value, filter_path: Option<&str>) -> Value {
    let Some(filter_path) = filter_path else {
        return body;
    };
    let mut output = Value::Object(Map::new());
    for pattern in filter_path
        .split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
    {
        let parts = pattern
            .split('.')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if let Some(filtered) = filter_value(&body, &parts) {
            merge_value(&mut output, filtered);
        }
    }
    output
}

fn filter_value(source: &Value, parts: &[&str]) -> Option<Value> {
    if parts.is_empty() {
        return Some(source.clone());
    }
    let source = source.as_object()?;
    if parts[0] == "*" {
        let mut output = Map::new();
        for (key, value) in source {
            if let Some(filtered) = filter_value(value, &parts[1..]) {
                output.insert(key.clone(), filtered);
            }
        }
        if output.is_empty() {
            None
        } else {
            Some(Value::Object(output))
        }
    } else {
        let filtered = filter_value(source.get(parts[0])?, &parts[1..])?;
        let mut output = Map::new();
        output.insert(parts[0].to_string(), filtered);
        Some(Value::Object(output))
    }
}

fn merge_value(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, source_value) in source {
                if let Some(target_value) = target.get_mut(&key) {
                    merge_value(target_value, source_value);
                } else {
                    target.insert(key, source_value);
                }
            }
        }
        (target, source) => *target = source,
    }
}
