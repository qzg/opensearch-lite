use serde_json::json;

use crate::responses::Response;

pub fn cluster_health(api_name: &str) -> Response {
    Response::json(
        200,
        json!({
            "cluster_name": "opensearch-lite",
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

pub fn empty_metadata(api_name: &str) -> Response {
    Response::json(200, json!({})).compatibility_signal(api_name, "best_effort")
}
