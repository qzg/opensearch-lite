mod support;

use http::Method;
use opensearch_lite::{server::AppState, Config};
use serde_json::{json, Value};
use support::{call, durable_state, ephemeral_state};

#[tokio::test]
async fn pit_lifecycle_is_process_local_and_opensearch_shaped() {
    let state = ephemeral_state();
    seed_orders(&state).await;

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=23h",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);
    let body = create.body.unwrap();
    let pit_id = body["pit_id"].as_str().unwrap().to_string();
    assert!(pit_id.starts_with("opensearch-lite-pit:"));
    assert_eq!(body["_shards"]["failed"], 0);
    assert_eq!(body["_shards"]["total"], 1);
    assert!(body["creation_time"].as_u64().is_some());

    let list = call(
        &state,
        Method::GET,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(list.status, 200);
    let body = list.body.unwrap();
    assert_eq!(body["pits"][0]["pit_id"], pit_id);
    assert_eq!(body["pits"][0]["keep_alive"], 82_800_000);

    let delete = call(
        &state,
        Method::DELETE,
        "/_search/point_in_time",
        json!({ "pit_id": [pit_id] }),
    )
    .await;
    assert_eq!(delete.status, 200);
    let body = delete.body.unwrap();
    assert_eq!(body["pits"][0]["successful"], true);
    assert!(body["pits"][0]["pit_id"]
        .as_str()
        .unwrap()
        .starts_with("opensearch-lite-pit:"));

    let empty = call(
        &state,
        Method::GET,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(empty.status, 200);
    assert_eq!(empty.body.unwrap()["pits"], json!([]));
}

#[tokio::test]
async fn delete_all_pits_removes_only_current_process_contexts() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());
    seed_orders(&state).await;

    for _ in 0..2 {
        let create = call(
            &state,
            Method::POST,
            "/orders/_search/point_in_time?keep_alive=1m",
            Value::Null,
        )
        .await;
        assert_eq!(create.status, 200);
    }

    let delete_all = call(
        &state,
        Method::DELETE,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(delete_all.status, 200);
    assert_eq!(
        delete_all.body.unwrap()["pits"].as_array().unwrap().len(),
        2
    );

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);

    drop(state);
    let restarted = durable_state(temp.path());
    let list = call(
        &restarted,
        Method::GET,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body.unwrap()["pits"], json!([]));
}

#[tokio::test]
async fn pit_create_and_delete_validate_inputs() {
    let state = ephemeral_state();
    seed_orders(&state).await;

    let missing_keep_alive = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time",
        Value::Null,
    )
    .await;
    assert_eq!(missing_keep_alive.status, 400);

    let invalid_keep_alive = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=forever",
        Value::Null,
    )
    .await;
    assert_eq!(invalid_keep_alive.status, 400);

    let missing_index = call(
        &state,
        Method::POST,
        "/missing/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(missing_index.status, 404);
    assert_eq!(
        missing_index.body.unwrap()["error"]["type"],
        "index_not_found_exception"
    );

    let missing_pit_id = call(
        &state,
        Method::DELETE,
        "/_search/point_in_time",
        Value::Null,
    )
    .await;
    assert_eq!(missing_pit_id.status, 400);
}

#[tokio::test]
async fn pit_search_fails_closed_until_frozen_search_is_implemented() {
    let state = ephemeral_state();
    seed_orders(&state).await;

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);
    let pit_id = create.body.unwrap()["pit_id"].as_str().unwrap().to_string();

    call(
        &state,
        Method::PUT,
        "/orders/_doc/2",
        json!({ "status": "pending" }),
    )
    .await;

    let search = call(
        &state,
        Method::POST,
        "/_search",
        json!({
            "pit": { "id": pit_id },
            "query": { "match_all": {} }
        }),
    )
    .await;
    assert_eq!(search.status, 501);
    assert_eq!(
        search.body.unwrap()["error"]["type"],
        "opensearch_lite_unsupported_api_exception"
    );
}

#[tokio::test]
async fn pit_creation_respects_runtime_memory_budget() {
    let config = Config {
        ephemeral: true,
        memory_limit_bytes: 1,
        max_body_bytes: 1,
        ..Default::default()
    };
    let state = AppState::new(config).unwrap();

    let create = call(
        &state,
        Method::POST,
        "/_all/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 429);
    assert_eq!(
        create.body.unwrap()["error"]["type"],
        "resource_limit_exception"
    );
}

async fn seed_orders(state: &AppState) {
    assert_eq!(
        call(state, Method::PUT, "/orders", json!({})).await.status,
        200
    );
    assert_eq!(
        call(
            state,
            Method::PUT,
            "/orders/_doc/1",
            json!({ "status": "paid" }),
        )
        .await
        .status,
        201
    );
}
