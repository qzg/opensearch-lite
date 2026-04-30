#![allow(clippy::field_reassign_with_default)]

mod support;

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use http::Method;
use opensearch_lite::{
    server::AppState,
    storage::{mutation_log, Database},
    Config,
};
use serde_json::{json, Value};

use support::{call, durable_state};

#[tokio::test]
async fn durable_files_are_directly_readable_by_a_coding_agent() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());

    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-durable",
            json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "keyword" },
                        "status": { "type": "keyword" },
                        "score": { "type": "long" }
                    }
                }
            }),
        )
        .await,
        200,
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-durable/_doc/doc-1",
            json!({
                "title": "Synthetic disk fixture",
                "status": "new",
                "score": 7
            }),
        )
        .await,
        201,
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-durable/_doc/doc-2",
            json!({
                "title": "Synthetic delete fixture",
                "status": "stale",
                "score": 2
            }),
        )
        .await,
        201,
    );
    assert_status(
        call(
            &state,
            Method::POST,
            "/agent-durable/_update/doc-1",
            json!({
                "doc": {
                    "status": "updated",
                    "score": 11
                }
            }),
        )
        .await,
        200,
    );
    assert_status(
        call(
            &state,
            Method::DELETE,
            "/agent-durable/_doc/doc-2",
            Value::Null,
        )
        .await,
        200,
    );
    drop(state);

    let records = read_jsonl(&temp.path().join("mutations.jsonl"));
    let begin_records = records
        .iter()
        .filter(|record| record["transaction"] == "begin")
        .collect::<Vec<_>>();
    let commit_ids = records
        .iter()
        .filter(|record| record["transaction"] == "commit")
        .filter_map(|record| record["id"].as_str())
        .collect::<BTreeSet<_>>();

    assert!(!begin_records.is_empty());
    assert_eq!(begin_records.len(), commit_ids.len());
    for begin in &begin_records {
        let transaction_id = begin["id"]
            .as_str()
            .expect("transaction begin record exposes an id");
        assert!(
            commit_ids.contains(transaction_id),
            "transaction begin id should have a matching commit"
        );
    }

    let mutations = begin_records
        .iter()
        .flat_map(|record| record["mutations"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    let kinds = mutations
        .iter()
        .filter_map(|mutation| mutation["kind"].as_str())
        .collect::<BTreeSet<_>>();

    assert!(kinds.contains("create_index"));
    assert!(kinds.contains("index_document"));
    assert!(kinds.contains("update_document"));
    assert!(kinds.contains("delete_document"));

    assert!(mutations.iter().any(|mutation| {
        mutation["kind"] == "index_document"
            && mutation["index"] == "agent-durable"
            && mutation["id"] == "doc-1"
            && mutation["source"]["title"] == "Synthetic disk fixture"
    }));
    assert!(mutations.iter().any(|mutation| {
        mutation["kind"] == "update_document"
            && mutation["index"] == "agent-durable"
            && mutation["id"] == "doc-1"
            && mutation["doc"]["status"] == "updated"
    }));
    assert!(mutations.iter().any(|mutation| {
        mutation["kind"] == "delete_document"
            && mutation["index"] == "agent-durable"
            && mutation["id"] == "doc-2"
    }));

    assert!(
        !temp.path().join("snapshot.json").exists(),
        "default durable writes should append to JSONL without rewriting a full snapshot below the dirty threshold"
    );
}

#[tokio::test]
async fn snapshot_metadata_is_directly_readable_after_dirty_threshold_flush() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.snapshot_write_threshold = 2;
    let state = AppState::new(config.clone()).unwrap();

    assert_status(
        call(&state, Method::PUT, "/agent-snapshot", json!({})).await,
        200,
    );
    assert!(
        !temp.path().join("snapshot.json").exists(),
        "first dirty write should remain append-only below the configured threshold"
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-snapshot/_doc/doc-1",
            json!({
                "title": "Synthetic disk fixture",
                "status": "new"
            }),
        )
        .await,
        201,
    );
    drop(state);

    let metadata = read_json(&temp.path().join("snapshot.meta.json"));
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["generation"], 1);
    assert_eq!(
        metadata["snapshot_file"],
        "snapshot.00000000000000000001.json"
    );
    assert!(temp
        .path()
        .join(metadata["snapshot_file"].as_str().unwrap())
        .exists());
    assert_eq!(metadata["index_count"], 1);
    assert_eq!(metadata["document_count"], 1);
    assert_eq!(metadata["indexes"][0]["name"], "agent-snapshot");
    assert_eq!(metadata["indexes"][0]["document_count"], 1);
    assert_eq!(metadata["log_compacted"], true);

    let snapshot = read_json(&temp.path().join("snapshot.json"));
    let documents = &snapshot["indexes"]["agent-snapshot"]["documents"];
    assert_eq!(
        documents["doc-1"]["source"]["title"],
        "Synthetic disk fixture"
    );
    assert_eq!(documents["doc-1"]["source"]["status"], "new");

    let active_records = read_jsonl(&temp.path().join("mutations.jsonl"));
    assert_eq!(active_records.len(), 1);
    assert_eq!(
        active_records[0]["transaction"], "compacted_after",
        "successful snapshot compaction should leave only a high-water marker when there were no later writes"
    );

    let replayed = AppState::new(config).unwrap();
    let response = call(
        &replayed,
        Method::GET,
        "/agent-snapshot/_doc/doc-1",
        Value::Null,
    )
    .await;
    assert_status(response.clone(), 200);
    assert_eq!(
        response.body.unwrap()["_source"]["title"],
        "Synthetic disk fixture"
    );
}

