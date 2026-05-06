mod support;

use http::Method;
use mainstack_search::{
    server::AppState,
    storage::mutation_log::{self, Mutation},
    Config,
};
use serde_json::{json, Value};
use std::fs;
use support::{call, durable_state, ephemeral_state, ndjson_call};

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

    let create_shadow = call(
        &state,
        Method::PUT,
        "/orders-shadow",
        json!({
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" }
                }
            }
        }),
    )
    .await;
    assert_eq!(create_shadow.status, 200);

    let hidden = call(
        &state,
        Method::PUT,
        "/.opensearch_dashboards",
        json!({ "mappings": { "properties": { "type": { "type": "keyword" } } } }),
    )
    .await;
    assert_eq!(hidden.status, 200);

    let alias = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "add": { "indices": ["orders", "orders-shadow"], "alias": "orders-read" } }
            ]
        }),
    )
    .await;
    assert_eq!(alias.status, 200);

    let resolved = call(&state, Method::GET, "/_resolve/index/*", Value::Null).await;
    assert_eq!(resolved.status, 200);
    let body = resolved.body.unwrap();
    assert_eq!(
        body["indices"],
        json!([
            {
                "name": "orders",
                "aliases": ["orders-read"],
                "attributes": ["open"]
            },
            {
                "name": "orders-shadow",
                "aliases": ["orders-read"],
                "attributes": ["open"]
            }
        ])
    );
    assert_eq!(
        body["aliases"],
        json!([{
            "name": "orders-read",
            "indices": ["orders", "orders-shadow"]
        }])
    );
    assert_eq!(body["data_streams"], json!([]));

    let resolved_alias = call(
        &state,
        Method::GET,
        "/_resolve/index/orders-read",
        Value::Null,
    )
    .await;
    assert_eq!(resolved_alias.status, 200);
    let body = resolved_alias.body.unwrap();
    assert_eq!(body["indices"], json!([]));
    assert_eq!(
        body["aliases"],
        json!([{
            "name": "orders-read",
            "indices": ["orders", "orders-shadow"]
        }])
    );

    let resolved_all = call(
        &state,
        Method::GET,
        "/_resolve/index/*?expand_wildcards=all",
        Value::Null,
    )
    .await;
    assert_eq!(resolved_all.status, 200);
    let body = resolved_all.body.unwrap();
    assert_eq!(body["indices"][0]["name"], ".opensearch_dashboards");
    assert_eq!(body["indices"][0]["attributes"], json!(["open", "hidden"]));

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
    assert_eq!(body["cluster_name"], "mainstack-search");
    assert!(body["cluster_uuid"]
        .as_str()
        .unwrap()
        .starts_with("mainstack-search-"));
    assert_eq!(body["nodes"]["count"]["total"], 1);
    assert_eq!(body["indices"]["count"], 3);
    assert_eq!(body["indices"]["docs"]["count"], 1);
    assert!(body["indices"]["store"]["size_in_bytes"].as_u64().unwrap() > 0);

    let nodes = call(
        &state,
        Method::GET,
        "/_nodes?filter_path=nodes.*.version,nodes.*.http.publish_address,nodes.*.ip",
        Value::Null,
    )
    .await;
    assert_eq!(nodes.status, 200);
    assert_eq!(
        nodes
            .headers
            .get("x-mainstack-search-api")
            .map(String::as_str),
        Some("nodes.info")
    );
    assert_eq!(
        nodes
            .headers
            .get("x-mainstack-search-tier")
            .map(String::as_str),
        Some("best_effort")
    );
    let body = nodes.body.unwrap();
    assert!(body.get("cluster_name").is_none());
    let nodes = body["nodes"].as_object().expect("nodes is an object");
    assert_eq!(nodes.len(), 1);
    let node = nodes.values().next().expect("local node is present");
    assert_eq!(node["version"], "3.6.0");
    assert_eq!(node["ip"], "127.0.0.1");
    assert_eq!(node["http"]["publish_address"], "127.0.0.1:9200");
    assert!(node.get("name").is_none());
}

