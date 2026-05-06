#![allow(clippy::field_reassign_with_default)]

use std::{env, time::Duration};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Uri};
use mainstack_search::{
    agent::{
        benchmark::{
            fixture_context, grade_agent_output, load_fixtures, FixtureCheck, FixtureGrade,
        },
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
use std::path::Path;
use tokio::time::sleep;

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
            "local_runtime": "mainstack-search",
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

    let response = call_with_live_retries(
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

#[tokio::test]
#[ignore = "requires paid/network OpenAI-compatible backend credentials"]
async fn live_deepseek_backend_satisfies_agent_fallback_fixture_contracts() {
    if !live_agent_tests_enabled() {
        return;
    }
    let config = live_agent_config(true);
    let client = AgentClient::from_config(&config.agent);
    let fixtures =
        load_fixtures(Path::new("fixtures/agent_fallback")).expect("agent fallback fixtures load");
    let mut failures = Vec::new();

    for fixture in fixtures {
        let minimum_score = fixture
            .expected
            .get("minimum_live_score")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let grade = complete_and_grade_fixture(&client, &fixture).await;

        if !grade.valid_wrapper || grade.score + f64::EPSILON < minimum_score {
            failures.push(format_fixture_failure(&fixture.id, minimum_score, &grade));
        }
    }

    assert!(
        failures.is_empty(),
        "live agent fixture contract failures:\n{}",
        failures.join("\n\n")
    );
}

async fn complete_and_grade_fixture(
    client: &AgentClient,
    fixture: &mainstack_search::agent::benchmark::BenchmarkFixture,
) -> FixtureGrade {
    let mut last_error = None;
    for attempt in 1..=3 {
        match client.complete_raw(fixture_context(fixture)).await {
            Ok(wrapper) => {
                let raw = serde_json::to_string(&wrapper)
                    .expect("agent response wrapper serializes for grading");
                return grade_agent_output(fixture, &raw);
            }
            Err(error) => {
                let retryable = is_retryable_agent_error(&error.reason);
                last_error = Some(error.reason);
                if retryable && attempt < 3 {
                    sleep(Duration::from_millis(500 * attempt)).await;
                } else {
                    break;
                }
            }
        }
    }
    FixtureGrade {
        fixture_id: fixture.id.clone(),
        valid_wrapper: false,
        score: 0.0,
        checks: vec![FixtureCheck {
            name: "live_agent_call".to_string(),
            passed: false,
            reason: format!(
                "live backend request failed after retries: {}",
                last_error.unwrap_or_else(|| "unknown error".to_string())
            ),
        }],
    }
}

fn is_retryable_agent_error(reason: &str) -> bool {
    reason.contains("HTTP 429")
        || reason.contains("Too Many Requests")
        || reason.contains("endpoint request failed")
        || reason.contains("non-JSON response")
}

fn live_agent_tests_enabled() -> bool {
    if env::var("MAINSTACK_SEARCH_LIVE_AGENT_TEST").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("skipping live agent backend test; set MAINSTACK_SEARCH_LIVE_AGENT_TEST=1");
    false
}

fn format_fixture_failure(fixture_id: &str, minimum_score: f64, grade: &FixtureGrade) -> String {
    let failed_checks = grade
        .checks
        .iter()
        .filter(|check| !check.passed)
        .map(|check| format!("  - {}: {}", check.name, check.reason))
        .collect::<Vec<_>>();
    format!(
        "{fixture_id}: score {:.3} below minimum {:.3}, valid_wrapper={}\n{}",
        grade.score,
        minimum_score,
        grade.valid_wrapper,
        failed_checks.join("\n")
    )
}

fn live_agent_config(write_enabled: bool) -> Config {
    let mut config = Config::default();
    config.ephemeral = true;
    config.agent.endpoint = Some(
        env::var("MAINSTACK_SEARCH_AGENT_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_OPENROUTER_ENDPOINT.to_string()),
    );
    config.agent.model = Some(
        env::var("MAINSTACK_SEARCH_AGENT_MODEL")
            .unwrap_or_else(|_| DEFAULT_DEEPSEEK_MODEL.to_string()),
    );
    config.agent.bearer_token_env = Some(
        env::var("MAINSTACK_SEARCH_AGENT_TOKEN_ENV")
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
) -> mainstack_search::responses::Response {
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

async fn call_with_live_retries(
    state: &AppState,
    method: Method,
    path: &str,
    body: Value,
) -> mainstack_search::responses::Response {
    let mut response = call(state, method.clone(), path, body.clone()).await;
    for attempt in 1..=3 {
        if !is_retryable_agent_provider_failure(&response) || attempt == 3 {
            return response;
        }
        sleep(Duration::from_secs(2 * attempt)).await;
        response = call(state, method.clone(), path, body.clone()).await;
    }
    response
}

fn is_retryable_agent_provider_failure(response: &mainstack_search::responses::Response) -> bool {
    let Some(reason) = response
        .body
        .as_ref()
        .and_then(|body| body.pointer("/error/reason"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    reason.contains("HTTP 429")
        || reason.contains("Too Many Requests")
        || reason.contains("endpoint request failed")
        || reason.contains("non-JSON response")
}
