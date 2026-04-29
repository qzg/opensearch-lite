#[test]
#[ignore = "requires opensearch-py and an unrestricted localhost server"]
fn python_client_smoke_placeholder() {
    assert!(std::path::Path::new("scripts/run-python-client-smoke.sh").exists());
}
