mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, ephemeral_state, ndjson_call};

#[tokio::test]
async fn mget_reads_path_index_ids_and_mixed_docs() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/customers/_doc/c1",
        json!({ "name": "Ada" }),
    )
    .await;

    let path_ids = call(
        &state,
        Method::POST,
        "/orders/_mget",
        json!({ "ids": ["1", "missing"] }),
    )
    .await;
    assert_eq!(path_ids.status, 200);
    let body = path_ids.body.unwrap();
    assert_eq!(body["docs"][0]["found"], true);
    assert_eq!(body["docs"][1]["found"], false);

    let mixed = call(
        &state,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": "orders", "_id": "1" },
                { "_index": "customers", "_id": "c1" }
            ]
        }),
    )
    .await;
    assert_eq!(mixed.status, 200);
    let body = mixed.body.unwrap();
    assert_eq!(body["docs"][0]["_source"]["status"], "paid");
    assert_eq!(body["docs"][1]["_source"]["name"], "Ada");
}

#[tokio::test]
async fn mget_applies_request_and_item_source_filtering() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({
            "status": "paid",
            "total": 42,
            "internal": "hidden",
            "details": { "city": "Austin", "secret": "hidden" }
        }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/orders/_doc/2",
        json!({ "status": "open", "total": 12, "internal": "hidden" }),
    )
    .await;

    let filtered = call(
        &state,
        Method::POST,
        "/orders/_mget",
        json!({ "ids": ["1"], "_source": ["status"] }),
    )
    .await;
    assert_eq!(filtered.status, 200);
    assert_eq!(
        filtered.body.unwrap()["docs"][0]["_source"],
        json!({ "status": "paid" })
    );

    let mixed = call(
        &state,
        Method::POST,
        "/_mget",
        json!({
            "docs": [
                { "_index": "orders", "_id": "1", "_source": false },
                { "_index": "orders", "_id": "2", "_source": ["total"] }
            ]
        }),
    )
    .await;
    let body = mixed.body.unwrap();
    assert!(body["docs"][0].get("_source").is_none());
    assert_eq!(body["docs"][1]["_source"], json!({ "total": 12 }));

    let nested = call(
        &state,
        Method::POST,
        "/orders/_mget?_source_includes=details.city,total&_source_excludes=details.secret",
        json!({ "docs": [{ "_id": "1" }] }),
    )
    .await;
    assert_eq!(nested.status, 200);
    assert_eq!(
        nested.body.unwrap()["docs"][0]["_source"],
        json!({ "details": { "city": "Austin" }, "total": 42 })
    );
}

