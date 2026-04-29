use opensearch_lite::{
    agent::validation::{validate_wrapper, AgentResponseWrapper},
    Config,
};
use serde_json::json;

#[test]
fn fallback_config_redacts_by_reference_and_rejects_insecure_network_http() {
    let error = Config::from_args([
        "opensearch-lite",
        "--agent-endpoint",
        "http://example.test/v1/chat/completions",
        "--agent-token-env",
        "OPENAI_API_KEY",
    ])
    .unwrap_err();

    assert!(error.contains("http:// is only allowed"));
}

#[test]
fn wrapper_validation_rejects_write_intent() {
    let wrapper = AgentResponseWrapper {
        status: 200,
        headers: Default::default(),
        body: json!({ "ok": true }),
        confidence: 100,
        failure_reason: None,
        read_only: false,
    };
    let raw = serde_json::to_string(&wrapper).unwrap();

    let error = validate_wrapper(&raw, 75, 4096).unwrap_err();
    assert!(error.reason.contains("write intent"));
}
