#[path = "support/client_smoke.rs"]
mod client_smoke_support;

#[test]
#[ignore = "requires opensearch-py and an unrestricted localhost server"]
fn python_client_smoke() {
    client_smoke_support::run_script("scripts/run-python-client-smoke.sh");
}