#[tokio::test]
async fn dashboards_saved_object_reference_ids_round_trip_as_decoded_document_ids() {
    let state = ephemeral_state();

    let data_view = call(
        &state,
        Method::PUT,
        "/.kibana/_create/index-pattern%3Aorders",
        json!({
            "type": "index-pattern",
            "index-pattern": { "title": "orders", "timeFieldName": "created_at" },
            "references": []
        }),
    )
    .await;
    assert_eq!(data_view.status, 201);
    assert_eq!(data_view.body.unwrap()["_id"], "index-pattern:orders");

    let visualization = call(
        &state,
        Method::PUT,
        "/.kibana/_create/visualization%3Aorders-status-vis",
        json!({
            "type": "visualization",
            "visualization": { "title": "Orders by status" },
            "references": [
                { "type": "index-pattern", "id": "orders", "name": "kibanaSavedObjectMeta.searchSourceJSON.index" }
            ]
        }),
    )
    .await;
    assert_eq!(visualization.status, 201);
    assert_eq!(
        visualization.body.unwrap()["_id"],
        "visualization:orders-status-vis"
    );

    let export_reference_lookup = call(
        &state,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": ".kibana", "_id": "index-pattern:orders" },
                { "_index": ".kibana", "_id": "visualization:orders-status-vis" }
            ]
        }),
    )
    .await;
    assert_eq!(export_reference_lookup.status, 200);
    let body = export_reference_lookup.body.unwrap();
    assert_eq!(body["docs"][0]["found"], true);
    assert_eq!(body["docs"][0]["_id"], "index-pattern:orders");
    assert_eq!(body["docs"][1]["found"], true);
    assert_eq!(body["docs"][1]["_id"], "visualization:orders-status-vis");
}

#[tokio::test]
async fn dashboards_saved_object_import_bulk_survives_durable_restart() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    let index = call(&state, Method::PUT, "/.kibana_1", json!({})).await;
    assert_eq!(index.status, 200);
    let alias = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "add": { "index": ".kibana_1", "alias": ".kibana" } }
            ]
        }),
    )
    .await;
    assert_eq!(alias.status, 200);

    let import = ndjson_call(
        &state,
        Method::POST,
        "/_bulk?refresh=wait_for",
        r#"{"index":{"_index":".kibana","_id":"index-pattern:orders"}}
{"type":"index-pattern","index-pattern":{"title":"orders","timeFieldName":"created_at"},"references":[]}
{"index":{"_index":".kibana","_id":"visualization:orders-status-vis"}}
{"type":"visualization","visualization":{"title":"Orders by status"},"references":[{"type":"index-pattern","id":"orders","name":"kibanaSavedObjectMeta.searchSourceJSON.index"}]}
{"index":{"_index":".kibana","_id":"dashboard:orders-dashboard"}}
{"type":"dashboard","dashboard":{"title":"Orders dashboard"},"references":[{"type":"visualization","id":"orders-status-vis","name":"panel_0"}]}
"#,
    )
    .await;
    assert_eq!(import.status, 200);
    let body = import.body.unwrap();
    assert_eq!(body["errors"], false);
    assert_eq!(body["items"].as_array().unwrap().len(), 3);

    let count = call(
        &state,
        Method::POST,
        "/.kibana/_count",
        json!({ "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(count.status, 200);
    assert_eq!(count.body.unwrap()["count"], 3);

    drop(state);

    let replayed = durable_state(temp.path());
    let dashboard = call(
        &replayed,
        Method::GET,
        "/.kibana/_doc/dashboard%3Aorders-dashboard",
        Value::Null,
    )
    .await;
    assert_eq!(dashboard.status, 200);
    let body = dashboard.body.unwrap();
    assert_eq!(body["_id"], "dashboard:orders-dashboard");
    assert_eq!(body["_source"]["dashboard"]["title"], "Orders dashboard");
    assert_eq!(
        body["_source"]["references"],
        json!([{ "type": "visualization", "id": "orders-status-vis", "name": "panel_0" }])
    );

    let reference_lookup = call(
        &replayed,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": ".kibana", "_id": "index-pattern:orders" },
                { "_index": ".kibana", "_id": "visualization:orders-status-vis" }
            ]
        }),
    )
    .await;
    assert_eq!(reference_lookup.status, 200);
    let body = reference_lookup.body.unwrap();
    assert_eq!(body["docs"][0]["found"], true);
    assert_eq!(body["docs"][0]["_source"]["references"], json!([]));
    assert_eq!(body["docs"][1]["found"], true);
    assert_eq!(
        body["docs"][1]["_source"]["references"],
        json!([{ "type": "index-pattern", "id": "orders", "name": "kibanaSavedObjectMeta.searchSourceJSON.index" }])
    );

    let replayed_count = call(
        &replayed,
        Method::POST,
        "/.kibana/_count",
        json!({ "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(replayed_count.status, 200);
    assert_eq!(replayed_count.body.unwrap()["count"], 3);
}

#[tokio::test]
async fn dashboards_saved_object_import_conflict_modes_match_bulk_traffic() {
    let state = ephemeral_state();

    let index = call(&state, Method::PUT, "/.kibana_1", json!({})).await;
    assert_eq!(index.status, 200);
    let alias = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "add": { "index": ".kibana_1", "alias": ".kibana" } }
            ]
        }),
    )
    .await;
    assert_eq!(alias.status, 200);

    let original = call(
        &state,
        Method::PUT,
        "/.kibana/_create/dashboard%3Aorders-dashboard?refresh=wait_for",
        json!({
            "type": "dashboard",
            "dashboard": { "title": "Original dashboard" },
            "references": []
        }),
    )
    .await;
    assert_eq!(original.status, 201);

    let overwrite_false = ndjson_call(
        &state,
        Method::POST,
        "/_bulk?refresh=wait_for",
        r#"{"create":{"_index":".kibana","_id":"dashboard:orders-dashboard"}}
{"type":"dashboard","dashboard":{"title":"Imported dashboard"},"references":[]}
"#,
    )
    .await;
    assert_eq!(overwrite_false.status, 200);
    let body = overwrite_false.body.unwrap();
    assert_eq!(body["errors"], true);
    assert_eq!(body["items"][0]["create"]["status"], 409);

    let existing = call(
        &state,
        Method::GET,
        "/.kibana/_doc/dashboard%3Aorders-dashboard",
        Value::Null,
    )
    .await;
    assert_eq!(existing.status, 200);
    assert_eq!(
        existing.body.unwrap()["_source"]["dashboard"]["title"],
        "Original dashboard"
    );

    let create_new_copies = ndjson_call(
        &state,
        Method::POST,
        "/_bulk?refresh=wait_for",
        r#"{"index":{"_index":".kibana","_id":"index-pattern:orders-copy"}}
{"type":"index-pattern","index-pattern":{"title":"orders-copy"},"references":[]}
{"index":{"_index":".kibana","_id":"dashboard:orders-dashboard-copy"}}
{"type":"dashboard","dashboard":{"title":"Imported dashboard copy"},"references":[{"type":"index-pattern","id":"orders-copy","name":"kibanaSavedObjectMeta.searchSourceJSON.index"}]}
"#,
    )
    .await;
    assert_eq!(create_new_copies.status, 200);
    let body = create_new_copies.body.unwrap();
    assert_eq!(body["errors"], false);
    assert_eq!(body["items"][0]["index"]["status"], 201);
    assert_eq!(body["items"][1]["index"]["status"], 201);

    let copied = call(
        &state,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": ".kibana", "_id": "dashboard:orders-dashboard" },
                { "_index": ".kibana", "_id": "dashboard:orders-dashboard-copy" },
                { "_index": ".kibana", "_id": "index-pattern:orders-copy" }
            ]
        }),
    )
    .await;
    assert_eq!(copied.status, 200);
    let body = copied.body.unwrap();
    assert_eq!(
        body["docs"][0]["_source"]["dashboard"]["title"],
        "Original dashboard"
    );
    assert_eq!(
        body["docs"][1]["_source"]["dashboard"]["title"],
        "Imported dashboard copy"
    );
    assert_eq!(
        body["docs"][1]["_source"]["references"],
        json!([{ "type": "index-pattern", "id": "orders-copy", "name": "kibanaSavedObjectMeta.searchSourceJSON.index" }])
    );
    assert_eq!(body["docs"][2]["found"], true);
}

