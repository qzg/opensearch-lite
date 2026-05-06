mod support;

use http::Method;
use mainstack_search::{server::AppState, Config};
use serde_json::{json, Value};
use std::fs;
use support::{call, durable_state};

#[tokio::test]
async fn snapshot_apis_fail_closed_in_ephemeral_mode_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: temp.path().to_path_buf(),
        ..Config::default()
    };
    config.ephemeral = true;
    let state = AppState::new(config).unwrap();

    let put_repo = call(
        &state,
        Method::PUT,
        "/_snapshot/local",
        json!({ "type": "fs" }),
    )
    .await;
    assert_eq!(put_repo.status, 501);
    assert_eq!(
        put_repo.body.unwrap()["error"]["type"],
        "mainstack_search_unsupported_api_exception"
    );

    let get_repo = call(&state, Method::GET, "/_snapshot", Value::Null).await;
    assert_eq!(get_repo.status, 501);
    assert!(!temp.path().join("repositories").exists());
}

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
async fn snapshot_restore_fails_closed_without_mutating_live_state() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_eq!(
        call(&state, Method::PUT, "/orders", json!({})).await.status,
        200
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/orders/_doc/1",
            json!({ "generation": "snapshot" }),
        )
        .await
        .status,
        201
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local",
            json!({ "type": "fs" })
        )
        .await
        .status,
        200
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local/snap-1",
            json!({ "indices": "orders" }),
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
            json!({ "generation": "live" }),
        )
        .await
        .status,
        200
    );

    let restore = call(
        &state,
        Method::POST,
        "/_snapshot/local/snap-1/_restore",
        json!({
            "indices": "orders",
            "rename_pattern": "orders",
            "rename_replacement": "restored-orders",
            "include_global_state": false
        }),
    )
    .await;
    assert_eq!(restore.status, 501);
    assert_eq!(
        restore.body.unwrap()["error"]["type"],
        "mainstack_search_unsupported_api_exception"
    );

    let live = call(&state, Method::GET, "/orders/_doc/1", Value::Null).await;
    assert_eq!(live.status, 200);
    assert_eq!(live.body.unwrap()["_source"]["generation"], "live");

    let renamed = call(&state, Method::GET, "/restored-orders/_doc/1", Value::Null).await;
    assert_eq!(renamed.status, 404);

    let snapshot = call(&state, Method::GET, "/_snapshot/local/snap-1", Value::Null).await;
    assert_eq!(snapshot.status, 200);
    assert_eq!(snapshot.body.unwrap()["snapshots"][0]["snapshot"], "snap-1");
}

#[tokio::test]
async fn snapshot_operation_tokens_in_name_slot_fail_closed_without_creating_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_eq!(
        call(&state, Method::PUT, "/orders", json!({})).await.status,
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
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local",
            json!({ "type": "fs" })
        )
        .await
        .status,
        200
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local/snap-1",
            json!({ "indices": "orders" }),
        )
        .await
        .status,
        200
    );

    for (method, path) in [
        (Method::POST, "/_snapshot/local/_restore"),
        (Method::POST, "/_snapshot/local/%5Frestore"),
        (Method::PUT, "/_snapshot/local/_clone"),
        (Method::PUT, "/_snapshot/local/%5Fclone"),
    ] {
        let response = call(&state, method, path, json!({ "indices": "orders" })).await;
        assert_eq!(response.status, 501, "{path}");
        assert_eq!(
            response.body.unwrap()["error"]["type"],
            "mainstack_search_unsupported_api_exception"
        );
    }

    let snapshots = call(&state, Method::GET, "/_snapshot/local/_all", Value::Null).await;
    assert_eq!(snapshots.status, 200);
    assert_eq!(
        snapshots.body.unwrap()["snapshots"][0]["snapshot"],
        "snap-1"
    );
}

