mod support;

use std::{collections::BTreeSet, fs, path::Path};

use http::Method;
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

    let snapshot = read_json(&temp.path().join("snapshot.json"));
    let documents = &snapshot["indexes"]["agent-durable"]["documents"];
    assert_eq!(
        documents["doc-1"]["source"]["title"],
        "Synthetic disk fixture"
    );
    assert_eq!(documents["doc-1"]["source"]["status"], "updated");
    assert_eq!(documents["doc-1"]["source"]["score"], 11);
    assert!(
        documents.get("doc-2").is_none(),
        "deleted documents should be absent from materialized snapshot state"
    );
    assert!(snapshot["indexes"]["agent-durable"]["tombstones"]["doc-2"]
        .as_u64()
        .is_some());
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
