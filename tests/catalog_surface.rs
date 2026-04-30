mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, ephemeral_state};

#[tokio::test]
async fn mapping_and_settings_round_trip() {
    let state = ephemeral_state();
    assert_eq!(
        call(&state, Method::PUT, "/catalog", json!({}))
            .await
            .status,
        200
    );

    let mapping = call(
        &state,
        Method::PUT,
        "/catalog/_mapping",
        json!({
            "properties": {
                "status": { "type": "keyword" }
            }
        }),
    )
    .await;
    assert_eq!(mapping.status, 200);

    let settings = call(
        &state,
        Method::PUT,
        "/catalog/_settings",
        json!({ "settings": { "index": { "refresh_interval": "-1" } } }),
    )
    .await;
    assert_eq!(settings.status, 200);

    let got_mapping = call(&state, Method::GET, "/catalog/_mapping", Value::Null).await;
    assert_eq!(
        got_mapping.body.unwrap()["catalog"]["mappings"]["properties"]["status"]["type"],
        "keyword"
    );

    let got_settings = call(&state, Method::GET, "/catalog/_settings", Value::Null).await;
    assert_eq!(
        got_settings.body.unwrap()["catalog"]["settings"]["index"]["refresh_interval"],
        "-1"
    );
}

#[tokio::test]
async fn field_mapping_stats_and_cat_indices_are_opensearch_shaped() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/catalog",
        json!({
            "settings": { "index": { "number_of_shards": 2, "number_of_replicas": 1 } },
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" },
                    "body": { "type": "text" },
                    "title": { "type": "text" },
                    "obj": {
                        "properties": {
                            "i_one": { "type": "keyword" },
                            "i_two": { "type": "keyword" }
                        }
                    }
                }
            }
        }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/catalog/_doc/1",
        json!({ "status": "open", "body": "hello" }),
    )
    .await;

    let field_mapping = call(
        &state,
        Method::GET,
        "/catalog/_mapping/field/body?include_defaults=true",
        Value::Null,
    )
    .await;
    assert_eq!(field_mapping.status, 200);
    let body = field_mapping.body.unwrap();
    assert_eq!(
        body["catalog"]["mappings"]["body"]["mapping"]["body"]["type"],
        "text"
    );
    assert_eq!(
        body["catalog"]["mappings"]["body"]["mapping"]["body"]["analyzer"],
        "default"
    );

    let wildcard_mapping = call(
        &state,
        Method::GET,
        "/catalog/_mapping/field/*tus,obj.i_*",
        Value::Null,
    )
    .await;
    assert_eq!(wildcard_mapping.status, 200);
    let body = wildcard_mapping.body.unwrap();
    assert!(body["catalog"]["mappings"]["status"].is_object());
    assert!(body["catalog"]["mappings"]["obj.i_one"].is_object());
    assert!(body["catalog"]["mappings"]["obj.i_two"].is_object());

    let global_mapping = call(&state, Method::GET, "/_mapping/field/status", Value::Null).await;
    assert_eq!(global_mapping.status, 200);
    assert!(global_mapping.body.unwrap()["catalog"]["mappings"]["status"].is_object());

    let stats = call(&state, Method::GET, "/catalog/_stats", Value::Null).await;
    assert_eq!(stats.status, 200);
    let body = stats.body.unwrap();
    assert_eq!(body["_shards"]["total"], 4);
    assert_eq!(body["indices"]["catalog"]["primaries"]["docs"]["count"], 1);
    assert!(
        body["indices"]["catalog"]["primaries"]["store"]["size_in_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );

    let docs_stats = call(&state, Method::GET, "/catalog/_stats/docs", Value::Null).await;
    assert_eq!(docs_stats.status, 200);
    let body = docs_stats.body.unwrap();
    assert_eq!(body["indices"]["catalog"]["primaries"]["docs"]["count"], 1);
    assert!(body["indices"]["catalog"]["primaries"]
        .get("store")
        .is_none());

    let bad_metric = call(&state, Method::GET, "/catalog/_stats/fieldata", Value::Null).await;
    assert_eq!(bad_metric.status, 400);
    assert_eq!(
        bad_metric.body.unwrap()["error"]["type"],
        "illegal_argument_exception"
    );

    let cat = call(
        &state,
        Method::GET,
        "/_cat/indices?format=json",
        Value::Null,
    )
    .await;
    assert_eq!(cat.status, 200);
    let body = cat.body.unwrap();
    assert_eq!(body[0]["pri"], "2");
    assert_eq!(body[0]["rep"], "1");
    assert_ne!(body[0]["store.size"], "0b");

    call(
        &state,
        Method::PUT,
        "/catalog-extra",
        json!({ "mappings": { "properties": { "status": { "type": "keyword" } } } }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/catalog-extra/_doc/1",
        json!({ "status": "open" }),
    )
    .await;

    let pattern_stats = call(&state, Method::GET, "/cat*/_stats/docs", Value::Null).await;
    assert_eq!(pattern_stats.status, 200);
    assert_eq!(
        pattern_stats.body.unwrap()["indices"]
            .as_object()
            .unwrap()
            .len(),
        2
    );

    let filtered_cat = call(
        &state,
        Method::GET,
        "/_cat/indices/catalog-extra?format=json",
        Value::Null,
    )
    .await;
    assert_eq!(filtered_cat.status, 200);
    let body = filtered_cat.body.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["index"], "catalog-extra");
}

#[tokio::test]
async fn aliases_endpoint_adds_removes_and_lists_aliases() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/catalog", json!({})).await;

    let add = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "add": { "index": "catalog", "alias": "catalog-read" } }
            ]
        }),
    )
    .await;
    assert_eq!(add.status, 200);

    let list = call(&state, Method::GET, "/_aliases", Value::Null).await;
    assert!(list.body.unwrap()["catalog"]["aliases"]["catalog-read"].is_object());

    let remove = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "remove": { "index": "catalog", "alias": "catalog-read" } }
            ]
        }),
    )
    .await;
    assert_eq!(remove.status, 200);

    let missing = call(&state, Method::GET, "/_alias/catalog-read", Value::Null).await;
    assert_eq!(missing.status, 404);
}

