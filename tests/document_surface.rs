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
async fn existing_index_missing_document_returns_found_false() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/orders", json!({})).await;

    let response = call(&state, Method::GET, "/orders/_doc/missing", Value::Null).await;

    assert_eq!(response.status, 404);
    assert_eq!(response.body.unwrap()["found"], false);
}
