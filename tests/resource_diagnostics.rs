#![allow(clippy::field_reassign_with_default)]

use std::fs;

use mainstack_search::{resources, Config};
use serde_json::json;

#[test]
fn cgroup_v2_unbounded_memory_is_unknown() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("memory.max"), "max\n").unwrap();

    assert_eq!(
        resources::detect_container_memory_limit_at(temp.path()),
        None
    );
}

#[test]
fn cgroup_memory_limit_is_detected() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("memory.max"), "268435456\n").unwrap();

    assert_eq!(
        resources::detect_container_memory_limit_at(temp.path()),
        Some(256 * 1024 * 1024)
    );
}

#[test]
fn compatible_container_budget_passes_validation() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.memory_limit_bytes = 64 * 1024 * 1024;

    let diagnostics =
        resources::validate_with_container_limit(&config, Some(256 * 1024 * 1024)).unwrap();

    assert_eq!(diagnostics.configured_memory_limit_bytes, 64 * 1024 * 1024);
    assert_eq!(
        diagnostics.detected_container_limit_bytes,
        Some(256 * 1024 * 1024)
    );
    assert_eq!(diagnostics.snapshot_estimated_stored_bytes, None);
}

#[test]
fn configured_budget_above_container_safe_budget_fails_with_remediation() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.memory_limit_bytes = 512 * 1024 * 1024;

    let error =
        resources::validate_with_container_limit(&config, Some(512 * 1024 * 1024)).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("--memory-limit"));
    assert!(message.contains("container memory"));
    assert!(message.contains("full OpenSearch"));
    assert!(message.contains("cloud-hosted OpenSearch"));
}

#[test]
fn snapshot_metadata_above_budget_fails_before_full_snapshot_load() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("snapshot.meta.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "generation": 7,
            "created_at_unix_millis": 1,
            "estimated_stored_bytes": 90 * 1024 * 1024,
            "index_count": 1,
            "document_count": 10,
            "template_count": 0,
            "alias_count": 0,
            "seq_no": 10,
            "last_transaction_id": "tx-test",
            "log_compacted": true,
            "indexes": []
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();
    config.memory_limit_bytes = 64 * 1024 * 1024;

    let error = resources::validate_with_container_limit(&config, None).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("snapshot metadata estimates stored data"));
    assert!(message.contains("--memory-limit"));
}
