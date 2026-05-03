mod support;

use http::Method;
use serde_json::{json, Value};
use std::fs;
use support::{call, durable_state};

#[tokio::test]
async fn snapshot_repository_catalog_and_snapshots_are_restart_safe() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/orders",
            json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": { "properties": { "status": { "type": "keyword" } } }
            }),
        )
        .await
        .status,
        200
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/orders/_doc/1",
            json!({ "status": "paid" }),
        )
        .await
        .status,
        201
    );

    let put_repo = call(
        &state,
        Method::PUT,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": "snapshots/local" }
        }),
    )
    .await;
    assert_eq!(put_repo.status, 200);
    assert_eq!(put_repo.body.unwrap()["acknowledged"], true);

    let repo_dir = temp.path().join("repositories/local");
    assert!(repo_dir.join("index.latest").exists());
    assert!(repo_dir.join("index-000001.json").exists());

    let get_repo = call(&state, Method::GET, "/_snapshot", Value::Null).await;
    assert_eq!(get_repo.status, 200);
    let body = get_repo.body.unwrap();
    assert_eq!(body["local"]["type"], "fs");
    assert_eq!(body["local"]["settings"]["location"], "snapshots/local");

    let verify = call(
        &state,
        Method::POST,
        "/_snapshot/local/_verify",
        Value::Null,
    )
    .await;
    assert_eq!(verify.status, 200);
    assert!(verify.body.unwrap()["nodes"].is_object());

    let create_snapshot = call(
        &state,
        Method::PUT,
        "/_snapshot/local/snap-1?wait_for_completion=true",
        json!({ "indices": "orders" }),
    )
    .await;
    assert_eq!(create_snapshot.status, 200);
    let body = create_snapshot.body.unwrap();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["snapshot"]["snapshot"], "snap-1");
    assert_eq!(body["snapshot"]["repository"], "local");
    assert_eq!(body["snapshot"]["state"], "SUCCESS");
    assert_eq!(body["snapshot"]["indices"], json!(["orders"]));
    assert_eq!(body["snapshot"]["shards"]["total"], 2);

    let blob_entries = fs::read_dir(repo_dir.join("blobs"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(blob_entries.len(), 1);

    drop(state);
    let restarted = durable_state(temp.path());
    let get_snapshot = call(
        &restarted,
        Method::GET,
        "/_snapshot/local/snap-1",
        Value::Null,
    )
    .await;
    assert_eq!(get_snapshot.status, 200);
    let body = get_snapshot.body.unwrap();
    assert_eq!(body["snapshots"][0]["snapshot"], "snap-1");
    assert_eq!(body["snapshots"][0]["indices"], json!(["orders"]));

    let delete_snapshot = call(
        &restarted,
        Method::DELETE,
        "/_snapshot/local/snap-1",
        Value::Null,
    )
    .await;
    assert_eq!(delete_snapshot.status, 200);
    assert_eq!(delete_snapshot.body.unwrap()["acknowledged"], true);

    let empty_list = call(
        &restarted,
        Method::GET,
        "/_snapshot/local/_all",
        Value::Null,
    )
    .await;
    assert_eq!(empty_list.status, 200);
    assert_eq!(empty_list.body.unwrap()["snapshots"], json!([]));

    let cleanup = call(
        &restarted,
        Method::POST,
        "/_snapshot/local/_cleanup",
        Value::Null,
    )
    .await;
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body.unwrap()["results"]["deleted_blobs"], 1);

    let delete_repo = call(&restarted, Method::DELETE, "/_snapshot/local", Value::Null).await;
    assert_eq!(delete_repo.status, 200);
    assert_eq!(delete_repo.body.unwrap()["acknowledged"], true);
    assert!(!repo_dir.exists());
}

#[tokio::test]
async fn snapshot_repositories_reject_unsafe_names_types_and_locations() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    let bad_name = call(
        &state,
        Method::PUT,
        "/_snapshot/bad%2Fname",
        json!({ "type": "fs" }),
    )
    .await;
    assert_eq!(bad_name.status, 400);
    assert_eq!(
        bad_name.body.unwrap()["error"]["type"],
        "repository_exception"
    );

    let unsupported_type = call(
        &state,
        Method::PUT,
        "/_snapshot/remote",
        json!({ "type": "s3" }),
    )
    .await;
    assert_eq!(unsupported_type.status, 400);
    assert_eq!(
        unsupported_type.body.unwrap()["error"]["type"],
        "repository_exception"
    );

    let unsafe_location = call(
        &state,
        Method::PUT,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": "../outside" }
        }),
    )
    .await;
    assert_eq!(unsafe_location.status, 400);

    assert!(!temp.path().join("repositories").exists());
    assert!(!temp.path().join("outside").exists());
}

#[tokio::test]
async fn snapshot_create_in_missing_repository_returns_repository_missing() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    let response = call(
        &state,
        Method::PUT,
        "/_snapshot/missing/snap-1",
        json!({ "indices": "_all" }),
    )
    .await;
    assert_eq!(response.status, 404);
    assert_eq!(
        response.body.unwrap()["error"]["type"],
        "repository_missing_exception"
    );
}
