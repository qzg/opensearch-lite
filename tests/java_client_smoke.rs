#[test]
#[ignore = "requires Java client dependencies and an unrestricted localhost server"]
fn java_client_smoke_placeholder() {
    assert!(std::path::Path::new("scripts/run-java-client-smoke.sh").exists());
}
