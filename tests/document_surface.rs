mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, ephemeral_state};

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
async fn existing_index_missing_document_returns_found_false() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/orders", json!({})).await;

    let response = call(&state, Method::GET, "/orders/_doc/missing", Value::Null).await;

    assert_eq!(response.status, 404);
    assert_eq!(response.body.unwrap()["found"], false);
}