#[tokio::test]
async fn snapshot_cleanup_preserves_blobs_referenced_by_remaining_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_eq!(
        call(&state, Method::PUT, "/orders", json!({})).await.status,
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
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local",
            json!({ "type": "fs" })
        )
        .await
        .status,
        200
    );
    for snapshot in ["snap-1", "snap-2"] {
        let response = call(
            &state,
            Method::PUT,
            &format!("/_snapshot/local/{snapshot}"),
            json!({ "indices": "orders" }),
        )
        .await;
        assert_eq!(response.status, 200);
    }

    let repo_dir = temp.path().join("repositories/local");
    assert_eq!(
        fs::read_dir(repo_dir.join("blobs"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len(),
        1
    );

    let delete_one = call(
        &state,
        Method::DELETE,
        "/_snapshot/local/snap-1",
        Value::Null,
    )
    .await;
    assert_eq!(delete_one.status, 200);

    let cleanup = call(
        &state,
        Method::POST,
        "/_snapshot/local/_cleanup",
        Value::Null,
    )
    .await;
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body.unwrap()["results"]["deleted_blobs"], 0);

    let remaining = call(&state, Method::GET, "/_snapshot/local/snap-2", Value::Null).await;
    assert_eq!(remaining.status, 200);
    assert_eq!(
        remaining.body.unwrap()["snapshots"][0]["snapshot"],
        "snap-2"
    );

    let delete_remaining = call(
        &state,
        Method::DELETE,
        "/_snapshot/local/snap-2",
        Value::Null,
    )
    .await;
    assert_eq!(delete_remaining.status, 200);
    let cleanup = call(
        &state,
        Method::POST,
        "/_snapshot/local/_cleanup",
        Value::Null,
    )
    .await;
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body.unwrap()["results"]["deleted_blobs"], 1);
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
async fn snapshot_reserved_names_are_selectors_not_creatable_names() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_eq!(
        call(&state, Method::PUT, "/orders", json!({})).await.status,
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
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local",
            json!({ "type": "fs" })
        )
        .await
        .status,
        200
    );
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_snapshot/local/snap-1",
            json!({ "indices": "orders" }),
        )
        .await
        .status,
        200
    );

    for repository in ["_all", "all"] {
        let response = call(
            &state,
            Method::PUT,
            &format!("/_snapshot/{repository}"),
            json!({ "type": "fs" }),
        )
        .await;
        assert_eq!(response.status, 400);
        assert_eq!(
            response.body.unwrap()["error"]["type"],
            "repository_exception"
        );
    }

    for snapshot in ["_all", "all", "_hidden"] {
        let response = call(
            &state,
            Method::PUT,
            &format!("/_snapshot/local/{snapshot}"),
            json!({ "indices": "orders" }),
        )
        .await;
        assert_eq!(response.status, 400);
        assert_eq!(
            response.body.unwrap()["error"]["type"],
            "invalid_snapshot_name_exception"
        );
    }

    for selector in ["_all", "all"] {
        let repositories = call(
            &state,
            Method::GET,
            &format!("/_snapshot/{selector}"),
            Value::Null,
        )
        .await;
        assert_eq!(repositories.status, 200);
        assert!(repositories.body.unwrap()["local"].is_object());

        let snapshots = call(
            &state,
            Method::GET,
            &format!("/_snapshot/local/{selector}"),
            Value::Null,
        )
        .await;
        assert_eq!(snapshots.status, 200);
        assert_eq!(
            snapshots.body.unwrap()["snapshots"][0]["snapshot"],
            "snap-1"
        );
    }

    for repository in ["_all", "all", "%5Fall", "%61ll", "local,_all"] {
        let response = call(
            &state,
            Method::DELETE,
            &format!("/_snapshot/{repository}"),
            Value::Null,
        )
        .await;
        assert_eq!(response.status, 400);
        assert_eq!(
            response.body.unwrap()["error"]["type"],
            "repository_exception"
        );

        let existing = call(&state, Method::GET, "/_snapshot/local", Value::Null).await;
        assert_eq!(existing.status, 200);
    }

    for snapshot in ["_all", "all", "%5Fall", "%61ll", "snap-1,_all"] {
        let response = call(
            &state,
            Method::DELETE,
            &format!("/_snapshot/local/{snapshot}"),
            Value::Null,
        )
        .await;
        assert_eq!(response.status, 400);
        assert_eq!(
            response.body.unwrap()["error"]["type"],
            "invalid_snapshot_name_exception"
        );

        let existing = call(&state, Method::GET, "/_snapshot/local/snap-1", Value::Null).await;
        assert_eq!(existing.status, 200);
        assert_eq!(existing.body.unwrap()["snapshots"][0]["snapshot"], "snap-1");
    }
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
