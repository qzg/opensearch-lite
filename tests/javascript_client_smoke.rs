#[path = "support/client_smoke.rs"]
mod client_smoke_support;

#[test]
#[ignore = "requires node dependencies and an unrestricted localhost server"]
fn javascript_client_smoke() {
    client_smoke_support::run_script("scripts/run-javascript-client-smoke.sh");
}
