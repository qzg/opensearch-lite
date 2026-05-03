use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    time::{Duration, Instant},
};

use opensearch_lite::{
    search::{search, SearchRequest},
    storage::{Database, IndexMetadata, StoredDocument},
};
use serde_json::{json, Value};

fn main() {
    let db = fixture_database(5_000);
    let unsorted = time_repeated(50, || {
        run_search(
            &db,
            json!({
                "query": {
                    "bool": {
                        "filter": [
                            { "term": { "status": "paid" } },
                            { "range": { "total": { "gte": 100 } } }
                        ]
                    }
                }
            }),
        )
    });
    let sorted = time_repeated(20, || {
        run_search(
            &db,
            json!({
                "query": { "term": { "status": "paid" } },
                "sort": [{ "total": { "order": "desc" } }]
            }),
        )
    });

    let unsorted_gate = gate("OPENSEARCH_LITE_UNSORTED_SCAN_GATE_MS", 1_500);
    let sorted_gate = gate("OPENSEARCH_LITE_SORTED_SCAN_GATE_MS", 1_500);
    println!(
        "search_scan: unsorted_50={}ms gate={}ms sorted_20={}ms gate={}ms",
        unsorted.as_millis(),
        unsorted_gate.as_millis(),
        sorted.as_millis(),
        sorted_gate.as_millis()
    );
    assert!(
        unsorted <= unsorted_gate,
        "unsorted scan gate exceeded: {:?} > {:?}",
        unsorted,
        unsorted_gate
    );
    assert!(
        sorted <= sorted_gate,
        "sorted scan gate exceeded: {:?} > {:?}",
        sorted,
        sorted_gate
    );
}

fn fixture_database(documents: usize) -> Database {
    let mut docs = BTreeMap::new();
    for id in 0..documents {
        docs.insert(
            id.to_string(),
            StoredDocument {
                id: id.to_string(),
                source: json!({
                    "status": if id % 3 == 0 { "paid" } else { "open" },
                    "total": id as f64,
                    "customer_id": format!("c{}", id % 250),
                    "name": format!("fixture order {id}")
                }),
                version: 1,
                seq_no: id as u64 + 1,
                primary_term: 1,
            },
        );
    }
    let mut indexes = BTreeMap::new();
    indexes.insert(
        "orders".to_string(),
        IndexMetadata {
            name: "orders".to_string(),
            settings: json!({}),
            mappings: json!({}),
            aliases: BTreeSet::new(),
            documents: docs,
            tombstones: BTreeMap::new(),
            store_size_bytes: documents * 128,
        },
    );
    Database {
        indexes,
        templates: BTreeMap::new(),
        registries: BTreeMap::new(),
        aliases: BTreeMap::new(),
        seq_no: documents as u64,
    }
}

fn run_search(db: &Database, body: Value) {
    let response = search(
        db,
        SearchRequest {
            indices: vec!["orders".to_string()],
            body,
            from: 0,
            size: 10,
            pit: false,
        },
    )
    .expect("benchmark query is supported");
    assert!(response["hits"]["total"]["value"].as_u64().unwrap_or(0) > 0);
}

fn time_repeated(iterations: usize, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    start.elapsed()
}

fn gate(name: &str, default_ms: u64) -> Duration {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(default_ms))
}