#[tokio::test]
async fn dashboards_legacy_encoded_saved_object_ids_are_repaired_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mutation_log = temp.path().join("mutations.jsonl");
    let legacy_mutations = vec![
        Mutation::CreateIndex {
            name: ".kibana_1".to_string(),
            settings: json!({}),
            mappings: json!({}),
        },
        Mutation::PutAlias {
            index: ".kibana_1".to_string(),
            alias: ".kibana".to_string(),
            raw: json!({ "index": ".kibana_1", "alias": ".kibana" }),
        },
        Mutation::CreateDocument {
            index: ".kibana_1".to_string(),
            id: "index-pattern%3Aorders".to_string(),
            source: json!({
                "type": "index-pattern",
                "index-pattern": { "title": "orders", "timeFieldName": "created_at" },
                "references": []
            }),
        },
        Mutation::CreateDocument {
            index: ".kibana_1".to_string(),
            id: "dashboard%3Aorders-dashboard".to_string(),
            source: json!({
                "type": "dashboard",
                "dashboard": { "title": "Orders dashboard" },
                "references": [
                    { "type": "index-pattern", "id": "orders", "name": "kibanaSavedObjectMeta.searchSourceJSON.index" }
                ]
            }),
        },
    ];
    mutation_log::append_transaction_begin(&mutation_log, "legacy-encoded-ids", &legacy_mutations)
        .unwrap();
    mutation_log::append_transaction_commit(&mutation_log, "legacy-encoded-ids").unwrap();

    let state = durable_state(temp.path());
    let dashboard = call(
        &state,
        Method::GET,
        "/.kibana/_doc/dashboard%3Aorders-dashboard",
        Value::Null,
    )
    .await;
    assert_eq!(dashboard.status, 200);
    let body = dashboard.body.unwrap();
    assert_eq!(body["_id"], "dashboard:orders-dashboard");
    assert_eq!(body["_source"]["dashboard"]["title"], "Orders dashboard");
    assert_eq!(
        body["_source"]["references"][0]["id"], "orders",
        "repair must preserve saved-object references"
    );

    let reference_lookup = call(
        &state,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": ".kibana", "_id": "index-pattern:orders" },
                { "_index": ".kibana", "_id": "dashboard:orders-dashboard" }
            ]
        }),
    )
    .await;
    assert_eq!(reference_lookup.status, 200);
    let body = reference_lookup.body.unwrap();
    assert_eq!(body["docs"][0]["found"], true);
    assert_eq!(body["docs"][1]["found"], true);

    let legacy_lookup = call(
        &state,
        Method::POST,
        "/_mget",
        json!({ "docs": [{ "_index": ".kibana", "_id": "index-pattern%3Aorders" }] }),
    )
    .await;
    assert_eq!(legacy_lookup.status, 200);
    assert_eq!(legacy_lookup.body.unwrap()["docs"][0]["found"], false);

    let log = fs::read_to_string(&mutation_log).unwrap();
    assert!(log.contains("\"kind\":\"rename_document\""));

    drop(state);
    let replayed = durable_state(temp.path());
    let replayed_lookup = call(
        &replayed,
        Method::POST,
        "/_mget",
        json!({ "docs": [{ "_index": ".kibana", "_id": "dashboard:orders-dashboard" }] }),
    )
    .await;
    assert_eq!(replayed_lookup.status, 200);
    assert_eq!(replayed_lookup.body.unwrap()["docs"][0]["found"], true);
}