#[tokio::test]
async fn registry_apis_round_trip_readable_json_objects() {
    let state = ephemeral_state();

    let component = call(
        &state,
        Method::PUT,
        "/_component_template/common-fields",
        json!({
            "template": {
                "mappings": {
                    "properties": {
                        "status": { "type": "keyword" }
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(component.status, 200);
    let component = call(
        &state,
        Method::GET,
        "/_component_template/common-fields",
        Value::Null,
    )
    .await;
    assert_eq!(component.status, 200);
    assert_eq!(
        component.body.unwrap()["component_templates"][0]["name"],
        "common-fields"
    );

    let ingest = call(
        &state,
        Method::PUT,
        "/_ingest/pipeline/normalize",
        json!({
            "processors": [
                { "set": { "field": "status", "value": "new" } }
            ]
        }),
    )
    .await;
    assert_eq!(ingest.status, 200);
    let ingest = call(
        &state,
        Method::GET,
        "/_ingest/pipeline/normalize",
        Value::Null,
    )
    .await;
    assert_eq!(ingest.status, 200);
    assert_eq!(
        ingest.body.unwrap()["normalize"]["processors"][0]["set"]["field"],
        "status"
    );

    let search_pipeline = call(
        &state,
        Method::PUT,
        "/_search/pipeline/default-search",
        json!({
            "request_processors": [],
            "response_processors": []
        }),
    )
    .await;
    assert_eq!(search_pipeline.status, 200);
    let search_pipeline = call(
        &state,
        Method::GET,
        "/_search/pipeline/default-search",
        Value::Null,
    )
    .await;
    assert_eq!(search_pipeline.status, 200);
    assert!(search_pipeline.body.unwrap()["default-search"]["request_processors"].is_array());

    let script = call(
        &state,
        Method::PUT,
        "/_scripts/calc-score",
        json!({
            "script": {
                "lang": "painless",
                "source": "return params.score"
            }
        }),
    )
    .await;
    assert_eq!(script.status, 200);
    let script = call(&state, Method::GET, "/_scripts/calc-score", Value::Null).await;
    assert_eq!(script.status, 200);
    assert_eq!(script.body.unwrap()["script"]["lang"], "painless");
}

#[tokio::test]
async fn legacy_templates_are_separate_from_composable_templates() {
    let state = ephemeral_state();

    let composable = call(
        &state,
        Method::PUT,
        "/_index_template/shared",
        json!({
            "index_patterns": ["shared-*"],
            "template": { "settings": {} }
        }),
    )
    .await;
    assert_eq!(composable.status, 200);

    let missing_legacy_delete =
        call(&state, Method::DELETE, "/_template/shared", Value::Null).await;
    assert_eq!(missing_legacy_delete.status, 404);

    let composable_after = call(&state, Method::GET, "/_index_template/shared", Value::Null).await;
    assert_eq!(composable_after.status, 200);
    assert_eq!(
        composable_after.body.unwrap()["index_templates"][0]["name"],
        "shared"
    );
}