#[tokio::test]
async fn get_source_returns_raw_source_filters_and_head_existence() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders/_doc/1",
        json!({
            "status": "paid",
            "total": 42,
            "details": { "city": "Austin", "secret": "hidden" }
        }),
    )
    .await;

    let source = call(&state, Method::GET, "/orders/_source/1", Value::Null).await;
    assert_eq!(source.status, 200);
    assert_eq!(source.body.unwrap()["status"], "paid");

    let filtered = call(
        &state,
        Method::GET,
        "/orders/_source/1?_source_includes=details.city,total&_source_excludes=details.secret",
        Value::Null,
    )
    .await;
    assert_eq!(filtered.status, 200);
    assert_eq!(
        filtered.body.unwrap(),
        json!({ "details": { "city": "Austin" }, "total": 42 })
    );

    assert_eq!(
        call(&state, Method::HEAD, "/orders/_source/1", Value::Null)
            .await
            .status,
        200
    );
    assert_eq!(
        call(&state, Method::HEAD, "/orders/_source/missing", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn get_source_respects_disabled_source_mappings() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/hidden",
        json!({
            "mappings": {
                "_source": { "enabled": false },
                "properties": { "status": { "type": "keyword" } }
            }
        }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/hidden/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;

    let source = call(&state, Method::GET, "/hidden/_source/1", Value::Null).await;
    assert_eq!(source.status, 404);
    assert_eq!(
        call(&state, Method::HEAD, "/hidden/_source/1", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn update_supports_explicit_upsert_and_filtered_get_response() {
    let state = ephemeral_state();

    let created = call(
        &state,
        Method::POST,
        "/orders/_update/1?_source=status",
        json!({
            "doc": { "status": "paid", "total": 42 },
            "upsert": { "status": "open", "internal": "hidden" }
        }),
    )
    .await;
    assert_eq!(created.status, 201);
    let body = created.body.unwrap();
    assert_eq!(body["result"], "created");
    assert_eq!(body["get"]["_source"], json!({ "status": "open" }));

    let updated = call(
        &state,
        Method::POST,
        "/orders/_update/1?_source=status,total",
        json!({
            "doc": { "status": "paid", "total": 42 },
            "upsert": { "status": "ignored" }
        }),
    )
    .await;
    assert_eq!(updated.status, 200);
    let body = updated.body.unwrap();
    assert_eq!(body["result"], "updated");
    assert_eq!(
        body["get"]["_source"],
        json!({ "status": "paid", "total": 42 })
    );

    let stored = call(&state, Method::GET, "/orders/_doc/1", Value::Null).await;
    assert_eq!(stored.body.unwrap()["_source"]["internal"], "hidden");

    let hidden_get = call(
        &state,
        Method::POST,
        "/orders/_update/1",
        json!({
            "doc": { "status": "refunded" },
            "_source": false
        }),
    )
    .await;
    assert_eq!(hidden_get.status, 200);
    assert!(hidden_get.body.unwrap().get("get").is_none());
}

#[tokio::test]
async fn update_rejects_malformed_upsert_bodies_without_mutation() {
    let state = ephemeral_state();

    let upsert_only = call(
        &state,
        Method::POST,
        "/bad-upsert/_update/1",
        json!({ "upsert": { "status": "created" } }),
    )
    .await;
    assert_eq!(upsert_only.status, 400);
    assert_eq!(
        call(&state, Method::HEAD, "/bad-upsert", Value::Null)
            .await
            .status,
        404
    );

    let misspelled = call(
        &state,
        Method::POST,
        "/bad-upsert/_update/1",
        json!({
            "dc": { "status": "created" },
            "upsert": { "status": "created" }
        }),
    )
    .await;
    assert_eq!(misspelled.status, 400);
    assert_eq!(
        call(&state, Method::HEAD, "/bad-upsert", Value::Null)
            .await
            .status,
        404
    );

    let scalar_doc = call(
        &state,
        Method::POST,
        "/bad-upsert/_update/1",
        json!({
            "doc": "not an object",
            "upsert": { "status": "created" }
        }),
    )
    .await;
    assert_eq!(scalar_doc.status, 400);
}

#[tokio::test]
async fn bulk_update_supports_explicit_upsert_and_rejects_malformed_upsert() {
    let state = ephemeral_state();
    let response = ndjson_call(
        &state,
        Method::POST,
        "/_bulk",
        r#"{"update":{"_index":"orders","_id":"1"}}
{"doc":{"status":"ignored"},"upsert":{"status":"from-upsert","internal":"hidden"}}
{"update":{"_index":"bad-upsert","_id":"1"}}
{"dc":{"status":"created"},"upsert":{"status":"created"}}
"#,
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["errors"], true);
    assert_eq!(body["items"][0]["update"]["status"], 201);
    assert_eq!(body["items"][0]["update"]["result"], "created");
    assert_eq!(body["items"][1]["update"]["status"], 400);

    let stored = call(&state, Method::GET, "/orders/_doc/1", Value::Null).await;
    assert_eq!(stored.body.unwrap()["_source"]["status"], "from-upsert");
    assert_eq!(
        call(&state, Method::HEAD, "/bad-upsert", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn existing_index_missing_document_returns_found_false() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/orders", json!({})).await;

    let response = call(&state, Method::GET, "/orders/_doc/missing", Value::Null).await;

    assert_eq!(response.status, 404);
    assert_eq!(response.body.unwrap()["found"], false);
}