#[tokio::test]
async fn nodes_metadata_uses_configured_version_and_listener() {
    let config = Config {
        ephemeral: true,
        advertised_version: "3.6.1-test".to_string(),
        listen: "127.0.0.2:9300".parse().unwrap(),
        ..Default::default()
    };
    let state = AppState::new(config).unwrap();

    call(&state, Method::PUT, "/orders", json!({})).await;
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;

    let info = call(&state, Method::GET, "/_nodes", Value::Null).await;
    assert_eq!(info.status, 200);
    let body = info.body.unwrap();
    let node = body["nodes"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .expect("local node is present");
    assert_eq!(node["version"], "3.6.1-test");
    assert_eq!(node["ip"], "127.0.0.2");
    assert_eq!(node["http"]["publish_address"], "127.0.0.2:9300");

    let stats = call(&state, Method::GET, "/_nodes/stats", Value::Null).await;
    assert_eq!(stats.status, 200);
    assert_eq!(
        stats
            .headers
            .get("x-mainstack-search-api")
            .map(String::as_str),
        Some("nodes.stats")
    );
    assert_eq!(
        stats
            .headers
            .get("x-mainstack-search-tier")
            .map(String::as_str),
        Some("best_effort")
    );
    let body = stats.body.unwrap();
    let node = body["nodes"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .expect("local node stats are present");
    assert_eq!(node["version"], "3.6.1-test");
    assert_eq!(node["ip"], "127.0.0.2");
    assert_eq!(node["http"]["publish_address"], "127.0.0.2:9300");
    assert_eq!(node["indices"]["docs"]["count"], 1);

    let filtered_stats = call(
        &state,
        Method::GET,
        "/_nodes/stats?filter_path=nodes.*.indices.docs.count,nodes.*.http.publish_address",
        Value::Null,
    )
    .await;
    assert_eq!(filtered_stats.status, 200);
    assert_eq!(
        filtered_stats
            .headers
            .get("x-mainstack-search-tier")
            .map(String::as_str),
        Some("best_effort")
    );
    let body = filtered_stats.body.unwrap();
    assert!(body.get("cluster_name").is_none());
    let node = body["nodes"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .expect("local node stats are present");
    assert_eq!(node["indices"]["docs"]["count"], 1);
    assert_eq!(node["http"]["publish_address"], "127.0.0.2:9300");
    assert!(node.get("version").is_none());
    assert!(node["indices"]["docs"].get("deleted").is_none());
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
