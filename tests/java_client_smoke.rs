#[path = "support/client_smoke.rs"]
mod client_smoke_support;

#[test]
#[ignore = "requires Java client dependencies and an unrestricted localhost server"]
fn java_client_smoke() {
    client_smoke_support::run_script("scripts/run-java-client-smoke.sh");
}
