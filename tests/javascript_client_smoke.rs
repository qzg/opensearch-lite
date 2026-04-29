#[test]
#[ignore = "requires node dependencies and an unrestricted localhost server"]
fn javascript_client_smoke_placeholder() {
    assert!(std::path::Path::new("scripts/run-javascript-client-smoke.sh").exists());
}
