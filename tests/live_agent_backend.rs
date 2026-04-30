#![allow(clippy::field_reassign_with_default)]

use std::{env, time::Duration};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Uri};
use opensearch_lite::{
    agent::{
        client::AgentClient,
        context::AgentRequestContext,
        tools::tool_catalog,
        validation::{validate_wrapper_value, ValidationMode},
    },
    http::{request::Request, router},
    server::AppState,
    Config,
};
use serde_json::{json, Value};

const DEFAULT_OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek/deepseek-v4-flash";

#[tokio::test]
#[ignore = "requires paid/network OpenAI-compatible backend credentials"]
async fn live_deepseek_backend_satisfies_read_wrapper_contract() {
    if !live_agent_tests_enabled() {
        return;
    }
    let config = live_agent_config(false);
    let client = AgentClient::from_config(&config.agent);
    let context = AgentRequestContext {
        method: "GET".to_string(),
        path: "/_unknown_read_api".to_string(),
        query: json!({}),
        body: Value::Null,
        api_name: "unknown.read".to_string(),
        route_tier: "agent_fallback_eligible".to_string(),
        catalog: json!({
            "local_runtime": "OpenSearch Lite",
            "expected_behavior": "Return a valid read-only OpenSearch-compatible JSON wrapper."
        }),
        tools: tool_catalog("unknown.read", false),
    };

    let wrapper = client
        .complete_raw(context)
        .await
        .expect("live backend returns a wrapper");
    let response = validate_wrapper_value(wrapper, 75, ValidationMode::ReadOnly)
        .expect("live backend wrapper validates as read-only");

    assert!((200..600).contains(&response.status));
}

#[tokio::test]
#[ignore = "requires paid/network OpenAI-compatible backend credentials"]
async fn live_deepseek_backend_can_commit_legacy_template_through_tool_boundary() {
    if !live_agent_tests_enabled() {
        return;
    }
    let state = AppState::new(live_agent_config(true)).expect("live agent state initializes");

    let response = call(
        &state,
        Method::PUT,
        "/_template/live-agent-deepseek",
        json!({ "index_patterns": ["live-agent-*"] }),
    )
    .await;

    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body.unwrap()["acknowledged"], true);
    assert!(state
        .store
        .database()
        .registries
        .get("legacy_template")
        .map(|registry| registry.contains_key("live-agent-deepseek"))
        .unwrap_or(false));
}

fn live_agent_tests_enabled() -> bool {
    if env::var("OPENSEARCH_LITE_LIVE_AGENT_TEST").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("skipping live agent backend test; set OPENSEARCH_LITE_LIVE_AGENT_TEST=1");
    false
}

fn live_agent_config(write_enabled: bool) -> Config {
    let mut config = Config::default();
    config.ephemeral = true;
    config.agent.endpoint = Some(
        env::var("OPENSEARCH_LITE_AGENT_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_OPENROUTER_ENDPOINT.to_string()),
    );
    config.agent.model = Some(
        env::var("OPENSEARCH_LITE_AGENT_MODEL")
            .unwrap_or_else(|_| DEFAULT_DEEPSEEK_MODEL.to_string()),
    );
    config.agent.bearer_token_env = Some(
        env::var("OPENSEARCH_LITE_AGENT_TOKEN_ENV")
            .unwrap_or_else(|_| "OPENROUTER_API_KEY".to_string()),
    );
    config.agent.timeout = Duration::from_secs(30);
    config.agent.response_limit_bytes = 1024 * 1024;
    config.agent.write_enabled = write_enabled;
    if write_enabled {
        config.agent.write_allowlist = vec!["indices.put_template".to_string()];
    }
    config
}

async fn call(
    state: &AppState,
    method: Method,
    path: &str,
    body: Value,
) -> opensearch_lite::responses::Response {
    let body = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).unwrap())
    };
    let mut headers = HeaderMap::new();
    if !body.is_empty() {
        headers.insert("content-type", HeaderValue::from_static("application/json"));
    }
    let request = Request::from_parts(method, path.parse::<Uri>().unwrap(), headers, body);
    router::handle(state.clone(), request).await
}