#[tokio::test]
async fn restart_replays_only_post_snapshot_mutations_after_compaction() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.snapshot_write_threshold = 2;
    let state = AppState::new(config.clone()).unwrap();

    assert_status(
        call(&state, Method::PUT, "/agent-replay", json!({})).await,
        200,
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-replay/_doc/doc-1",
            json!({ "generation": "snapshot" }),
        )
        .await,
        201,
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-replay/_doc/doc-2",
            json!({ "generation": "post-snapshot" }),
        )
        .await,
        201,
    );
    drop(state);

    let active_records = read_jsonl(&temp.path().join("mutations.jsonl"));
    let active_mutations = active_records
        .iter()
        .filter(|record| record["transaction"] == "begin")
        .flat_map(|record| record["mutations"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    assert_eq!(active_mutations.len(), 1);
    assert_eq!(active_mutations[0]["kind"], "index_document");
    assert_eq!(active_mutations[0]["id"], "doc-2");

    let replayed = AppState::new(config).unwrap();
    let doc_1 = call(
        &replayed,
        Method::GET,
        "/agent-replay/_doc/doc-1",
        Value::Null,
    )
    .await;
    let doc_2 = call(
        &replayed,
        Method::GET,
        "/agent-replay/_doc/doc-2",
        Value::Null,
    )
    .await;
    assert_status(doc_1.clone(), 200);
    assert_status(doc_2.clone(), 200);
    assert_eq!(doc_1.body.unwrap()["_source"]["generation"], "snapshot");
    assert_eq!(
        doc_2.body.unwrap()["_source"]["generation"],
        "post-snapshot"
    );
}

#[tokio::test]
async fn restart_recovers_when_crash_follows_log_compaction_before_final_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.snapshot_write_threshold = 2;
    let state = AppState::new(config.clone()).unwrap();

    assert_status(
        call(&state, Method::PUT, "/agent-compaction-crash", json!({})).await,
        200,
    );
    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-compaction-crash/_doc/doc-1",
            json!({ "survives": true }),
        )
        .await,
        201,
    );
    drop(state);

    let metadata_path = temp.path().join("snapshot.meta.json");
    let mut metadata = read_json(&metadata_path);
    assert_eq!(metadata["log_compacted"], true);
    metadata["log_compacted"] = json!(false);
    fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&metadata).expect("metadata should serialize"),
    )
    .unwrap();

    let replayed = AppState::new(config).unwrap();
    let response = call(
        &replayed,
        Method::GET,
        "/agent-compaction-crash/_doc/doc-1",
        Value::Null,
    )
    .await;
    assert_status(response.clone(), 200);
    assert_eq!(response.body.unwrap()["_source"]["survives"], true);
}

#[tokio::test]
async fn dirty_interval_flushes_only_after_a_following_write() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.snapshot_write_threshold = 1000;
    config.snapshot_interval = Duration::from_millis(1);
    let state = AppState::new(config).unwrap();

    assert_status(
        call(&state, Method::PUT, "/agent-interval", json!({})).await,
        200,
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert!(
        !temp.path().join("snapshot.json").exists(),
        "elapsed dirty time alone should not flush without another committed write"
    );

    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-interval/_doc/doc-1",
            json!({ "flushed": true }),
        )
        .await,
        201,
    );

    let metadata = read_json(&temp.path().join("snapshot.meta.json"));
    assert_eq!(metadata["index_count"], 1);
    assert_eq!(metadata["document_count"], 1);
}

#[tokio::test]
async fn restart_rejects_replayed_state_above_memory_limit() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.memory_limit_bytes = 2 * 1024 * 1024;
    config.max_body_bytes = 512 * 1024;
    let state = AppState::new(config.clone()).unwrap();

    assert_status(
        call(
            &state,
            Method::PUT,
            "/agent-replay-limit/_doc/doc-1",
            json!({ "payload": "x".repeat(64 * 1024) }),
        )
        .await,
        201,
    );
    drop(state);

    let mut constrained = config;
    constrained.memory_limit_bytes = 4 * 1024;
    constrained.max_body_bytes = 4 * 1024;
    let error = match AppState::new(constrained) {
        Ok(_) => panic!("restart should reject replayed durable state above the memory limit"),
        Err(error) => error,
    };
    let message = error.to_string();

    assert!(message.contains("loaded durable state"));
    assert!(message.contains("--memory-limit"));
    assert!(message.contains("cloud-hosted OpenSearch"));
}

#[test]
fn mutation_replay_rejects_missing_snapshot_high_water_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("mutations.jsonl");
    fs::write(
        &log,
        r#"{"version":1,"transaction":"begin","id":"tx-present","mutations":[{"kind":"create_index","name":"orders","settings":{},"mappings":{}}]}
{"version":1,"transaction":"commit","id":"tx-present"}
"#,
    )
    .unwrap();
    let mut db = Database::default();

    let error = mutation_log::replay_after(&log, &mut db, Some("tx-missing")).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error
        .to_string()
        .contains("snapshot high-water transaction"));
    assert!(db.indexes.is_empty());
}

fn assert_status(response: opensearch_lite::responses::Response, expected: u16) {
    assert_eq!(
        response.status, expected,
        "response body: {:?}",
        response.body
    );
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("mutation log line should be JSON"))
        .collect()
}

fn read_json(path: &Path) -> Value {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&contents).expect("snapshot should be JSON")
}
