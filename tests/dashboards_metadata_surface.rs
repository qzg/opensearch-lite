mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, ephemeral_state};

#[tokio::test]
async fn dashboards_data_view_metadata_is_deterministic() {
    let state = ephemeral_state();

    let create = call(
        &state,
        Method::PUT,
        "/orders",
        json!({
            "settings": { "index": { "number_of_shards": 1, "number_of_replicas": 0 } },
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" },
                    "message": { "type": "text" },
                    "created_at": { "type": "date" },
                    "customer": {
                        "properties": {
                            "tier": { "type": "keyword" }
                        }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(create.status, 200);

    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({
            "status": "paid",
            "message": "Northwind order",
            "created_at": "2026-04-30T12:00:00Z",
            "customer": { "tier": "gold" },
            "total": 42,
            "priority": true,
            "tags": ["coffee", "hardware"]
        }),
    )
    .await;

    let exists = call(&state, Method::HEAD, "/orders", Value::Null).await;
    assert_eq!(exists.status, 200);
    assert!(exists.body.is_none());

    let missing = call(&state, Method::HEAD, "/missing", Value::Null).await;
    assert_eq!(missing.status, 404);
    assert!(missing.body.is_none());

    let get_caps = call(
        &state,
        Method::GET,
        "/orders/_field_caps?fields=*",
        Value::Null,
    )
    .await;
    assert_eq!(get_caps.status, 200);
    let body = get_caps.body.unwrap();
    assert_eq!(body["fields"]["status"]["keyword"]["type"], "keyword");
    assert_eq!(body["fields"]["status"]["keyword"]["searchable"], true);
    assert_eq!(body["fields"]["status"]["keyword"]["aggregatable"], true);
    assert_eq!(body["fields"]["message"]["text"]["aggregatable"], false);
    assert_eq!(body["fields"]["created_at"]["date"]["type"], "date");
    assert_eq!(
        body["fields"]["customer.tier"]["keyword"]["type"],
        "keyword"
    );
    assert_eq!(body["fields"]["total"]["long"]["type"], "long");
    assert_eq!(body["fields"]["priority"]["boolean"]["type"], "boolean");
    assert_eq!(body["fields"]["tags"]["keyword"]["type"], "keyword");

    let post_caps = call(
        &state,
        Method::POST,
        "/_field_caps?fields=status,total",
        json!({}),
    )
    .await;
    assert_eq!(post_caps.status, 200);
    let body = post_caps.body.unwrap();
    assert!(body["fields"]["status"].is_object());
    assert!(body["fields"]["total"].is_object());
    assert!(body["fields"].get("message").is_none());

    let plugins = call(
        &state,
        Method::GET,
        "/_cat/plugins?format=json",
        Value::Null,
    )
    .await;
    assert_eq!(plugins.status, 200);
    assert_eq!(plugins.body.unwrap(), json!([]));

    let templates = call(
        &state,
        Method::GET,
        "/_cat/templates?format=json",
        Value::Null,
    )
    .await;
    assert_eq!(templates.status, 200);
    assert_eq!(templates.body.unwrap(), json!([]));

    let stats = call(&state, Method::GET, "/_cluster/stats", Value::Null).await;
    assert_eq!(stats.status, 200);
    let body = stats.body.unwrap();
    assert_eq!(body["cluster_name"], "opensearch-lite");
    assert!(body["cluster_uuid"]
        .as_str()
        .unwrap()
        .starts_with("opensearch-lite-"));
    assert_eq!(body["nodes"]["count"]["total"], 1);
    assert_eq!(body["indices"]["count"], 1);
    assert_eq!(body["indices"]["docs"]["count"], 1);
    assert!(body["indices"]["store"]["size_in_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn field_caps_handles_empty_missing_malformed_and_conflicting_states() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/mapped-empty",
        json!({
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" },
                    "message": { "type": "text" }
                }
            }
        }),
    )
    .await;
    call(&state, Method::PUT, "/unmapped-empty", json!({})).await;
    call(&state, Method::PUT, "/mixed", json!({})).await;
    call(&state, Method::PUT, "/mixed/_doc/1", json!({ "value": 1 })).await;
    call(
        &state,
        Method::PUT,
        "/mixed/_doc/2",
        json!({ "value": "one" }),
    )
    .await;

    let mapped = call(
        &state,
        Method::GET,
        "/mapped-empty/_field_caps?fields=*",
        Value::Null,
    )
    .await;
    assert_eq!(mapped.status, 200);
    assert_eq!(
        mapped.body.unwrap()["fields"]["status"]["keyword"]["type"],
        "keyword"
    );

    call(
        &state,
        Method::PUT,
        "/mapped-empty/_doc/1",
        json!({ "status": "mapped value", "message": "mapped text" }),
    )
    .await;
    let mapped_with_observed_value = call(
        &state,
        Method::GET,
        "/mapped-empty/_field_caps?fields=message",
        Value::Null,
    )
    .await;
    assert_eq!(mapped_with_observed_value.status, 200);
    let body = mapped_with_observed_value.body.unwrap();
    assert!(body["fields"]["message"]["text"].is_object());
    assert_eq!(
        body["fields"]["message"].as_object().unwrap().len(),
        1,
        "explicit mappings stay authoritative over observed values"
    );

    let unmapped = call(
        &state,
        Method::GET,
        "/unmapped-empty/_field_caps?fields=*",
        Value::Null,
    )
    .await;
    assert_eq!(unmapped.status, 200);
    assert_eq!(unmapped.body.unwrap()["fields"], json!({}));

    let missing_ignored = call(
        &state,
        Method::GET,
        "/missing/_field_caps?fields=*&ignore_unavailable=true",
        Value::Null,
    )
    .await;
    assert_eq!(missing_ignored.status, 200);
    assert_eq!(missing_ignored.body.unwrap()["fields"], json!({}));

    let missing = call(
        &state,
        Method::GET,
        "/missing/_field_caps?fields=*",
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "index_not_found_exception"
    );

    let malformed = call(
        &state,
        Method::GET,
        "/mapped-empty/_field_caps?fields=",
        Value::Null,
    )
    .await;
    assert_eq!(malformed.status, 400);

    let mixed = call(
        &state,
        Method::GET,
        "/mixed/_field_caps?fields=value",
        Value::Null,
    )
    .await;
    assert_eq!(mixed.status, 200);
    let body = mixed.body.unwrap();
    assert!(body["fields"]["value"]["long"].is_object());
    assert!(body["fields"]["value"]["keyword"].is_object());
}

#[tokio::test]
async fn alias_remove_index_can_replace_index_with_alias() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/test", json!({})).await;
    call(&state, Method::PUT, "/test_2", json!({})).await;

    let response = call(
        &state,
        Method::POST,
        "/_alias",
        json!({
            "actions": [
                { "add": { "index": "test_2", "aliases": ["test", "test_write"] } },
                { "remove_index": { "index": "test" } }
            ]
        }),
    )
    .await;
    assert_eq!(response.status, 200);

    assert_eq!(
        call(&state, Method::HEAD, "/_alias/test", Value::Null)
            .await
            .status,
        200
    );
    assert_eq!(
        call(&state, Method::HEAD, "/_alias/test_write", Value::Null)
            .await
            .status,
        200
    );
    let get = call(&state, Method::GET, "/test", Value::Null).await;
    assert_eq!(get.status, 200);
    assert!(get.body.unwrap()["test_2"].is_object());
}

#[tokio::test]
async fn alias_actions_preserve_order_except_index_replacement_conflicts() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/old", json!({})).await;
    call(&state, Method::PUT, "/new", json!({})).await;
    call(&state, Method::PUT, "/old/_alias/old_alias", json!({})).await;

    let response = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "remove": { "index": "old", "alias": "old_alias" } },
                { "remove_index": { "index": "old" } },
                { "add": { "index": "new", "alias": "old" } }
            ]
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        call(&state, Method::HEAD, "/_alias/old_alias", Value::Null)
            .await
            .status,
        404
    );
    let get = call(&state, Method::GET, "/old", Value::Null).await;
    assert_eq!(get.status, 200);
    assert!(get.body.unwrap()["new"].is_object());

    let malformed = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                {
                    "add": { "index": "new", "alias": "also_new" },
                    "remove_index": { "index": "new" }
                }
            ]
        }),
    )
    .await;
    assert_eq!(malformed.status, 400);
}

#[tokio::test]
async fn legacy_template_delete_is_deterministic() {
    let state = ephemeral_state();

    let put_composable = call(
        &state,
        Method::PUT,
        "/_index_template/opensearch_dashboards",
        json!({
            "index_patterns": [".opensearch_dashboards*"],
            "template": {}
        }),
    )
    .await;
    assert_eq!(put_composable.status, 200);

    let missing = call(
        &state,
        Method::DELETE,
        "/_template/opensearch_dashboards",
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "index_template_missing_exception"
    );

    let get_composable = call(
        &state,
        Method::GET,
        "/_index_template/opensearch_dashboards",
        Value::Null,
    )
    .await;
    assert_eq!(get_composable.status, 200);
    assert_eq!(
        get_composable.body.unwrap()["index_templates"][0]["name"],
        "opensearch_dashboards"
    );
}
