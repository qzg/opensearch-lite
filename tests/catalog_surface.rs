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
