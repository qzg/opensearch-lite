mod support;

use http::Method;
use opensearch_lite::{server::AppState, Config};
use serde_json::{json, Value};
use support::{call, durable_state, ephemeral_state};
use tokio::time::{sleep, Duration};

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
async fn expired_pits_are_purged_from_list_search_and_budget() {
    let state = ephemeral_state();
    seed_orders(&state).await;

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1ms",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);
    let pit_id = create.body.unwrap()["pit_id"].as_str().unwrap().to_string();

    sleep(Duration::from_millis(10)).await;

    let list = call(
        &state,
        Method::GET,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body.unwrap()["pits"], json!([]));

    let expired_search = call(
        &state,
        Method::POST,
        "/_search",
        json!({ "pit": { "id": pit_id }, "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(expired_search.status, 404);
    assert_eq!(
        expired_search.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );

    let recreated = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(recreated.status, 200);
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
async fn pit_search_uses_frozen_view_and_refreshes_keep_alive() {
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
            "pit": { "id": pit_id, "keep_alive": "10m" },
            "query": { "match_all": {} }
        }),
    )
    .await;
    assert_eq!(search.status, 200);
    let body = search.body.unwrap();
    assert_eq!(body["pit_id"], pit_id);
    assert_eq!(body["hits"]["total"]["value"], 1);
    assert_eq!(body["hits"]["hits"][0]["_id"], "1");

    let live = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({ "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(live.status, 200);
    assert_eq!(live.body.unwrap()["hits"]["total"]["value"], 2);

    let list = call(
        &state,
        Method::GET,
        "/_search/point_in_time/_all",
        Value::Null,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body.unwrap()["pits"][0]["keep_alive"], 600_000);
}

#[tokio::test]
async fn pit_search_after_remains_stable_after_live_writes() {
    let state = ephemeral_state();
    seed_orders(&state).await;
    call(
        &state,
        Method::PUT,
        "/orders/_doc/2",
        json!({ "status": "paid" }),
    )
    .await;

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);
    let pit_id = create.body.unwrap()["pit_id"].as_str().unwrap().to_string();

    let first = call(
        &state,
        Method::POST,
        "/_search",
        json!({
            "pit": { "id": pit_id },
            "size": 1,
            "query": { "match_all": {} },
            "sort": [{ "status": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let first = first.body.unwrap();
    assert_eq!(first["hits"]["hits"][0]["_id"], "1");
    assert_eq!(
        first["hits"]["hits"][0]["sort"],
        json!(["paid", "orders\u{1f}1"])
    );
    let after = first["hits"]["hits"][0]["sort"].clone();

    call(
        &state,
        Method::PUT,
        "/orders/_doc/3",
        json!({ "status": "pending" }),
    )
    .await;

    let second = call(
        &state,
        Method::POST,
        "/_search",
        json!({
            "pit": { "id": pit_id },
            "size": 10,
            "query": { "match_all": {} },
            "sort": [{ "status": { "order": "asc" } }],
            "search_after": after
        }),
    )
    .await;
    assert_eq!(second.status, 200);
    let body = second.body.unwrap();
    assert_eq!(body["hits"]["total"]["value"], 2);
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"]["hits"][0]["_id"], "2");
}

#[tokio::test]
async fn pit_search_rejects_missing_conflicting_and_malformed_cursor_shapes() {
    let state = ephemeral_state();
    seed_orders(&state).await;

    let missing = call(
        &state,
        Method::POST,
        "/_search",
        json!({ "pit": { "id": "missing" }, "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );

    let create = call(
        &state,
        Method::POST,
        "/orders/_search/point_in_time?keep_alive=1m",
        Value::Null,
    )
    .await;
    assert_eq!(create.status, 200);
    let pit_id = create.body.unwrap()["pit_id"].as_str().unwrap().to_string();

    let path_index = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({ "pit": { "id": pit_id }, "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(path_index.status, 400);

    let malformed_search_after = call(
        &state,
        Method::POST,
        "/_search",
        json!({
            "pit": { "id": pit_id },
            "search_after": [1],
            "sort": [{ "_id": { "order": "asc" } }],
            "query": { "match_all": {} }
        }),
    )
    .await;
    assert_eq!(malformed_search_after.status, 400);
    assert_eq!(
        malformed_search_after.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
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
